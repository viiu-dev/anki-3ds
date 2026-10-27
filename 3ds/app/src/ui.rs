// Copyright: Anki 3DS contributors
// License: GNU AGPL, version 3 or later; http://www.gnu.org/licenses/agpl.html

use std::borrow::Cow;
use std::ffi::CString;
use std::os::raw::c_char;
use std::os::raw::c_int;

use super::i18n::tr;
use super::review_text::Block;
use super::review_text::Inline;
use super::review_text::PitchMarker;
use super::review_text::Style;

pub const TOP_WIDTH: f32 = 400.0;
pub const TOP_HEIGHT: f32 = 240.0;
pub const BOTTOM_WIDTH: f32 = 320.0;
pub const CARD_WIDTH: f32 = 380.0;

const CARD_TEXT_SCALE: f32 = 0.55;
const CARD_LINE_HEIGHT: f32 = 19.0;
// Furigana is deliberately larger than the usual ~60% of the base size so
// readings stay legible at the 3DS's small bitmap glyph sizes.
const CARD_RUBY_SCALE: f32 = 0.78;
const CARD_RUBY_HEIGHT: f32 = 14.0;
const CARD_BLOCK_GAP: f32 = 6.0;
const CARD_SECTION_GAP: f32 = 13.0;
const CARD_SECTION_RULE_HEIGHT: f32 = 9.0;
const CARD_LIST_GAP: f32 = 2.0;
const CARD_Y_PADDING: f32 = 8.0;
const CARD_RULE_HEIGHT: f32 = 12.0;
const CARD_CJK_SCALE: f32 = 1.18;
const CARD_HEADWORD_SCALE: f32 = 1.20;
const CARD_HEADWORD_MAX_CHARS: usize = 28;
const CARD_PITCH_HEIGHT: f32 = 6.0;
const CARD_PITCH_GAP: f32 = 4.0;

#[inline]
pub const fn rgba(r: u8, g: u8, b: u8, a: u8) -> u32 {
    (r as u32) | ((g as u32) << 8) | ((b as u32) << 16) | ((a as u32) << 24)
}

pub const COLOR_BG_TOP: u32 = rgba(246, 246, 242, 255);
pub const COLOR_BG_BOTTOM: u32 = rgba(236, 238, 240, 255);
pub const COLOR_PANEL_WHITE: u32 = rgba(254, 254, 252, 255);
pub const COLOR_PANEL_BORDER: u32 = rgba(214, 214, 208, 255);
pub const COLOR_TEXT_DARK: u32 = rgba(30, 30, 30, 255);
pub const COLOR_TEXT_MUTED: u32 = rgba(100, 105, 115, 255);
pub const COLOR_COUNT_DUE: u32 = rgba(36, 130, 56, 255);
pub const COLOR_COUNT_NEW: u32 = rgba(20, 100, 200, 255);
pub const COLOR_COUNT_LEARN: u32 = rgba(210, 60, 40, 255);
pub const COLOR_CLOZE_BG: u32 = rgba(255, 243, 166, 255);
pub const COLOR_PITCH: u32 = rgba(37, 99, 235, 255);

extern "C" {
    fn ui_keyboard(
        hint: *const c_char,
        initial: *const c_char,
        output: *mut c_char,
        capacity: usize,
        secret: bool,
        cancel: *const c_char,
        accept: *const c_char,
    ) -> c_int;
    fn ui_init() -> bool;
    fn ui_wifi_connected() -> bool;
    fn ui_fini();
    fn ui_frame_begin();
    fn ui_frame_end();
    fn ui_target_top(bg_color: u32);
    fn ui_target_bottom(bg_color: u32);

    fn ui_draw_rect(x: f32, y: f32, w: f32, h: f32, color: u32);
    fn ui_draw_line(x0: f32, y0: f32, x1: f32, y1: f32, thickness: f32, color: u32);

    fn ui_draw_text(x: f32, y: f32, scale: f32, color: u32, str: *const c_char);
    fn ui_draw_text_centered(x_center: f32, y: f32, scale: f32, color: u32, str: *const c_char);
    fn ui_draw_text_right(x_right: f32, y: f32, scale: f32, color: u32, str: *const c_char);
    fn ui_get_text_width(scale: f32, str: *const c_char) -> f32;
    fn ui_draw_card_text(
        x: f32,
        y: f32,
        scale: f32,
        color: u32,
        str: *const c_char,
        bold: bool,
        italic: bool,
    );
    fn ui_get_card_text_width(scale: f32, str: *const c_char) -> f32;
    fn ui_draw_status_compact(x_right: f32, y: f32, scale: f32);
    fn ui_draw_scroll_indicator(x_right: f32, y: f32, w: f32, h: f32, scroll: f32, max_scroll: f32);

    fn ui_draw_nav_tab(
        x: f32,
        y: f32,
        w: f32,
        h: f32,
        label: *const c_char,
        icon_type: c_int,
        active: bool,
    );
    fn ui_draw_deck_row(
        y: f32,
        w: f32,
        name: *const c_char,
        level: c_int,
        has_children: bool,
        expanded: bool,
        due_count: u32,
        new_count: u32,
        learn_count: u32,
        selected: bool,
    );
    fn ui_draw_scrollbar(
        x: f32,
        y: f32,
        w: f32,
        h: f32,
        total_items: c_int,
        visible_items: c_int,
        scroll_pos: c_int,
    );
    fn ui_draw_button(
        x: f32,
        y: f32,
        w: f32,
        h: f32,
        text: *const c_char,
        subtext: *const c_char,
        style: c_int,
        pressed: bool,
    );

}

// Citro2D renders Fluent's invisible direction isolates as missing glyphs.
// Current UI locales are left-to-right; leave translated text and values
// intact.
fn display_text(text: &str) -> Cow<'_, str> {
    if text.chars().any(|c| matches!(c, '\u{2066}'..='\u{2069}')) {
        Cow::Owned(
            text.chars()
                .filter(|c| !matches!(c, '\u{2066}'..='\u{2069}'))
                .collect(),
        )
    } else {
        Cow::Borrowed(text)
    }
}

pub struct Ui;

impl Ui {
    pub fn wifi_connected() -> bool {
        unsafe { ui_wifi_connected() }
    }
    pub fn keyboard(hint: &str, initial: &str) -> Result<Option<String>, String> {
        Self::keyboard_with_mode(hint, initial, false)
    }
    pub fn keyboard_secret(hint: &str) -> Result<Option<String>, String> {
        Self::keyboard_with_mode(hint, "", true)
    }
    fn keyboard_with_mode(
        hint: &str,
        initial: &str,
        secret: bool,
    ) -> Result<Option<String>, String> {
        let hint = CString::new(hint).map_err(|e| e.to_string())?;
        let initial = CString::new(initial).map_err(|e| e.to_string())?;
        let cancel = CString::new(tr("Cancel")).unwrap();
        let accept = CString::new(tr("Accept")).unwrap();
        let mut output = vec![0u8; 8192];
        match unsafe {
            ui_keyboard(
                hint.as_ptr(),
                initial.as_ptr(),
                output.as_mut_ptr().cast(),
                output.len(),
                secret,
                cancel.as_ptr(),
                accept.as_ptr(),
            )
        } {
            1 => {
                let end = output.iter().position(|b| *b == 0).unwrap_or(output.len());
                let value = String::from_utf8(output[..end].to_vec())
                    .map(Some)
                    .map_err(|e| e.to_string());
                if secret {
                    output.fill(0);
                }
                value
            }
            0 => Ok(None),
            _ => Err(tr("Could not open Nintendo 3DS keyboard.").into()),
        }
    }

    pub fn fit_text(center: f32, y: f32, scale: f32, width: f32, color: u32, text: &str) {
        let text = Self::ellipsize(text, scale, width);
        Self::draw_text_centered(center, y, scale, color, &text);
    }

    pub fn ellipsize(text: &str, scale: f32, width: f32) -> String {
        if Self::get_text_width(scale, text) <= width {
            return text.to_owned();
        }
        let ends: Vec<usize> = text
            .char_indices()
            .map(|(i, _)| i)
            .chain(std::iter::once(text.len()))
            .collect();
        let mut low = 0;
        let mut high = ends.len() - 1;
        while low < high {
            let mid = (low + high + 1) / 2;
            if Self::get_text_width(scale, &format!("{}...", &text[..ends[mid]])) <= width {
                low = mid;
            } else {
                high = mid - 1;
            }
        }
        format!("{}...", &text[..ends[low]])
    }

    pub fn init() -> bool {
        unsafe { ui_init() }
    }

    pub fn fini() {
        unsafe { ui_fini() }
    }

    pub fn frame_begin() {
        unsafe { ui_frame_begin() }
    }

    pub fn frame_end() {
        unsafe { ui_frame_end() }
    }

    pub fn target_top(bg_color: u32) {
        unsafe { ui_target_top(bg_color) }
    }

    pub fn target_bottom(bg_color: u32) {
        unsafe { ui_target_bottom(bg_color) }
    }

    pub fn draw_rect(x: f32, y: f32, w: f32, h: f32, color: u32) {
        unsafe { ui_draw_rect(x, y, w, h, color) }
    }

    pub fn draw_line(x0: f32, y0: f32, x1: f32, y1: f32, thickness: f32, color: u32) {
        unsafe { ui_draw_line(x0, y0, x1, y1, thickness, color) }
    }

    pub fn draw_text(x: f32, y: f32, scale: f32, color: u32, s: &str) {
        if let Ok(c_str) = CString::new(display_text(s).as_bytes()) {
            unsafe { ui_draw_text(x, y, scale, color, c_str.as_ptr()) }
        }
    }

    pub fn draw_text_centered(x_center: f32, y: f32, scale: f32, color: u32, s: &str) {
        if let Ok(c_str) = CString::new(display_text(s).as_bytes()) {
            unsafe { ui_draw_text_centered(x_center, y, scale, color, c_str.as_ptr()) }
        }
    }

    pub fn draw_text_right(x_right: f32, y: f32, scale: f32, color: u32, s: &str) {
        if let Ok(c_str) = CString::new(display_text(s).as_bytes()) {
            unsafe { ui_draw_text_right(x_right, y, scale, color, c_str.as_ptr()) }
        }
    }

    pub fn get_text_width(scale: f32, s: &str) -> f32 {
        if let Ok(c_str) = CString::new(display_text(s).as_bytes()) {
            unsafe { ui_get_text_width(scale, c_str.as_ptr()) }
        } else {
            0.0
        }
    }

    pub fn draw_card_text(
        x: f32,
        y: f32,
        scale: f32,
        color: u32,
        s: &str,
        bold: bool,
        italic: bool,
    ) {
        if let Ok(c_str) = CString::new(display_text(s).as_bytes()) {
            unsafe { ui_draw_card_text(x, y, scale, color, c_str.as_ptr(), bold, italic) }
        }
    }

    pub fn get_card_text_width(scale: f32, s: &str) -> f32 {
        if let Ok(c_str) = CString::new(display_text(s).as_bytes()) {
            unsafe { ui_get_card_text_width(scale, c_str.as_ptr()) }
        } else {
            0.0
        }
    }

    pub fn draw_status_compact(x_right: f32, y: f32, scale: f32) {
        unsafe { ui_draw_status_compact(x_right, y, scale) }
    }

    pub fn draw_scroll_indicator(x: f32, y: f32, w: f32, h: f32, scroll: f32, max_scroll: f32) {
        unsafe { ui_draw_scroll_indicator(x, y, w, h, scroll, max_scroll) }
    }

    pub fn draw_nav_tab(x: f32, y: f32, w: f32, h: f32, label: &str, icon_type: i32, active: bool) {
        if let Ok(c_str) = CString::new(display_text(label).as_bytes()) {
            unsafe { ui_draw_nav_tab(x, y, w, h, c_str.as_ptr(), icon_type as c_int, active) }
        }
    }

    pub fn draw_deck_row(
        y: f32,
        w: f32,
        name: &str,
        level: i32,
        has_children: bool,
        expanded: bool,
        due_count: u32,
        new_count: u32,
        learn_count: u32,
        selected: bool,
    ) {
        if let Ok(c_str) = CString::new(display_text(name).as_bytes()) {
            unsafe {
                ui_draw_deck_row(
                    y,
                    w,
                    c_str.as_ptr(),
                    level as c_int,
                    has_children,
                    expanded,
                    due_count,
                    new_count,
                    learn_count,
                    selected,
                )
            }
        }
    }

    pub fn draw_scrollbar(
        x: f32,
        y: f32,
        w: f32,
        h: f32,
        total_items: usize,
        visible_items: usize,
        scroll_pos: usize,
    ) {
        unsafe {
            ui_draw_scrollbar(
                x,
                y,
                w,
                h,
                total_items as c_int,
                visible_items as c_int,
                scroll_pos as c_int,
            )
        }
    }

    pub fn draw_button(
        x: f32,
        y: f32,
        w: f32,
        h: f32,
        text: &str,
        subtext: Option<&str>,
        style: i32,
        pressed: bool,
    ) {
        if let Ok(c_text) = CString::new(display_text(text).as_bytes()) {
            let c_sub = subtext.and_then(|s| CString::new(display_text(s).as_bytes()).ok());
            let sub_ptr = c_sub
                .as_ref()
                .map(|c| c.as_ptr())
                .unwrap_or(std::ptr::null());
            unsafe {
                ui_draw_button(
                    x,
                    y,
                    w,
                    h,
                    c_text.as_ptr(),
                    sub_ptr,
                    style as c_int,
                    pressed,
                )
            }
        }
    }
}

#[derive(Clone)]
struct CardPiece {
    text: String,
    style: Style,
    x: f32,
    width: f32,
    base_width: f32,
    ruby: Option<String>,
    rise: f32,
    scale: f32,
    pitch_marker: Option<PitchMarker>,
}

struct CardLine {
    top: f32,
    height: f32,
    ruby_height: f32,
    upper: f32,
    width: f32,
    pitch_height: f32,
    marker: Option<String>,
    pieces: Vec<CardPiece>,
    rule: bool,
}

struct LineDraft {
    pieces: Vec<CardPiece>,
    x: f32,
    ruby_height: f32,
    upper: f32,
    lower: f32,
    max_scale: f32,
    pitch_height: f32,
    marker: Option<String>,
}

impl LineDraft {
    fn new(x: f32, marker: Option<String>) -> Self {
        Self {
            pieces: Vec::new(),
            x,
            ruby_height: 0.0,
            upper: 0.0,
            lower: 0.0,
            max_scale: 1.0,
            pitch_height: 0.0,
            marker,
        }
    }

    fn has_content(&self) -> bool {
        !self.pieces.is_empty()
    }
}

#[derive(Clone, Copy)]
struct CardMetrics {
    width: f32,
    font_scale: f32,
    nominal: f32,
    content_left: f32,
}

/// Cached rich-card layout. HTML is parsed and measured only when the card or
/// the selected font scale changes; drawing then uses the cached lines.
#[derive(Default)]
pub struct RichTextLayout {
    html: String,
    width: f32,
    font_scale: f32,
    lines: Vec<CardLine>,
    content_height: f32,
    ready: bool,
}

impl RichTextLayout {
    pub fn set_text(&mut self, html: &str, width: f32, font_scale: f32) {
        let font_scale = font_scale.clamp(0.5, 2.5);
        if self.ready
            && self.html == html
            && (self.width - width).abs() < 0.01
            && (self.font_scale - font_scale).abs() < 0.001
        {
            return;
        }
        self.html.clear();
        self.html.push_str(html);
        self.width = width;
        self.font_scale = font_scale;

        let blocks = super::review_text::parse_html(html);
        let nominal = CARD_LINE_HEIGHT * font_scale;
        let block_gap = CARD_BLOCK_GAP * font_scale;
        let section_gap = CARD_SECTION_GAP * font_scale;
        let section_rule_height = CARD_SECTION_RULE_HEIGHT * font_scale;
        let list_gap = CARD_LIST_GAP * font_scale;
        let rule_height = CARD_RULE_HEIGHT * font_scale;
        let mut lines = Vec::new();
        let mut cursor = 0.0f32;
        let mut first = true;
        for block in blocks {
            let between = if first {
                0.0
            } else if matches!(&block, Block::ListItem { .. }) {
                list_gap
            } else if matches!(&block, Block::Section(_)) {
                section_gap
            } else {
                block_gap
            };
            match block {
                Block::Rule => {
                    cursor += between;
                    lines.push(CardLine {
                        top: cursor,
                        height: rule_height,
                        ruby_height: 0.0,
                        upper: 0.0,
                        width,
                        pitch_height: 0.0,
                        marker: None,
                        pieces: Vec::new(),
                        rule: true,
                    });
                    cursor += rule_height;
                }
                Block::Paragraph(content) => {
                    cursor += between;
                    let block_scale = if first {
                        headword_multiplier(&content)
                    } else {
                        1.0
                    };
                    self.layout_inlines(
                        &content,
                        None,
                        width,
                        font_scale,
                        block_scale,
                        &mut lines,
                        &mut cursor,
                        nominal,
                    );
                }
                Block::Section(content) => {
                    cursor += between;
                    if !first && !lines.last().is_some_and(|line| line.rule) {
                        lines.push(CardLine {
                            top: cursor,
                            height: section_rule_height,
                            ruby_height: 0.0,
                            upper: 0.0,
                            width,
                            pitch_height: 0.0,
                            marker: None,
                            pieces: Vec::new(),
                            rule: true,
                        });
                        cursor += section_rule_height;
                    }
                    let block_scale = if first {
                        headword_multiplier(&content)
                    } else {
                        1.0
                    };
                    self.layout_inlines(
                        &content,
                        None,
                        width,
                        font_scale,
                        block_scale,
                        &mut lines,
                        &mut cursor,
                        nominal,
                    );
                }
                Block::ListItem { marker, content } => {
                    cursor += between;
                    self.layout_inlines(
                        &content,
                        Some(marker),
                        width,
                        font_scale,
                        1.0,
                        &mut lines,
                        &mut cursor,
                        nominal,
                    );
                }
            }
            first = false;
        }
        self.content_height = cursor;
        self.lines = lines;
        self.ready = true;
    }

    pub fn max_scroll(&self) -> f32 {
        (self.content_height + 2.0 * CARD_Y_PADDING - TOP_HEIGHT).max(0.0)
    }

    pub fn draw(&self, scroll: f32) {
        if self.lines.is_empty() {
            return;
        }
        let total = self.content_height + 2.0 * CARD_Y_PADDING;
        let origin = if total <= TOP_HEIGHT {
            CARD_Y_PADDING + (TOP_HEIGHT - total) / 2.0
        } else {
            CARD_Y_PADDING - scroll.max(0.0)
        };
        let scale = CARD_TEXT_SCALE * self.font_scale;
        let ruby_scale = scale * CARD_RUBY_SCALE;
        let content_left = (TOP_WIDTH - self.width).max(0.0) / 2.0;
        for line in &self.lines {
            let top = origin + line.top;
            if top > TOP_HEIGHT || top + line.height < 0.0 {
                continue;
            }
            if line.rule {
                let y = top + line.height * 0.5;
                Ui::draw_line(
                    content_left,
                    y,
                    content_left + self.width,
                    y,
                    1.0,
                    COLOR_PANEL_BORDER,
                );
                continue;
            }
            let line_left = content_left + (self.width - line.width).max(0.0) / 2.0;
            let pitch_top = top + line.pitch_height + line.upper;
            let text_top = pitch_top + line.ruby_height;
            if let Some(marker) = &line.marker {
                Ui::draw_card_text(
                    line_left,
                    text_top,
                    scale,
                    COLOR_TEXT_MUTED,
                    marker,
                    false,
                    false,
                );
            }
            for piece in &line.pieces {
                let x = line_left + piece.x;
                if let Some(marker) = &piece.pitch_marker {
                    self.draw_pitch_marker(marker, x, text_top, pitch_top, piece.width, scale);
                    continue;
                }
                let text_scale = scale
                    * piece.scale
                    * if piece.style.sub || piece.style.sup {
                        0.75
                    } else {
                        1.0
                    };
                let y = text_top + piece.rise;
                let base_x = x + (piece.width - piece.base_width) / 2.0;
                let color = if let Some(rgb) = piece.style.color {
                    rgba(rgb[0], rgb[1], rgb[2], 255)
                } else if piece.style.cloze {
                    COLOR_COUNT_NEW
                } else {
                    COLOR_TEXT_DARK
                };
                let highlight = piece
                    .style
                    .highlight
                    .map(|rgb| rgba(rgb[0], rgb[1], rgb[2], 255))
                    .or_else(|| piece.style.cloze.then_some(COLOR_CLOZE_BG));
                if let Some(highlight) = highlight {
                    let height = 15.0
                        * self.font_scale
                        * piece.scale
                        * if piece.style.sub || piece.style.sup {
                            0.75
                        } else {
                            1.0
                        };
                    Ui::draw_rect(base_x, y - 1.0, piece.base_width, height, highlight);
                }
                Ui::draw_card_text(
                    base_x,
                    y,
                    text_scale,
                    color,
                    &piece.text,
                    piece.style.bold,
                    piece.style.italic,
                );
                if piece.style.underline {
                    let underline_y = y + 15.0
                        * self.font_scale
                        * piece.scale
                        * if piece.style.sub || piece.style.sup {
                            0.75
                        } else {
                            1.0
                        };
                    Ui::draw_line(
                        base_x,
                        underline_y,
                        base_x + piece.base_width,
                        underline_y,
                        1.0,
                        color,
                    );
                }
                if piece.style.pitch.high || piece.style.pitch.downstep || piece.style.pitch.upstep
                {
                    let bar_y = pitch_top - CARD_PITCH_GAP * self.font_scale;
                    if piece.style.pitch.high {
                        Ui::draw_line(
                            base_x,
                            bar_y,
                            base_x + piece.base_width,
                            bar_y,
                            1.5,
                            COLOR_PITCH,
                        );
                    }
                    if piece.style.pitch.downstep {
                        Ui::draw_line(
                            base_x + piece.base_width,
                            bar_y,
                            base_x + piece.base_width,
                            y + 2.0 * self.font_scale,
                            1.5,
                            COLOR_PITCH,
                        );
                    }
                    if piece.style.pitch.upstep {
                        Ui::draw_line(
                            base_x,
                            y + 2.0 * self.font_scale,
                            base_x,
                            bar_y,
                            1.5,
                            COLOR_PITCH,
                        );
                    }
                }
                if let Some(reading) = &piece.ruby {
                    let reading_scale = ruby_scale * piece.scale;
                    let reading_width = Ui::get_card_text_width(reading_scale, reading);
                    Ui::draw_card_text(
                        x + (piece.width - reading_width) / 2.0,
                        top + line.pitch_height + line.upper,
                        reading_scale,
                        color,
                        reading,
                        false,
                        false,
                    );
                }
            }
        }
    }

    fn draw_pitch_marker(
        &self,
        marker: &PitchMarker,
        x: f32,
        text_top: f32,
        pitch_top: f32,
        width: f32,
        scale: f32,
    ) {
        let factor = self.font_scale;
        let bar_y = pitch_top - CARD_PITCH_GAP * factor;
        match marker {
            PitchMarker::High => {
                Ui::draw_line(x, bar_y, x + width, bar_y, 1.5, COLOR_PITCH);
            }
            PitchMarker::Up => {
                let mid = x + width / 2.0;
                Ui::draw_line(x, bar_y + 4.0 * factor, mid, bar_y, 1.4, COLOR_PITCH);
                Ui::draw_line(
                    mid,
                    bar_y,
                    x + width,
                    bar_y + 4.0 * factor,
                    1.4,
                    COLOR_PITCH,
                );
            }
            PitchMarker::Down => {
                let mid = x + width / 2.0;
                Ui::draw_line(x, bar_y, mid, bar_y + 4.0 * factor, 1.4, COLOR_PITCH);
                Ui::draw_line(
                    mid,
                    bar_y + 4.0 * factor,
                    x + width,
                    bar_y,
                    1.4,
                    COLOR_PITCH,
                );
            }
            PitchMarker::Accent(number) => {
                let text = number.to_string();
                let text_scale = scale * 0.62;
                let text_width = Ui::get_card_text_width(text_scale, &text);
                Ui::draw_card_text(
                    x + (width - text_width) / 2.0,
                    text_top + 1.0,
                    text_scale,
                    COLOR_PITCH,
                    &text,
                    false,
                    false,
                );
            }
            PitchMarker::Graph(points) => {
                let height = CARD_PITCH_HEIGHT * factor;
                let mut previous = None;
                for point in points {
                    let px = x + point.0 * width;
                    let py = pitch_top - height + point.1 * height;
                    if let Some((last_x, last_y)) = previous {
                        Ui::draw_line(last_x, last_y, px, py, 1.4, COLOR_PITCH);
                    }
                    previous = Some((px, py));
                }
            }
        }
    }

    #[allow(clippy::too_many_arguments)]
    fn layout_inlines(
        &self,
        inlines: &[Inline],
        marker: Option<String>,
        width: f32,
        font_scale: f32,
        block_scale: f32,
        lines: &mut Vec<CardLine>,
        cursor: &mut f32,
        nominal: f32,
    ) {
        let scale = CARD_TEXT_SCALE * font_scale;
        let marker_width = marker
            .as_ref()
            .map(|marker| Ui::get_card_text_width(scale, marker))
            .unwrap_or(0.0);
        let content_left = if marker.is_some() {
            marker_width + 4.0 * font_scale
        } else {
            0.0
        };
        let metrics = CardMetrics {
            width,
            font_scale,
            nominal,
            content_left,
        };
        let mut draft = LineDraft::new(content_left, marker);
        let mut pending_space = false;
        for inline in inlines {
            match inline {
                Inline::Text { text, style } => {
                    let mut start = None;
                    for (index, ch) in text.char_indices() {
                        if ch.is_whitespace() {
                            if let Some(start) = start.take() {
                                self.place_word(
                                    &text[start..index],
                                    style,
                                    None,
                                    block_scale,
                                    metrics,
                                    &mut draft,
                                    &mut pending_space,
                                    lines,
                                    cursor,
                                );
                            }
                            pending_space = true;
                        } else if start.is_none() {
                            start = Some(index);
                        }
                    }
                    if let Some(start) = start {
                        self.place_word(
                            &text[start..],
                            style,
                            None,
                            block_scale,
                            metrics,
                            &mut draft,
                            &mut pending_space,
                            lines,
                            cursor,
                        );
                    }
                }
                Inline::Ruby {
                    base,
                    reading,
                    style,
                } => {
                    self.place_word(
                        base,
                        style,
                        Some(reading.clone()),
                        block_scale,
                        metrics,
                        &mut draft,
                        &mut pending_space,
                        lines,
                        cursor,
                    );
                }
                Inline::PitchWord {
                    base,
                    accent,
                    style,
                } => {
                    self.place_pitch_word(
                        base,
                        *accent,
                        style,
                        block_scale,
                        metrics,
                        &mut draft,
                        &mut pending_space,
                        lines,
                        cursor,
                    );
                }
                Inline::Marker(marker) => {
                    self.place_marker(
                        marker,
                        metrics,
                        &mut draft,
                        &mut pending_space,
                        lines,
                        cursor,
                    );
                }
                Inline::Break => {
                    let force = !(draft.pieces.is_empty() && draft.marker.is_none());
                    finish_line(
                        &mut draft,
                        lines,
                        cursor,
                        metrics,
                        force || !lines.is_empty(),
                    );
                    pending_space = false;
                }
            }
        }
        if draft.has_content() || draft.marker.is_some() {
            finish_line(&mut draft, lines, cursor, metrics, true);
        }
    }

    #[allow(clippy::too_many_arguments)]
    fn place_word(
        &self,
        text: &str,
        style: &Style,
        ruby: Option<String>,
        block_scale: f32,
        metrics: CardMetrics,
        draft: &mut LineDraft,
        pending_space: &mut bool,
        lines: &mut Vec<CardLine>,
        cursor: &mut f32,
    ) {
        if text.is_empty() {
            return;
        }
        if ruby.is_some() {
            let multiplier = block_scale * script_multiplier(text);
            self.place_run(
                text,
                style,
                ruby,
                multiplier,
                metrics,
                draft,
                pending_space,
                lines,
                cursor,
            );
            return;
        }
        if let Some(accent) = style.pitch.accent {
            let mut clean = style.clone();
            clean.pitch = Default::default();
            self.place_pitch_word(
                text,
                accent,
                &clean,
                block_scale,
                metrics,
                draft,
                pending_space,
                lines,
                cursor,
            );
            return;
        }
        for (start, end) in script_runs(text) {
            let run = &text[start..end];
            let multiplier = block_scale
                * if is_cjk_text(run) {
                    CARD_CJK_SCALE
                } else {
                    1.0
                };
            self.place_run(
                run,
                style,
                None,
                multiplier,
                metrics,
                draft,
                pending_space,
                lines,
                cursor,
            );
        }
    }

    #[allow(clippy::too_many_arguments)]
    fn place_pitch_word(
        &self,
        base: &str,
        accent: u8,
        style: &Style,
        block_scale: f32,
        metrics: CardMetrics,
        draft: &mut LineDraft,
        pending_space: &mut bool,
        lines: &mut Vec<CardLine>,
        cursor: &mut f32,
    ) {
        let mut mora = -1i32;
        for ch in base.chars() {
            if !is_small_kana(ch) {
                mora += 1;
            }
            let mora = mora.max(0) as u8;
            let high = if accent == 0 {
                mora >= 1
            } else if accent == 1 {
                mora == 0
            } else {
                mora >= 1 && mora < accent
            };
            let downstep = accent > 0 && mora + 1 == accent;
            let mut piece_style = style.clone();
            piece_style.pitch.high = high;
            piece_style.pitch.low = !high;
            piece_style.pitch.downstep = downstep;
            let multiplier = block_scale * if is_cjk_char(ch) { CARD_CJK_SCALE } else { 1.0 };
            let mut buffer = [0u8; 4];
            let piece = ch.encode_utf8(&mut buffer);
            self.place_run(
                piece,
                &piece_style,
                None,
                multiplier,
                metrics,
                draft,
                pending_space,
                lines,
                cursor,
            );
        }
    }

    fn place_marker(
        &self,
        marker: &PitchMarker,
        metrics: CardMetrics,
        draft: &mut LineDraft,
        pending_space: &mut bool,
        lines: &mut Vec<CardLine>,
        cursor: &mut f32,
    ) {
        let factor = metrics.font_scale;
        let width = match marker {
            PitchMarker::High => 10.0 * factor,
            PitchMarker::Up | PitchMarker::Down => 7.0 * factor,
            PitchMarker::Accent(_) => 10.0 * factor,
            PitchMarker::Graph(_) => 30.0 * factor,
        };
        if draft.has_content() && draft.x + width > metrics.width {
            finish_line(draft, lines, cursor, metrics, true);
        }
        let x = draft.x;
        draft.pieces.push(CardPiece {
            text: String::new(),
            style: Style::default(),
            x,
            width,
            base_width: width,
            ruby: None,
            rise: 0.0,
            scale: 1.0,
            pitch_marker: Some(marker.clone()),
        });
        draft.x += width;
        if matches!(
            marker,
            PitchMarker::High | PitchMarker::Up | PitchMarker::Down | PitchMarker::Graph(_)
        ) {
            draft.pitch_height = draft.pitch_height.max(CARD_PITCH_HEIGHT * factor);
        }
        *pending_space = false;
    }

    #[allow(clippy::too_many_arguments)]
    fn place_run(
        &self,
        text: &str,
        style: &Style,
        ruby: Option<String>,
        scale_multiplier: f32,
        metrics: CardMetrics,
        draft: &mut LineDraft,
        pending_space: &mut bool,
        lines: &mut Vec<CardLine>,
        cursor: &mut f32,
    ) {
        let (width, base_width, ruby_height, rise) = self.measure_piece(
            text,
            style,
            ruby.as_deref(),
            metrics.font_scale,
            scale_multiplier,
        );
        if draft.has_content() {
            let gap = if *pending_space {
                self.space_width(metrics)
            } else {
                0.0
            };
            if draft.x + gap + width > metrics.width {
                finish_line(draft, lines, cursor, metrics, true);
            }
        }
        let available = metrics.width - draft.x;
        if ruby.is_none() && width > available {
            if draft.has_content() {
                finish_line(draft, lines, cursor, metrics, true);
            }
            if width > metrics.width - draft.x {
                self.place_split_text(
                    text,
                    style,
                    scale_multiplier,
                    metrics,
                    draft,
                    pending_space,
                    lines,
                    cursor,
                    ruby_height,
                );
                return;
            }
        }
        let gap = if draft.has_content() && *pending_space {
            self.space_width(metrics)
        } else {
            0.0
        };
        draft.x += gap;
        draft.pieces.push(CardPiece {
            text: text.to_owned(),
            style: style.clone(),
            x: draft.x,
            width,
            base_width,
            ruby,
            rise,
            scale: scale_multiplier,
            pitch_marker: None,
        });
        draft.x += width;
        draft.ruby_height = draft.ruby_height.max(ruby_height);
        if rise < 0.0 {
            draft.upper = draft.upper.max(-rise);
        } else if rise > 0.0 {
            draft.lower = draft.lower.max(rise);
        }
        draft.max_scale = draft.max_scale.max(scale_multiplier);
        if style.pitch.high || style.pitch.downstep || style.pitch.upstep {
            draft.pitch_height = draft
                .pitch_height
                .max(CARD_PITCH_HEIGHT * metrics.font_scale);
        }
        *pending_space = false;
    }

    #[allow(clippy::too_many_arguments)]
    fn place_split_text(
        &self,
        text: &str,
        style: &Style,
        scale_multiplier: f32,
        metrics: CardMetrics,
        draft: &mut LineDraft,
        pending_space: &mut bool,
        lines: &mut Vec<CardLine>,
        cursor: &mut f32,
        ruby_height: f32,
    ) {
        let scale = CARD_TEXT_SCALE * metrics.font_scale;
        for ch in text.chars() {
            let piece_scale = scale_multiplier;
            let text_scale = scale * piece_scale * if style.sub || style.sup { 0.75 } else { 1.0 };
            let mut buffer = [0u8; 4];
            let piece = ch.encode_utf8(&mut buffer);
            let width = Ui::get_card_text_width(text_scale, piece);
            if draft.has_content() && draft.x + width > metrics.width {
                finish_line(draft, lines, cursor, metrics, true);
            }
            draft.pieces.push(CardPiece {
                text: piece.to_owned(),
                style: style.clone(),
                x: draft.x,
                width,
                base_width: width,
                ruby: None,
                rise: 0.0,
                scale: piece_scale,
                pitch_marker: None,
            });
            draft.x += width;
            draft.ruby_height = draft.ruby_height.max(ruby_height);
            draft.max_scale = draft.max_scale.max(piece_scale);
            if style.pitch.high || style.pitch.downstep || style.pitch.upstep {
                draft.pitch_height = draft
                    .pitch_height
                    .max(CARD_PITCH_HEIGHT * metrics.font_scale);
            }
        }
        *pending_space = false;
    }

    fn measure_piece(
        &self,
        text: &str,
        style: &Style,
        ruby: Option<&str>,
        font_scale: f32,
        scale_multiplier: f32,
    ) -> (f32, f32, f32, f32) {
        let scale = CARD_TEXT_SCALE * font_scale * scale_multiplier;
        let text_scale = scale * if style.sub || style.sup { 0.75 } else { 1.0 };
        let base_width = Ui::get_card_text_width(text_scale, text);
        let reading_width = ruby
            .map(|reading| Ui::get_card_text_width(scale * CARD_RUBY_SCALE, reading))
            .unwrap_or(0.0);
        let mut width = base_width.max(reading_width);
        if style.bold {
            width += 0.7 * font_scale * scale_multiplier;
        }
        let ruby_height = if ruby.is_some() {
            CARD_RUBY_HEIGHT * font_scale * scale_multiplier + 1.0
        } else {
            0.0
        };
        let rise = if style.sup {
            -5.0 * font_scale * scale_multiplier
        } else if style.sub {
            4.0 * font_scale * scale_multiplier
        } else {
            0.0
        };
        (width, base_width, ruby_height, rise)
    }

    fn space_width(&self, metrics: CardMetrics) -> f32 {
        let scale = CARD_TEXT_SCALE * metrics.font_scale;
        let width = Ui::get_card_text_width(scale, " ");
        if width <= 0.0 {
            4.0 * metrics.font_scale
        } else {
            width
        }
    }
}

fn finish_line(
    draft: &mut LineDraft,
    lines: &mut Vec<CardLine>,
    cursor: &mut f32,
    metrics: CardMetrics,
    force: bool,
) {
    if draft.pieces.is_empty() && draft.marker.is_none() && !force {
        return;
    }
    let scale = draft.max_scale.max(1.0);
    let height = metrics.nominal * scale
        + draft.ruby_height
        + draft.upper
        + draft.lower
        + draft.pitch_height;
    lines.push(CardLine {
        top: *cursor,
        height,
        ruby_height: draft.ruby_height,
        upper: draft.upper,
        width: draft.x.max(metrics.content_left),
        pitch_height: draft.pitch_height,
        marker: draft.marker.take(),
        pieces: std::mem::take(&mut draft.pieces),
        rule: false,
    });
    *cursor += height;
    draft.x = metrics.content_left;
    draft.ruby_height = 0.0;
    draft.upper = 0.0;
    draft.lower = 0.0;
    draft.max_scale = 1.0;
    draft.pitch_height = 0.0;
}

fn script_runs(text: &str) -> Vec<(usize, usize)> {
    let mut runs = Vec::new();
    let mut start = None;
    let mut cjk = None;
    for (index, ch) in text.char_indices() {
        let current = is_cjk_char(ch);
        match (start, cjk) {
            (None, _) => {
                start = Some(index);
                cjk = Some(current);
            }
            (Some(_), Some(previous)) if previous != current => {
                runs.push((start.unwrap(), index));
                start = Some(index);
                cjk = Some(current);
            }
            _ => {}
        }
    }
    if let Some(start) = start {
        runs.push((start, text.len()));
    }
    runs
}

fn script_multiplier(text: &str) -> f32 {
    if is_cjk_text(text) {
        CARD_CJK_SCALE
    } else {
        1.0
    }
}

fn is_cjk_text(text: &str) -> bool {
    text.chars().any(is_cjk_char)
}

fn is_cjk_char(ch: char) -> bool {
    matches!(
        ch as u32,
        0x3000..=0x303f
            | 0x3040..=0x30ff
            | 0x31f0..=0x31ff
            | 0x3400..=0x4dbf
            | 0x4e00..=0x9fff
            | 0xf900..=0xfaff
            | 0xff66..=0xff9f
            | 0x20000..=0x2fa1f
    )
}

fn is_small_kana(ch: char) -> bool {
    matches!(
        ch,
        'ぁ' | 'ぃ'
            | 'ぅ'
            | 'ぇ'
            | 'ぉ'
            | 'っ'
            | 'ゃ'
            | 'ゅ'
            | 'ょ'
            | 'ゎ'
            | 'ァ'
            | 'ィ'
            | 'ゥ'
            | 'ェ'
            | 'ォ'
            | 'ッ'
            | 'ャ'
            | 'ュ'
            | 'ョ'
            | 'ヮ'
    )
}

fn block_text(content: &[Inline]) -> String {
    let mut text = String::new();
    for inline in content {
        match inline {
            Inline::Text { text: value, .. } => text.push_str(value),
            Inline::Ruby { base, .. } | Inline::PitchWord { base, .. } => text.push_str(base),
            Inline::Marker(_) => {}
            Inline::Break => text.push(' '),
        }
    }
    text
}

fn headword_multiplier(content: &[Inline]) -> f32 {
    let text = block_text(content);
    let count = text.chars().filter(|ch| !ch.is_whitespace()).count();
    if (1..=CARD_HEADWORD_MAX_CHARS).contains(&count) {
        CARD_HEADWORD_SCALE
    } else {
        1.0
    }
}
