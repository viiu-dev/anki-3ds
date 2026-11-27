// Copyright: Anki 3DS contributors
// License: GNU AGPL, version 3 or later; http://www.gnu.org/licenses/agpl.html

mod anim;
mod sfx;
mod assets;
mod auth_store;
mod backend;
mod backup;
mod clock;
mod controls;
mod event_log;
mod i18n;
mod leds;
mod media_sync;
mod quickstart;
mod review_text;
mod stats;
mod sync;
mod theme;
mod time_correction;
mod ui;
use std::collections::HashSet;
use std::fs;
use std::time::Duration;
use std::time::Instant;

use anki_proto::decks::DeckTreeNode;
use backend::Backend;
use backend::DeckOptions;
use backend::LimitScope;
use backend::NoteDraft;
use backend::OfferedCard;
use controls::ReviewAction;
use controls::ScreenState;
use ctru::prelude::*;
use i18n::tr;
use theme::current as current_theme;
use theme::ThemeKind;
use ui::*;

#[cfg(feature = "emulator")]
const TEST_SCRIPT: &str = "sdmc:/3ds/anki-3ds/test-script.txt";
const ROW_Y: f32 = 43.0;
const ROW_H: f32 = 24.0;
const ROWS: usize = 6;

fn event(text: &str) {
    event_log::append(text);
}
fn error_text(error: &dyn std::error::Error) -> String {
    let mut text = error.to_string();
    let mut source = error.source();
    while let Some(error) = source {
        text.push_str(&format!(": {error}"));
        source = error.source();
    }
    text
}
pub(crate) fn media_diag(path: &str) -> String {
    let size = match fs::metadata(path) {
        Ok(meta) => meta.len(),
        Err(_) => return "missing".into(),
    };
    let mut head = [0u8; 8];
    let count = fs::File::open(path)
        .map(|mut file| {
            use std::io::Read;
            file.read(&mut head).unwrap_or(0)
        })
        .unwrap_or(0);
    if count == 0 {
        return format!("size={size} head=empty");
    }
    let mut hex = String::with_capacity(count * 2);
    for byte in &head[..count] {
        hex.push_str(&format!("{byte:02x}"));
    }
    format!("size={size} head={hex}")
}
pub(crate) fn log_media_once(key: &str, text: &str) {
    static LOGGED: std::sync::OnceLock<std::sync::Mutex<HashSet<String>>> =
        std::sync::OnceLock::new();
    let logged = LOGGED.get_or_init(|| std::sync::Mutex::new(HashSet::new()));
    let mut guard = logged
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    if guard.len() >= 128 && !guard.contains(key) {
        return;
    }
    if guard.insert(key.to_owned()) {
        event(text);
    }
}

pub struct DeckItem {
    pub id: i64,
    pub name: String,
    pub leaf_name: String,
    pub level: i32,
    pub has_children: bool,
    pub review_count: u32,
    pub learn_count: u32,
    pub new_count: u32,
}
fn collect_deck_items(node: &DeckTreeNode, collapsed: &HashSet<i64>, items: &mut Vec<DeckItem>) {
    if node.deck_id != 0 {
        items.push(DeckItem {
            id: node.deck_id,
            name: node.name.clone(),
            leaf_name: node
                .name
                .rsplit("::")
                .next()
                .unwrap_or(&node.name)
                .to_owned(),
            level: (node.level as i32 - 1).max(0),
            has_children: !node.children.is_empty(),
            review_count: node.review_count,
            learn_count: node.learn_count,
            new_count: node.new_count,
        });
    }
    if node.deck_id == 0 || !collapsed.contains(&node.deck_id) {
        for child in &node.children {
            collect_deck_items(child, collapsed, items);
        }
    }
}

struct Browser {
    query: String,
    filter: usize,
    ids: Vec<i64>,
    selected: usize,
    preview: (String, String),
    status: backend::CardStatus,
    titles: Vec<String>,
    page: usize,
}
impl Browser {
    fn load(backend: &mut Backend, query: String) -> backend::Result<Self> {
        let ids = backend.search(&query)?;
        let mut browser = Self {
            filter: Self::filter_of(&query),
            query,
            ids,
            selected: 0,
            preview: Default::default(),
            status: Default::default(),
            titles: Vec::new(),
            page: usize::MAX,
        };
        browser.refresh(backend)?;
        Ok(browser)
    }
    fn filter_of(query: &str) -> usize {
        if query == "is:buried" {
            2
        } else if query == "is:suspended" {
            3
        } else if query == "tag:leech" {
            4
        } else if query.starts_with("deck:") {
            1
        } else {
            0
        }
    }
    fn refresh(&mut self, backend: &mut Backend) -> backend::Result<()> {
        if self.ids.is_empty() {
            self.preview = (tr("No results").into(), String::new());
            self.status = Default::default();
            return Ok(());
        }
        self.selected = self.selected.min(self.ids.len() - 1);
        self.preview = backend.preview(self.ids[self.selected])?;
        self.status = backend.card_status(self.ids[self.selected])?;
        let page = self.selected / ROWS;
        if page != self.page {
            self.page = page;
            self.titles.clear();
            for id in self.ids.iter().skip(page * ROWS).take(ROWS) {
                self.titles.push(backend.preview(*id)?.0.replace('\n', " "));
            }
        }
        Ok(())
    }
}

const BROWSE_FILTERS: [&str; 5] = ["All", "Current deck", "Buried", "Suspended", "Leeches"];

fn browse_filter_query(index: usize, deck_name: &str) -> String {
    match index {
        1 => format!("deck:\"{}\"", deck_name.replace('"', "")),
        2 => "is:buried".into(),
        3 => "is:suspended".into(),
        4 => "tag:leech".into(),
        _ => String::new(),
    }
}

fn browse_status(status: &backend::CardStatus) -> String {
    let mut parts = Vec::new();
    if status.buried {
        parts.push(tr("Buried"));
    }
    if status.suspended {
        parts.push(tr("Suspended"));
    }
    if status.leech {
        parts.push(tr("Leech"));
    }
    if parts.is_empty() {
        tr("Active").to_owned()
    } else {
        parts.join(" · ")
    }
}

/// Per-card review extras: typed answer text, open hints, animated-media
/// selection/playback, and the cached reveal HTML. Reset whenever the
/// reviewed card id changes; kept across the question/answer transition of
/// the same card.
#[derive(Default)]
struct ReviewState {
    typed: Option<String>,
    hints_open: Vec<u16>,
    media_sel: usize,
    media_playing: bool,
    media_ready: bool,
    revealed: Option<RevealedHtml>,
}

struct RevealedHtml {
    card_id: i64,
    answer: String,
}

impl ReviewState {
    fn hints_set(&self) -> HashSet<u16> {
        self.hints_open.iter().copied().collect()
    }

    fn toggle_hint(&mut self, id: u16) {
        if let Some(index) = self.hints_open.iter().position(|hint| *hint == id) {
            self.hints_open.remove(index);
        } else {
            self.hints_open.push(id);
        }
    }
}

/// Next unopened hint, if any, for the L button.
fn next_closed_hint(top_card: &RichTextLayout, review: &ReviewState) -> Option<u16> {
    top_card
        .hint_ids()
        .into_iter()
        .find(|id| !review.hints_open.contains(id))
}

fn review_action_to_action(
    action: ReviewAction,
    screen: &Screen,
    top_card: &RichTextLayout,
    review: &ReviewState,
) -> Action {
    match action {
        ReviewAction::ShowAnswer => Action::Reveal,
        ReviewAction::Again => Action::Rate(0),
        ReviewAction::Hard => Action::Rate(1),
        ReviewAction::Good => Action::Rate(2),
        ReviewAction::Easy => Action::Rate(3),
        ReviewAction::PlayAudio => Action::PlayAudio(None),
        ReviewAction::Undo => Action::Undo,
        ReviewAction::Bury => Action::Bury,
        ReviewAction::Hint => {
            if let Some(id) = next_closed_hint(top_card, review) {
                Action::HintToggle(id)
            } else if matches!(screen, Screen::Question { card, .. } if card.type_answer.is_some())
            {
                Action::TypeAnswer
            } else {
                Action::PlayAudio(None)
            }
        }
        ReviewAction::ScrollUp | ReviewAction::ScrollDown => Action::None,
    }
}

fn browse_chip_rects() -> [(f32, f32); 5] {
    let labels = [
        tr("All"),
        tr("Current deck"),
        tr("Buried"),
        tr("Suspended"),
        tr("Leeches"),
    ];
    let widths: Vec<f32> = labels
        .iter()
        .map(|label| Ui::get_text_width(0.38, label) + 12.0)
        .collect();
    let total: f32 = widths.iter().sum::<f32>() + 3.0 * (widths.len() as f32 - 1.0);
    let mut x = ((320.0 - total) * 0.5).max(2.0);
    let mut rects = [(0.0f32, 0.0f32); 5];
    for (index, width) in widths.iter().enumerate() {
        rects[index] = (x, *width);
        x += width + 3.0;
    }
    rects
}

fn browse_chip(x: f32, y: f32, w: f32, h: f32, label: &str, active: bool) {
    let background = if active {
        current_theme().accent
    } else {
        current_theme().surface_alt
    };
    let foreground = if active {
        current_theme().on_accent
    } else {
        current_theme().text
    };
    Ui::draw_rounded_rect(x, y, w, h, h * 0.5, background);
    Ui::draw_text_centered(
        x + w * 0.5,
        y + 4.0,
        0.38,
        foreground,
        &Ui::ellipsize(label, 0.38, w - 6.0),
    );
}

const AUDIO_Y: f32 = 84.0;
const AUDIO_H: f32 = 32.0;

fn card_audio(card: &backend::OfferedCard, answer: bool) -> Vec<backend::MediaRef> {
    let media = if answer {
        &card.answer_media
    } else {
        &card.question_media
    };
    media.iter().filter(|item| item.audio).cloned().collect()
}

fn replay_audio(card: &backend::OfferedCard, answer: bool) -> Vec<backend::MediaRef> {
    let mut audio = if i18n::replay_all_audio() {
        let mut all = card_audio(card, false);
        all.extend(card_audio(card, true));
        all
    } else {
        card_audio(card, answer)
    };
    let mut seen = HashSet::new();
    audio.retain(|item| seen.insert(item.filename.clone()));
    audio
}

fn audio_path(backend: Option<&Backend>, item: &backend::MediaRef) -> String {
    backend
        .map(|active| active.media_path(&item.filename))
        .unwrap_or_else(|| item.path())
}

fn audio_button_rects(visible: usize, paged: bool) -> Vec<(f32, f32)> {
    if visible == 0 {
        return Vec::new();
    }
    let side = if paged { 20.0 } else { 8.0 };
    let gap = 6.0;
    let usable = 320.0 - 2.0 * side;
    let four_slot = (usable - (AUDIO_VISIBLE as f32 - 1.0) * gap) / AUDIO_VISIBLE as f32;
    let fit = (usable - (visible as f32 - 1.0) * gap) / visible as f32;
    let width = if paged { four_slot } else { fit };
    let row = visible as f32 * width + (visible as f32 - 1.0) * gap;
    let start = (320.0 - row) * 0.5;
    (0..visible)
        .map(|index| (start + index as f32 * (width + gap), width))
        .collect()
}

const AUDIO_VISIBLE: usize = 4;

fn audio_page_count(total: usize) -> usize {
    total.div_ceil(AUDIO_VISIBLE).max(1)
}

fn audio_page_offset(total: usize, page: usize) -> usize {
    page.min(audio_page_count(total) - 1) * AUDIO_VISIBLE
}

fn draw_audio_buttons(audio: &[backend::MediaRef], states: &[AudioState], phase: u64, page: usize) {
    let total = audio.len();
    let pages = audio_page_count(total);
    let page = page.min(pages - 1);
    let offset = audio_page_offset(total, page);
    let visible = total.saturating_sub(offset).min(AUDIO_VISIBLE);
    for (slot, (x, width)) in audio_button_rects(visible, pages > 1)
        .into_iter()
        .enumerate()
    {
        let index = offset + slot;
        let state = states.get(index).copied().unwrap_or(AudioState::Pending);
        let (background, foreground) = match state {
            AudioState::Playing => (current_theme().accent, current_theme().on_accent),
            AudioState::Ready => (current_theme().surface_alt, current_theme().text),
            AudioState::Pending => (current_theme().surface_alt, current_theme().text_muted),
        };
        Ui::draw_rounded_rect(x, AUDIO_Y, width, AUDIO_H, AUDIO_H * 0.5, background);
        let center_y = AUDIO_Y + AUDIO_H * 0.5;
        let label = i18n::format("Audio {0}", &[&(index + 1).to_string()]);
        let label_width = Ui::get_text_width(0.42, &label);
        let icon_width = if state == AudioState::Playing {
            22.0
        } else {
            14.0
        };
        let gap = 6.0;
        let group = icon_width + gap + label_width;
        let group_x = x + (width - group) * 0.5;
        let icon_x = group_x + icon_width * 0.5;
        match state {
            AudioState::Ready => {
                Ui::draw_triangle(
                    icon_x - 4.5,
                    center_y - 6.0,
                    icon_x - 4.5,
                    center_y + 6.0,
                    icon_x + 5.5,
                    center_y,
                    current_theme().accent,
                );
            }
            AudioState::Pending => {
                Ui::draw_rect(
                    icon_x - 1.5,
                    center_y - 7.0,
                    4.0,
                    7.0,
                    current_theme().text_muted,
                );
                Ui::draw_triangle(
                    icon_x - 5.0,
                    center_y,
                    icon_x + 5.0,
                    center_y,
                    icon_x,
                    center_y + 6.0,
                    current_theme().text_muted,
                );
            }
            AudioState::Playing => {
                Ui::draw_rect(icon_x - 7.0, center_y - 3.0, 4.0, 6.0, foreground);
                Ui::draw_triangle(
                    icon_x - 3.0,
                    center_y - 6.0,
                    icon_x - 3.0,
                    center_y + 6.0,
                    icon_x + 3.0,
                    center_y,
                    foreground,
                );
                for bar in 0..2 {
                    let height = 3.0 + (((phase + index as u64 * 2 + bar) % 8) as f32 - 4.0).abs();
                    Ui::draw_rect(
                        icon_x + 5.0 + bar as f32 * 4.0,
                        center_y - height * 0.5,
                        2.0,
                        height,
                        foreground,
                    );
                }
            }
        }
        Ui::draw_text(
            group_x + icon_width + gap,
            AUDIO_Y + 10.0,
            0.42,
            foreground,
            &label,
        );
    }
    if pages > 1 {
        let center_y = AUDIO_Y + AUDIO_H * 0.5;
        let prev = if page > 0 {
            current_theme().text
        } else {
            current_theme().text_faint
        };
        let next = if page + 1 < pages {
            current_theme().text
        } else {
            current_theme().text_faint
        };
        Ui::draw_triangle(
            7.0,
            center_y,
            15.0,
            center_y - 6.0,
            15.0,
            center_y + 6.0,
            prev,
        );
        Ui::draw_triangle(
            313.0,
            center_y,
            305.0,
            center_y - 6.0,
            305.0,
            center_y + 6.0,
            next,
        );
        let spacing = 7.0;
        let start = 160.0 - (pages as f32 - 1.0) * spacing * 0.5;
        for dot in 0..pages {
            let color = if dot == page {
                current_theme().accent
            } else {
                current_theme().border
            };
            Ui::draw_rect(start + dot as f32 * spacing - 1.5, 119.0, 3.0, 3.0, color);
        }
    }
}

fn with_alpha(color: u32, alpha: f32) -> u32 {
    let a = (alpha.clamp(0.0, 1.0) * 255.0) as u32;
    (color & 0x00ff_ffff) | (a << 24)
}

fn feedback_alpha(elapsed: f32) -> f32 {
    if elapsed < 0.10 {
        elapsed / 0.10
    } else if elapsed < 0.55 {
        1.0
    } else {
        (1.0 - (elapsed - 0.55) / 0.40).clamp(0.0, 1.0)
    }
}

fn rating_colors(rating: i32) -> (u32, u32) {
    match rating {
        0 => (current_theme().again_bg, current_theme().again_fg),
        1 => (current_theme().hard_bg, current_theme().hard_fg),
        2 => (current_theme().good_bg, current_theme().good_fg),
        _ => (current_theme().easy_bg, current_theme().easy_fg),
    }
}

fn draw_rating_icon(rating: i32, cx: f32, cy: f32, scale: f32, color: u32) {
    match rating {
        0 => {
            Ui::draw_line(
                cx - 6.0 * scale,
                cy - 6.0 * scale,
                cx + 6.0 * scale,
                cy + 6.0 * scale,
                2.5 * scale,
                color,
            );
            Ui::draw_line(
                cx - 6.0 * scale,
                cy + 6.0 * scale,
                cx + 6.0 * scale,
                cy - 6.0 * scale,
                2.5 * scale,
                color,
            );
        }
        1 => {
            Ui::draw_rect(
                cx - 8.0 * scale,
                cy - 1.5 * scale,
                16.0 * scale,
                3.0 * scale,
                color,
            );
        }
        2 => {
            Ui::draw_line(
                cx - 7.0 * scale,
                cy + 1.0 * scale,
                cx - 2.0 * scale,
                cy + 6.0 * scale,
                2.5 * scale,
                color,
            );
            Ui::draw_line(
                cx - 2.0 * scale,
                cy + 6.0 * scale,
                cx + 8.0 * scale,
                cy - 6.0 * scale,
                2.5 * scale,
                color,
            );
        }
        _ => {
            Ui::draw_line(
                cx - 9.0 * scale,
                cy + 0.5 * scale,
                cx - 5.0 * scale,
                cy + 5.0 * scale,
                2.2 * scale,
                color,
            );
            Ui::draw_line(
                cx - 5.0 * scale,
                cy + 5.0 * scale,
                cx + 1.0 * scale,
                cy - 4.0 * scale,
                2.2 * scale,
                color,
            );
            Ui::draw_line(
                cx - 1.0 * scale,
                cy + 0.5 * scale,
                cx + 3.0 * scale,
                cy + 5.0 * scale,
                2.2 * scale,
                color,
            );
            Ui::draw_line(
                cx + 3.0 * scale,
                cy + 5.0 * scale,
                cx + 9.0 * scale,
                cy - 4.0 * scale,
                2.2 * scale,
                color,
            );
        }
    }
}

fn draw_rating_badge(feedback: Option<(i32, Instant)>) {
    if !i18n::rating_icon() {
        return;
    }
    let Some((rating, at)) = feedback else {
        return;
    };
    let elapsed = at.elapsed().as_secs_f32();
    if elapsed >= 0.95 {
        return;
    }
    let alpha = feedback_alpha(elapsed);
    let (_, foreground) = rating_colors(rating);
    draw_rating_icon(
        rating,
        TOP_WIDTH - 20.0,
        18.0,
        1.0,
        with_alpha(foreground, alpha),
    );
}

fn draw_sync_button(x: f32, y: f32, w: f32, h: f32, state: i32) {
    let label = tr("Sync");
    let text_width = Ui::get_text_width(0.50, label);
    let icon = 10.0;
    let gap = 7.0;
    let group = icon + gap + text_width;
    let start = x + (w - group) * 0.5;
    let text_y = y + h * 0.5 - 6.0;
    Ui::draw_button(x, y, w, h, "", None, 6, false);
    Ui::draw_sync_icon(start, text_y + 2.0, icon, state);
    Ui::draw_text(
        start + icon + gap,
        text_y,
        0.50,
        current_theme().text,
        label,
    );
}

fn draw_undo_button(x: f32, y: f32, w: f32, h: f32, enabled: bool) {
    let style = if enabled { 6 } else { 8 };
    let color = if enabled {
        current_theme().text
    } else {
        current_theme().text_faint
    };
    let size = if h >= 48.0 { 20.0 } else { 18.0 };
    Ui::draw_button(x, y, w, h, "", None, style, false);
    Ui::draw_icon_undo(x + (w - size) * 0.5, y + (h - size) * 0.5, size, color);
}

fn draw_leech_notice(notice: Option<&(String, Instant)>) {
    let Some((text, at)) = notice else {
        return;
    };
    let elapsed = at.elapsed().as_secs_f32();
    if elapsed >= 2.8 {
        return;
    }
    let alpha = if elapsed < 0.15 {
        elapsed / 0.15
    } else if elapsed < 2.3 {
        1.0
    } else {
        (1.0 - (elapsed - 2.3) / 0.50).clamp(0.0, 1.0)
    };
    let theme = current_theme();
    let width = TOP_WIDTH - 96.0;
    let height = 32.0;
    let x = (TOP_WIDTH - width) * 0.5;
    let y = 196.0;
    let radius = height * 0.5;
    Ui::draw_rounded_rect(
        x,
        y + 1.5,
        width,
        height,
        radius,
        with_alpha(theme.shadow, alpha),
    );
    Ui::draw_rounded_rect(x, y, width, height, radius, with_alpha(theme.border, alpha));
    Ui::draw_rounded_rect(
        x + 1.0,
        y + 1.0,
        width - 2.0,
        height - 2.0,
        radius - 1.0,
        with_alpha(theme.surface_alt, alpha),
    );
    Ui::draw_rect(
        x + 14.0,
        y + height * 0.5 - 2.0,
        4.0,
        4.0,
        with_alpha(theme.hard_fg, alpha),
    );
    Ui::fit_text(
        TOP_WIDTH / 2.0 + 5.0,
        y + 9.0,
        0.46,
        width,
        with_alpha(theme.text, alpha),
        text,
    );
}

fn leech_message(event: backend::LeechEvent) -> String {
    match event {
        backend::LeechEvent::Suspended => tr("Leech: card suspended").to_owned(),
        backend::LeechEvent::Tagged => tr("Leech: card tagged").to_owned(),
    }
}

#[derive(Clone, Copy, PartialEq)]
enum AudioState {
    Pending,
    Ready,
    Playing,
}

fn audio_states(
    audio: &[backend::MediaRef],
    backend: Option<&Backend>,
    playing: Option<&str>,
) -> Vec<AudioState> {
    audio
        .iter()
        .map(|item| {
            if Some(item.filename.as_str()) == playing {
                AudioState::Playing
            } else if backend.is_some_and(|active| active.media_exists(&item.filename)) {
                AudioState::Ready
            } else {
                AudioState::Pending
            }
        })
        .collect()
}

enum PrefetchFile {
    Image(String),
    Audio(String),
}

fn prefetch_for_card(backend: &Backend, card: &backend::OfferedCard) -> Vec<PrefetchFile> {
    let mut files = Vec::new();
    for item in card
        .question_media
        .iter()
        .chain(card.answer_media.iter())
        .filter(|item| item.audio)
    {
        if backend.media_exists(&item.filename) {
            files.push(PrefetchFile::Audio(backend.media_path(&item.filename)));
        }
    }
    for item in card
        .answer_media
        .iter()
        .chain(card.question_media.iter())
        .filter(|item| item.kind == backend::MediaKind::Image)
    {
        if backend.media_exists(&item.filename) {
            files.push(PrefetchFile::Image(backend.media_path(&item.filename)));
        }
    }
    files
}

struct PrefetchQueue {
    generation: u32,
    files: Vec<PrefetchFile>,
    pending: bool,
    shutdown: bool,
}

#[derive(Default)]
struct Prefetch {
    thread: Option<std::thread::JoinHandle<()>>,
    state: Option<std::sync::Arc<(std::sync::Mutex<PrefetchQueue>, std::sync::Condvar)>>,
    generation: std::sync::Arc<std::sync::atomic::AtomicU32>,
}

impl Prefetch {
    fn ensure_worker(&mut self) {
        if self.thread.is_some() {
            return;
        }
        let shared = std::sync::Arc::new((
            std::sync::Mutex::new(PrefetchQueue {
                generation: 0,
                files: Vec::new(),
                pending: false,
                shutdown: false,
            }),
            std::sync::Condvar::new(),
        ));
        let worker_state = shared.clone();
        let worker_gen = self.generation.clone();
        let handle = std::thread::Builder::new()
            .name("prefetch".into())
            .stack_size(256 * 1024)
            .spawn(move || loop {
                let (gen_id, files) = {
                    let (lock, cv) = &*worker_state;
                    let mut guard = lock
                        .lock()
                        .unwrap_or_else(|poisoned| poisoned.into_inner());
                    while !guard.pending && !guard.shutdown {
                        guard = cv
                            .wait(guard)
                            .unwrap_or_else(|poisoned| poisoned.into_inner());
                    }
                    if guard.shutdown {
                        break;
                    }
                    guard.pending = false;
                    (guard.generation, std::mem::take(&mut guard.files))
                };
                Ui::media_prefetch_clear();
                Ui::audio_prefetch_clear();
                for file in files {
                    if worker_gen.load(std::sync::atomic::Ordering::Acquire) != gen_id {
                        break;
                    }
                    match file {
                        PrefetchFile::Image(path) => {
                            Ui::media_prefetch(&path);
                        }
                        PrefetchFile::Audio(path) => {
                            Ui::audio_prefetch(&path);
                        }
                    }
                }
            })
            .ok();
        if handle.is_some() {
            self.state = Some(shared);
            self.thread = handle;
        }
    }

    fn stop(&mut self) {
        self.generation
            .fetch_add(1, std::sync::atomic::Ordering::AcqRel);
        if let Some(shared) = self.state.take() {
            let (lock, cv) = &*shared;
            let mut guard = lock
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            guard.shutdown = true;
            guard.files.clear();
            cv.notify_one();
        }
        if let Some(handle) = self.thread.take() {
            let _ = handle.join();
        }
        Ui::media_prefetch_clear();
        Ui::audio_prefetch_clear();
    }

    fn request(&mut self, files: Vec<PrefetchFile>) {
        if files.is_empty() && self.thread.is_none() {
            Ui::media_prefetch_clear();
            Ui::audio_prefetch_clear();
            return;
        }
        self.ensure_worker();
        let next = self
            .generation
            .fetch_add(1, std::sync::atomic::Ordering::AcqRel)
            .wrapping_add(1);
        if let Some(shared) = &self.state {
            let (lock, cv) = &**shared;
            let mut guard = lock
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            guard.generation = next;
            guard.files = files;
            guard.pending = true;
            cv.notify_one();
        }
    }
}

const MEDIA_STATUS_Y: f32 = 28.0;

fn draw_hint_chips(top_card: &RichTextLayout, review: &ReviewState, y: f32) {
    let ids = top_card.hint_ids();
    if ids.is_empty() {
        return;
    }
    for (index, (x, width)) in hint_chip_rects(&ids, review).into_iter().enumerate() {
        let id = ids[index];
        let open = review.hints_open.contains(&id);
        Ui::draw_button(
            x,
            y,
            width,
            18.0,
            &hint_chip_label(index, open),
            None,
            if open { 0 } else { 6 },
            false,
        );
    }
}

fn draw_media_bar(top_card: &RichTextLayout, review: &ReviewState, answer_side: bool) {
    let items = top_card.media_items();
    if items.is_empty() {
        return;
    }
    let selected = review.media_sel.min(items.len() - 1);
    let playing = review.media_playing
        && items
            .get(selected)
            .is_some_and(|anim| anim.supported && Ui::anim_is_playing(anim.id));
    let finished = items
        .get(selected)
        .is_some_and(|anim| anim.supported && Ui::anim_finished(anim.id));
    let (y, replay, play, sel) = if answer_side {
        (150.0, (8.0, 68.0), (80.0, 68.0), (152.0, 76.0))
    } else {
        (206.0, (8.0, 70.0), (82.0, 70.0), (156.0, 100.0))
    };
    Ui::draw_button(replay.0, y, replay.1, 24.0, tr("Replay"), None, 6, false);
    Ui::draw_button(
        play.0,
        y,
        play.1,
        24.0,
        if playing { tr("Pause") } else { tr("Play") },
        None,
        6,
        false,
    );
    let show_selector = items.len() > 1;
    if show_selector {
        let label = format!("< {}/{} >", selected + 1, items.len());
        Ui::draw_button(sel.0, y, sel.1, 24.0, &label, None, 6, false);
    }
    let bar_right = if show_selector {
        sel.0 + sel.1
    } else if answer_side {
        228.0
    } else {
        312.0
    };
    let bar_w = bar_right - 8.0;
    Ui::draw_rect(8.0, y + 26.0, bar_w, 3.0, current_theme().surface_alt);
    let (fraction, state) = if finished {
        (1.0, current_theme().text_faint)
    } else if playing {
        (1.0, current_theme().review)
    } else {
        (0.33, current_theme().accent)
    };
    Ui::draw_rect(8.0, y + 26.0, bar_w * fraction, 3.0, state);
}

fn draw_media_strip(status: Option<&str>, pending: bool) {
    if pending {
        Ui::draw_text_centered(
            160.0,
            MEDIA_STATUS_Y,
            0.36,
            current_theme().text_muted,
            tr("Downloading media..."),
        );
    } else if let Some(status) = status {
        let text = i18n::format("Media sync: {0}", &[status]);
        Ui::fit_text(
            160.0,
            MEDIA_STATUS_Y,
            0.36,
            300.0,
            current_theme().text_muted,
            &text,
        );
    }
}

fn audio_tap(audio: &[backend::MediaRef], page: usize, x: f32) -> Action {
    let total = audio.len();
    let pages = audio_page_count(total);
    if pages > 1 {
        if x < 20.0 {
            return Action::AudioPage(-1);
        }
        if x >= 300.0 {
            return Action::AudioPage(1);
        }
    }
    let offset = audio_page_offset(total, page);
    let visible = total.saturating_sub(offset).min(AUDIO_VISIBLE);
    for (slot, (bx, bw)) in audio_button_rects(visible, pages > 1)
        .into_iter()
        .enumerate()
    {
        if x >= bx && x < bx + bw {
            return Action::PlayAudio(Some(offset + slot));
        }
    }
    Action::None
}

fn hint_chip_label(index: usize, open: bool) -> String {
    let key = if open {
        "Hide Hint {0}"
    } else {
        "Show Hint {0}"
    };
    i18n::format(key, &[&(index + 1).to_string()])
}

/// Centered row of hint chips for the bottom screen.
fn hint_chip_rects(ids: &[u16], review: &ReviewState) -> Vec<(f32, f32)> {
    let widths: Vec<f32> = ids
        .iter()
        .enumerate()
        .map(|(index, id)| {
            Ui::get_text_width(
                0.38,
                &hint_chip_label(index, review.hints_open.contains(id)),
            ) + 16.0
        })
        .collect();
    let total: f32 = widths.iter().sum::<f32>() + 4.0 * (widths.len().saturating_sub(1) as f32);
    let mut x = ((320.0 - total) * 0.5).max(2.0);
    let mut rects = Vec::with_capacity(widths.len());
    for width in widths {
        rects.push((x, width));
        x += width + 4.0;
    }
    rects
}

fn hint_chip_tap(top_card: &RichTextLayout, review: &ReviewState, x: f32) -> Action {
    let ids = top_card.hint_ids();
    for (index, (cx, cw)) in hint_chip_rects(&ids, review).into_iter().enumerate() {
        if x >= cx && x < cx + cw {
            if let Some(id) = ids.get(index) {
                return Action::HintToggle(*id);
            }
        }
    }
    Action::None
}

fn media_bar_tap(top_card: &RichTextLayout, x: f32, answer_side: bool) -> Action {
    let items = top_card.media_items();
    if items.is_empty() {
        return Action::None;
    }
    // Must match draw_media_bar geometry.
    let (replay, play, sel) = if answer_side {
        ((8.0, 68.0), (80.0, 68.0), (152.0, 76.0))
    } else {
        ((8.0, 70.0), (82.0, 70.0), (156.0, 100.0))
    };
    if x >= replay.0 && x < replay.0 + replay.1 {
        Action::MediaReplay
    } else if x >= play.0 && x < play.0 + play.1 {
        Action::MediaToggle
    } else if items.len() > 1 && x >= sel.0 && x < sel.0 + sel.1 {
        Action::MediaSelect(if x < sel.0 + sel.1 / 2.0 { -1 } else { 1 })
    } else {
        Action::None
    }
}

fn browse_row(y: f32, title: &str, selected: bool) {
    Ui::draw_rounded_rect(
        6.0,
        y,
        296.0,
        21.0,
        7.0,
        if selected {
            current_theme().row_selected
        } else {
            current_theme().surface
        },
    );
    Ui::draw_text(
        14.0,
        y + 3.0,
        0.46,
        current_theme().text,
        &Ui::ellipsize(title, 0.46, 284.0),
    );
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum OptField {
    Section,
    LimitScope,
    NewLimit,
    ReviewLimit,
    LearnSteps,
    InsertOrder,
    RelearnSteps,
    LeechThreshold,
    LeechAction,
    Fsrs,
    Retention,
    NewMix,
    BuryNew,
    BuryReviews,
}

const OPTION_ROWS: usize = 18;

fn format_steps(steps: &[f32]) -> String {
    steps
        .iter()
        .map(|value| {
            if *value >= 1440.0 && (value / 1440.0).fract() == 0.0 {
                format!("{}d", (value / 1440.0) as i64)
            } else if *value >= 60.0 && (value / 60.0).fract() == 0.0 {
                format!("{}h", (value / 60.0) as i64)
            } else if value.fract() == 0.0 {
                format!("{}m", *value as i64)
            } else {
                format!("{value}m")
            }
        })
        .collect::<Vec<_>>()
        .join(" ")
}

fn parse_steps(text: &str) -> Option<Vec<f32>> {
    let mut steps = Vec::new();
    for token in text.split_whitespace() {
        if steps.len() >= 20 {
            return None;
        }
        let (number, multiplier) = if let Some(rest) = token.strip_suffix('d') {
            (rest, 1440.0)
        } else if let Some(rest) = token.strip_suffix('h') {
            (rest, 60.0)
        } else if let Some(rest) = token.strip_suffix('m') {
            (rest, 1.0)
        } else if let Some(rest) = token.strip_suffix('s') {
            (rest, 1.0 / 60.0)
        } else {
            (token, 1.0)
        };
        let value: f32 = number.parse().ok()?;
        if !(0.0..=100000.0).contains(&value) {
            return None;
        }
        let step = value * multiplier;
        if !step.is_finite() || step <= 0.0 || step > 365.0 * 1440.0 {
            return None;
        }
        steps.push(step);
    }
    if steps.is_empty() {
        None
    } else {
        Some(steps)
    }
}

fn parse_limited_u32<'a>(
    text: &str,
    min: u32,
    max: u32,
    message: &'a str,
) -> std::result::Result<u32, Box<dyn std::error::Error + 'a>> {
    let trimmed = text.trim();
    if trimmed.is_empty() {
        return Err(message.into());
    }
    let value: u32 = trimmed.parse().map_err(|_| message)?;
    Ok(value.clamp(min, max))
}

fn on_off(value: bool) -> String {
    tr(if value { "On" } else { "Off" }).to_owned()
}

fn insert_order_label(value: i32) -> String {
    tr(if value == 1 {
        "Random"
    } else {
        "Sequential (oldest first)"
    })
    .to_owned()
}

fn leech_action_label(value: i32) -> String {
    tr(if value == 1 {
        "Tag only"
    } else {
        "Suspend card"
    })
    .to_owned()
}

fn new_mix_label(value: i32) -> String {
    tr(match value {
        1 => "Show after reviews",
        2 => "Show before reviews",
        _ => "Mix with reviews",
    })
    .to_owned()
}

fn limit_value(options: &DeckOptions, value: u32, preset: u32) -> String {
    if options.limit_scope == LimitScope::Preset {
        value.to_string()
    } else {
        format!("{value} ({}: {preset})", tr("Preset"))
    }
}

fn option_rows(options: &DeckOptions) -> Vec<(String, String, OptField)> {
    vec![
        (tr("Daily limits").into(), String::new(), OptField::Section),
        (
            tr("Limits apply to").into(),
            tr(match options.limit_scope {
                LimitScope::Preset => "Preset",
                LimitScope::ThisDeck => "This deck",
                LimitScope::TodayOnly => "Today only",
            })
            .to_owned(),
            OptField::LimitScope,
        ),
        (
            tr("New cards/day").into(),
            limit_value(options, options.new, options.preset_new),
            OptField::NewLimit,
        ),
        (
            tr("Maximum reviews/day").into(),
            limit_value(options, options.reviews, options.preset_reviews),
            OptField::ReviewLimit,
        ),
        (tr("New cards").into(), String::new(), OptField::Section),
        (
            tr("Learning steps").into(),
            format_steps(&options.learn_steps),
            OptField::LearnSteps,
        ),
        (
            tr("Insertion order").into(),
            insert_order_label(options.insert_order),
            OptField::InsertOrder,
        ),
        (
            tr("Lapses and leeches").into(),
            String::new(),
            OptField::Section,
        ),
        (
            tr("Relearning steps").into(),
            format_steps(&options.relearn_steps),
            OptField::RelearnSteps,
        ),
        (
            tr("Leech threshold").into(),
            options.leech_threshold.to_string(),
            OptField::LeechThreshold,
        ),
        (
            tr("Leech action").into(),
            leech_action_label(options.leech_action),
            OptField::LeechAction,
        ),
        (tr("FSRS").into(), String::new(), OptField::Section),
        (
            tr("Enable FSRS").into(),
            on_off(options.fsrs),
            OptField::Fsrs,
        ),
        (
            tr("Desired retention").into(),
            format!("{}%", (options.desired_retention * 100.0).round() as i32),
            OptField::Retention,
        ),
        (
            tr("Display order and burying").into(),
            String::new(),
            OptField::Section,
        ),
        (
            tr("New/review order").into(),
            new_mix_label(options.new_mix),
            OptField::NewMix,
        ),
        (
            tr("Bury new siblings").into(),
            on_off(options.bury_new),
            OptField::BuryNew,
        ),
        (
            tr("Bury review siblings").into(),
            on_off(options.bury_reviews),
            OptField::BuryReviews,
        ),
    ]
}

fn option_row(y: f32, label: &str, value: &str, section: bool, selected: bool) {
    let background = if section {
        current_theme().surface_alt
    } else if selected {
        current_theme().row_selected
    } else {
        current_theme().surface
    };
    Ui::draw_rounded_rect(6.0, y + 1.0, 296.0, 22.0, 8.0, background);
    let color = if section {
        current_theme().text_muted
    } else {
        current_theme().text
    };
    Ui::draw_text(
        16.0,
        y + 4.0,
        0.48,
        color,
        &Ui::ellipsize(label, 0.48, 168.0),
    );
    if !value.is_empty() {
        Ui::draw_text_right(
            298.0,
            y + 4.0,
            0.46,
            current_theme().text_muted,
            &Ui::ellipsize(value, 0.46, 120.0),
        );
    }
}

enum Screen {
    Decks,
    SyncWait,
    SyncChoice {
        upload_ok: bool,
        download_ok: bool,
        download: bool,
        confirmed: bool,
    },
    Question {
        card: OfferedCard,
        deck_name: String,
    },
    Answer {
        card: OfferedCard,
        deck_name: String,
    },
    Empty {
        deck_name: String,
    },
    Browser(Browser),
    Editor {
        draft: NoteDraft,
        field: usize,
        return_query: Option<String>,
    },
    Options {
        options: DeckOptions,
        field: usize,
    },
    Menu {
        selected: usize,
        previous: Box<Screen>,
    },
    Settings {
        selected: usize,
        previous: Box<Screen>,
    },
    SettingsCategory {
        category: SettingsCategory,
        selected: usize,
        previous: Box<Screen>,
    },
    SettingsChoice {
        kind: ChoiceKind,
        selected: usize,
        previous: Box<Screen>,
    },
    SettingsButton {
        action: ReviewAction,
        selected: usize,
        previous: Box<Screen>,
    },
    SettingsToolbar {
        selected: usize,
        previous: Box<Screen>,
    },
    SettingsAnimations {
        selected: usize,
        previous: Box<Screen>,
    },
    SettingsBackups {
        selected: usize,
        backups: Vec<backup::Entry>,
        status: Option<String>,
        previous: Box<Screen>,
    },
    BackupConfirm {
        entry: backup::Entry,
        previous: Box<Screen>,
    },
    CardChoice {
        card_id: i64,
        selected: usize,
        previous: Box<Screen>,
    },
    Stats {
        data: Box<stats::Dashboard>,
        page: stats::Page,
        timeframe: stats::Timeframe,
        previous: Box<Screen>,
    },
    Quickstart {
        step: usize,
        selected: usize,
        previous: Box<Screen>,
    },
    Info {
        title: String,
        text: String,
        previous: Box<Screen>,
    },
    Error(String),
}

fn screen_is_startup_or_decks(screen: &Screen) -> bool {
    matches!(screen, Screen::Decks)
}
#[derive(Clone, Copy)]
enum Action {
    None,
    Up,
    Down,
    Left,
    Right,
    Primary,
    Back,
    Reveal,
    Rate(i32),
    Add,
    Edit,
    Browse,
    Search,
    Field,
    Save,
    Options,
    Menu,
    Settings,
    Quickstart,
    Help,
    Stats,
    Sync,
    FullSync,
    Login,
    Logout,
    Clock,
    Language,
    Undo,
    Bury,
    Filter(usize),
    Restore,
    PlayAudio(Option<usize>),
    AudioPage(i32),
    TypeAnswer,
    HintToggle(u16),
    MediaToggle,
    MediaReplay,
    MediaSelect(i32),
    MediaSync,
    Exit,
}

fn wifi_disabled_message() -> String {
    tr("Wi-Fi is disabled or disconnected. Turn on wireless communication and connect to a network to sync.").to_owned()
}

fn user_error(error: &(dyn std::error::Error + 'static)) -> String {
    if let Some(anki::error::AnkiError::NetworkError { source }) =
        error.downcast_ref::<anki::error::AnkiError>()
    {
        if source.info.contains("Wi-Fi is disabled")
            || (source.kind == anki::error::NetworkErrorKind::Offline && !clock::connected_now())
        {
            return wifi_disabled_message();
        }
    }
    if let Some(error) = error.downcast_ref::<anki::error::AnkiError>() {
        error.message(&anki::prelude::I18n::new(&[i18n::language()]))
    } else {
        let text = error_text(error);
        if text.contains("Wi-Fi is disabled") || (!clock::connected_now() && text.contains("DNS failed")) {
            wifi_disabled_message()
        } else {
            text
        }
    }
}

fn wrap_text(text: &str, width: f32, scale: f32) -> Vec<String> {
    let mut lines = Vec::new();
    for paragraph in text.split('\n') {
        let mut line = String::new();
        for word in paragraph.split_whitespace() {
            let candidate = if line.is_empty() {
                word.to_owned()
            } else {
                format!("{line} {word}")
            };
            if Ui::get_text_width(scale, &review_text::plain(&candidate)) <= width {
                line = candidate;
                continue;
            }
            if !line.is_empty() {
                lines.push(std::mem::take(&mut line));
            }
            for ch in word.chars() {
                let mut candidate = line.clone();
                candidate.push(ch);
                if Ui::get_text_width(scale, &review_text::plain(&candidate)) > width
                    && !line.is_empty()
                {
                    lines.push(std::mem::take(&mut line));
                }
                line.push(ch);
            }
        }
        lines.push(line);
    }
    lines
}
#[derive(Default)]
struct TextLayout {
    text: String,
    width: f32,
    lines: Vec<String>,
    emphasis: Vec<usize>,
}
impl TextLayout {
    fn draw(
        &mut self,
        text: &str,
        width: f32,
        center: f32,
        y: f32,
        count: usize,
        scroll: usize,
    ) -> usize {
        if self.text != text || self.width != width || self.lines.is_empty() {
            self.lines = wrap_text(text, width, 0.55);
            let mut depth = 0usize;
            self.emphasis = self
                .lines
                .iter()
                .map(|line| {
                    let initial = depth;
                    for ch in line.chars() {
                        if ch == review_text::CLOZE_START {
                            depth += 1;
                        } else if ch == review_text::CLOZE_END {
                            depth = depth.saturating_sub(1);
                        }
                    }
                    initial
                })
                .collect();
            self.text = text.to_owned();
            self.width = width;
        }
        let max = self.lines.len().saturating_sub(count);
        let start = scroll.min(max);
        for (index, line) in self.lines.iter().enumerate().skip(start).take(count) {
            let mut emphasis = self.emphasis[index];
            let mut x = center - Ui::get_text_width(0.55, &review_text::plain(line)) / 2.0;
            let mut run = String::new();
            for ch in line.chars().chain(std::iter::once('\0')) {
                if matches!(ch, review_text::CLOZE_START | review_text::CLOZE_END | '\0') {
                    if index >= start {
                        let color = if emphasis > 0 {
                            current_theme().new
                        } else {
                            current_theme().text
                        };
                        Ui::draw_text(x, y + (index - start) as f32 * 19.0, 0.55, color, &run);
                    }
                    x += Ui::get_text_width(0.55, &run);
                    run.clear();
                    if ch == review_text::CLOZE_START {
                        emphasis += 1;
                    } else if ch == review_text::CLOZE_END {
                        emphasis = emphasis.saturating_sub(1);
                    }
                } else {
                    run.push(ch);
                }
            }
        }
        if max > 0 {
            Ui::draw_text_right(
                center + width / 2.0,
                y + count as f32 * 19.0,
                0.40,
                current_theme().text_muted,
                &format!("{} / {}  UP/DOWN", start + 1, max + 1),
            );
        }
        max
    }
}
fn header(width: f32, title: &str) {
    Ui::draw_rect(0.0, 0.0, width, 42.0, current_theme().surface_alt);
    Ui::draw_line(0.0, 42.0, width, 42.0, 1.0, current_theme().border);
    Ui::fit_text(
        width / 2.0,
        12.0,
        0.60,
        width - 24.0,
        current_theme().text,
        title,
    );
}
fn footer(text: &str) {
    Ui::draw_rect(0.0, 224.0, BOTTOM_WIDTH, 16.0, current_theme().surface_alt);
    Ui::fit_text(160.0, 226.0, 0.40, 304.0, current_theme().text_muted, text);
}
fn counts(new: u32, learn: u32, review: u32, y: f32) {
    for (x, label, value, color) in [
        (75.0, tr("New"), new, current_theme().new),
        (200.0, tr("Learning"), learn, current_theme().learn),
        (325.0, tr("Review"), review, current_theme().review),
    ] {
        Ui::draw_text_centered(x, y, 0.48, color, &value.to_string());
        Ui::draw_text_centered(x, y + 17.0, 0.43, current_theme().text_muted, label);
    }
}
fn counts_compact(new: u32, learn: u32, review: u32, y: f32) {
    for (x, label, value, color) in [
        (70.0, tr("New"), new, current_theme().new),
        (160.0, tr("Learning"), learn, current_theme().learn),
        (250.0, tr("Review"), review, current_theme().review),
    ] {
        Ui::draw_text_centered(x, y, 0.54, color, &value.to_string());
        Ui::draw_text_centered(x, y + 17.0, 0.37, current_theme().text_muted, label);
    }
}
fn draw_review_header(deck_name: &str, sync_state: i32) {
    let (items, count) = i18n::active_toolbar_items();
    let status_width = Ui::draw_status_compact(314.0, 3.0, 0.42, sync_state, &items[..count]);
    let max_deck_width = if status_width > 0.0 {
        (314.0 - status_width - 16.0).max(80.0)
    } else {
        304.0
    };
    if i18n::deck_title_visible() {
        Ui::draw_text(
            8.0,
            3.0,
            0.44,
            current_theme().text_muted,
            &Ui::ellipsize(deck_name, 0.44, max_deck_width),
        );
    }
}

fn button(x: f32, y: f32, w: f32, label: &str, hint: Option<&str>, style: i32) {
    Ui::draw_button(
        x,
        y,
        w,
        if hint.is_some() { 36.0 } else { 28.0 },
        label,
        hint,
        style,
        false,
    );
}
fn row(y: f32, label: &str, selected: bool) {
    Ui::draw_rounded_rect(
        6.0,
        y + 1.0,
        296.0,
        22.0,
        8.0,
        if selected {
            current_theme().row_selected
        } else {
            current_theme().surface
        },
    );
    Ui::draw_text(
        16.0,
        y + 4.0,
        0.52,
        current_theme().text,
        &Ui::ellipsize(label, 0.52, 276.0),
    );
}
const MAIN_MENU: [(Action, &str); 10] = [
    (Action::Undo, "Undo"),
    (Action::Bury, "Bury / Suspend"),
    (Action::Edit, "Edit note"),
    (Action::Add, "Add note"),
    (Action::Browse, "Browse"),
    (Action::Sync, "Sync"),
    (Action::Stats, "Statistics"),
    (Action::Options, "Deck options"),
    (Action::Settings, "Settings"),
    (Action::Exit, "Exit"),
];
#[derive(Clone, Copy, PartialEq, Eq)]
enum SettingsCategory {
    Appearance,
    Audio,
    Controls,
    Network,
    Backups,
    System,
    Language,
}

impl SettingsCategory {
    const ALL: [SettingsCategory; 7] = [
        SettingsCategory::Appearance,
        SettingsCategory::Audio,
        SettingsCategory::Controls,
        SettingsCategory::Network,
        SettingsCategory::Backups,
        SettingsCategory::System,
        SettingsCategory::Language,
    ];

    fn label(self) -> &'static str {
        match self {
            SettingsCategory::Appearance => "Appearance and display",
            SettingsCategory::Audio => "Audio and review",
            SettingsCategory::Controls => "Controls and keybinds",
            SettingsCategory::Network => "Sync and network",
            SettingsCategory::Backups => "Backups",
            SettingsCategory::System => "System and hardware",
            SettingsCategory::Language => "Language",
        }
    }

    fn hint(self) -> &'static str {
        match self {
            SettingsCategory::Appearance => "Theme, spacing and counts.",
            SettingsCategory::Audio => "Audio playback and review options.",
            SettingsCategory::Controls => "Scroll speed, review buttons and defaults.",
            SettingsCategory::Network => "Account, clock and media downloads.",
            SettingsCategory::Backups => "Create or restore collection backups.",
            SettingsCategory::System => "Hardware lights and help.",
            SettingsCategory::Language => "Choose the interface language.",
        }
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum ChoiceKind {
    Language,
    Theme,
    Accent,
    FontScale,
    Spacing,
    ScrollSpeed,
    Battery,
    Connections,
    AudioSpeed,
    ReplayMode,
    SfxVolume,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum ToggleKind {
    Autoplay,
    AutoplayVideo,
    ConsoleLeds,
    RatingIcon,
    CardCounts,
    RatingIntervals,
    UiSfx,
}

#[derive(Clone, Copy)]
enum SettingsItem {
    Choice(ChoiceKind),
    Toggle(ToggleKind),
    Binding(ReviewAction),
    ResetControls,
    Action(Action, &'static str),
    Toolbar,
    Animations,
}

const APPEARANCE_ITEMS: [SettingsItem; 9] = [
    SettingsItem::Choice(ChoiceKind::Theme),
    SettingsItem::Choice(ChoiceKind::Accent),
    SettingsItem::Choice(ChoiceKind::FontScale),
    SettingsItem::Choice(ChoiceKind::Spacing),
    SettingsItem::Choice(ChoiceKind::Battery),
    SettingsItem::Toggle(ToggleKind::CardCounts),
    SettingsItem::Toggle(ToggleKind::RatingIntervals),
    SettingsItem::Toolbar,
    SettingsItem::Animations,
];
const AUDIO_ITEMS: [SettingsItem; 7] = [
    SettingsItem::Toggle(ToggleKind::Autoplay),
    SettingsItem::Toggle(ToggleKind::AutoplayVideo),
    SettingsItem::Choice(ChoiceKind::AudioSpeed),
    SettingsItem::Choice(ChoiceKind::ReplayMode),
    SettingsItem::Toggle(ToggleKind::RatingIcon),
    SettingsItem::Toggle(ToggleKind::UiSfx),
    SettingsItem::Choice(ChoiceKind::SfxVolume),
];
const CONTROLS_ITEMS: [SettingsItem; 13] = [
    SettingsItem::Choice(ChoiceKind::ScrollSpeed),
    SettingsItem::Binding(ReviewAction::ShowAnswer),
    SettingsItem::Binding(ReviewAction::Again),
    SettingsItem::Binding(ReviewAction::Hard),
    SettingsItem::Binding(ReviewAction::Good),
    SettingsItem::Binding(ReviewAction::Easy),
    SettingsItem::Binding(ReviewAction::PlayAudio),
    SettingsItem::Binding(ReviewAction::Undo),
    SettingsItem::Binding(ReviewAction::Bury),
    SettingsItem::Binding(ReviewAction::Hint),
    SettingsItem::Binding(ReviewAction::ScrollUp),
    SettingsItem::Binding(ReviewAction::ScrollDown),
    SettingsItem::ResetControls,
];
const NETWORK_ITEMS: [SettingsItem; 6] = [
    SettingsItem::Action(Action::Login, "Log in"),
    SettingsItem::Action(Action::Logout, "Log out"),
    SettingsItem::Action(Action::FullSync, "Full sync"),
    SettingsItem::Action(Action::Clock, "Update clock"),
    SettingsItem::Choice(ChoiceKind::Connections),
    SettingsItem::Action(Action::MediaSync, "Media sync"),
];
const SYSTEM_ITEMS: [SettingsItem; 3] = [
    SettingsItem::Toggle(ToggleKind::ConsoleLeds),
    SettingsItem::Action(Action::Quickstart, "Quickstart guide"),
    SettingsItem::Action(Action::Help, "Help"),
];
const LANGUAGE_ITEMS: [SettingsItem; 1] = [SettingsItem::Choice(ChoiceKind::Language)];

fn settings_items(category: SettingsCategory) -> &'static [SettingsItem] {
    match category {
        SettingsCategory::Appearance => &APPEARANCE_ITEMS,
        SettingsCategory::Audio => &AUDIO_ITEMS,
        SettingsCategory::Controls => &CONTROLS_ITEMS,
        SettingsCategory::Network => &NETWORK_ITEMS,
        SettingsCategory::Backups => &[],
        SettingsCategory::System => &SYSTEM_ITEMS,
        SettingsCategory::Language => &LANGUAGE_ITEMS,
    }
}

fn settings_row_count(category: SettingsCategory) -> usize {
    settings_items(category).len()
}

fn choice_title(kind: ChoiceKind) -> &'static str {
    match kind {
        ChoiceKind::Language => "Language",
        ChoiceKind::Theme => "Theme",
        ChoiceKind::Accent => "Accent color",
        ChoiceKind::FontScale => "Font scale",
        ChoiceKind::Spacing => "Text spacing",
        ChoiceKind::ScrollSpeed => "Scrolling speed",
        ChoiceKind::Battery => "Battery indicator",
        ChoiceKind::Connections => "Download connections",
        ChoiceKind::AudioSpeed => "Audio playback speed",
        ChoiceKind::ReplayMode => "Replay audio",
        ChoiceKind::SfxVolume => "Sound effects volume",
    }
}

fn choice_options(kind: ChoiceKind) -> Vec<String> {
    match kind {
        ChoiceKind::Language => i18n::LANGUAGES
            .iter()
            .map(|(_, name)| (*name).to_owned())
            .collect(),
        ChoiceKind::Theme => ThemeKind::ALL
            .iter()
            .map(|value| tr(value.label()).to_owned())
            .collect(),
        ChoiceKind::Accent => theme::AccentKind::ALL
            .iter()
            .map(|value| tr(value.label()).to_owned())
            .collect(),
        ChoiceKind::FontScale => i18n::FONT_SCALES
            .iter()
            .map(|value| format!("{value}%"))
            .collect(),
        ChoiceKind::Spacing => vec![
            tr("Compact").to_owned(),
            tr("Normal").to_owned(),
            tr("Relaxed").to_owned(),
        ],
        ChoiceKind::ScrollSpeed => vec![
            tr("Slow").to_owned(),
            tr("Normal").to_owned(),
            tr("Fast").to_owned(),
            tr("Very fast").to_owned(),
            tr("Extreme").to_owned(),
        ],
        ChoiceKind::Battery => {
            vec![
                tr("Linear (25%)").to_owned(),
                tr("System default").to_owned(),
            ]
        }
        ChoiceKind::Connections => (1..=4).map(|value| value.to_string()).collect(),
        ChoiceKind::AudioSpeed => i18n::AUDIO_SPEED_LABELS
            .iter()
            .map(|value| (*value).to_owned())
            .collect(),
        ChoiceKind::ReplayMode => vec![
            tr("Current side").to_owned(),
            tr("All card audio").to_owned(),
        ],
        ChoiceKind::SfxVolume => i18n::SFX_VOLUMES.iter().map(|v| format!("{v}%")).collect(),
    }
}

fn choice_selected(kind: ChoiceKind) -> usize {
    match kind {
        ChoiceKind::Language => i18n::language_index(),
        ChoiceKind::Theme => ThemeKind::ALL
            .iter()
            .position(|value| *value == theme::kind())
            .unwrap_or(0),
        ChoiceKind::Accent => theme::AccentKind::ALL
            .iter()
            .position(|value| *value == theme::accent_kind())
            .unwrap_or(0),
        ChoiceKind::FontScale => i18n::FONT_SCALES
            .iter()
            .position(|value| *value == i18n::font_scale())
            .unwrap_or(1),
        ChoiceKind::Spacing => i18n::spacing_index(),
        ChoiceKind::ScrollSpeed => i18n::scroll_speed_index(),
        ChoiceKind::Battery => usize::from(!i18n::battery_linear()),
        ChoiceKind::Connections => i18n::download_connections().saturating_sub(1),
        ChoiceKind::AudioSpeed => i18n::audio_speed_index(),
        ChoiceKind::ReplayMode => usize::from(i18n::replay_all_audio()),
        ChoiceKind::SfxVolume => i18n::sfx_volume_index(),
    }
}

fn choice_label(kind: ChoiceKind) -> String {
    let options = choice_options(kind);
    let selected = choice_selected(kind).min(options.len().saturating_sub(1));
    i18n::format("{0}: {1}", &[tr(choice_title(kind)), &options[selected]])
}

fn apply_choice(kind: ChoiceKind, index: usize) -> std::io::Result<()> {
    match kind {
        ChoiceKind::Language => {
            i18n::set_language(index)?;
            event(&format!("LANGUAGE {}", i18n::language()));
        }
        ChoiceKind::Theme => {
            let value = ThemeKind::ALL[index.min(ThemeKind::ALL.len() - 1)];
            theme::set(value)?;
            Ui::apply_theme();
            event(&format!("THEME {}", value.key()));
        }
        ChoiceKind::Accent => {
            let value = theme::AccentKind::ALL[index.min(theme::AccentKind::ALL.len() - 1)];
            theme::set_accent(value)?;
            Ui::apply_theme();
            event(&format!("ACCENT {}", value.key()));
        }
        ChoiceKind::FontScale => {
            let percent =
                i18n::set_font_scale(i18n::FONT_SCALES[index.min(i18n::FONT_SCALES.len() - 1)])?;
            event(&format!("FONT_SCALE {percent}%"));
        }
        ChoiceKind::Spacing => {
            let value = i18n::set_spacing_index(index)?;
            event(&format!("TEXT_SPACING {value}"));
        }
        ChoiceKind::ScrollSpeed => {
            let value = i18n::set_scroll_speed_index(index)?;
            event(&format!("SCROLL_SPEED {value}"));
        }
        ChoiceKind::Battery => {
            let linear = index == 0;
            i18n::set_battery_linear(linear)?;
            Ui::set_battery_linear(linear);
            event(&format!(
                "BATTERY_INDICATOR {}",
                if linear { "linear" } else { "system" }
            ));
        }
        ChoiceKind::Connections => {
            let value = i18n::set_download_connections(index as u8 + 1)?;
            event(&format!("DOWNLOAD_CONNECTIONS {value}"));
        }
        ChoiceKind::AudioSpeed => {
            let value = i18n::set_audio_speed_index(index)?;
            Ui::audio_set_speed(i18n::AUDIO_SPEEDS[value]);
            event(&format!("AUDIO_SPEED {}", i18n::AUDIO_SPEEDS[value]));
        }
        ChoiceKind::ReplayMode => {
            let value = i18n::set_replay_all_audio(index != 0)?;
            event(&format!(
                "REPLAY_AUDIO {}",
                if value { "all" } else { "side" }
            ));
        }
        ChoiceKind::SfxVolume => {
            let percent = i18n::set_sfx_volume_index(index)?;
            sfx::play_preview();
            event(&format!("SFX_VOLUME {percent}%"));
        }
    }
    Ok(())
}

fn apply_toggle(kind: ToggleKind) -> std::io::Result<()> {
    match kind {
        ToggleKind::Autoplay => {
            let value = i18n::toggle_autoplay_audio()?;
            event(&format!("AUTOPLAY {}", if value { "on" } else { "off" }));
        }
        ToggleKind::AutoplayVideo => {
            let value = i18n::toggle_autoplay_video()?;
            event(&format!(
                "AUTOPLAY_VIDEO {}",
                if value { "on" } else { "off" }
            ));
        }
        ToggleKind::ConsoleLeds => {
            let value = i18n::toggle_console_leds()?;
            leds::apply(value);
            event(&format!(
                "CONSOLE_LEDS {}",
                if value { "on" } else { "off" }
            ));
        }
        ToggleKind::RatingIcon => {
            let value = i18n::toggle_rating_icon()?;
            event(&format!("RATING_ICON {}", if value { "on" } else { "off" }));
        }
        ToggleKind::CardCounts => {
            let value = i18n::toggle_card_counts()?;
            event(&format!("CARD_COUNTS {}", if value { "on" } else { "off" }));
        }
        ToggleKind::RatingIntervals => {
            let value = i18n::toggle_rating_intervals()?;
            event(&format!(
                "RATING_INTERVALS {}",
                if value { "on" } else { "off" }
            ));
        }
        ToggleKind::UiSfx => {
            let value = i18n::toggle_ui_sfx()?;
            if value {
                sfx::play_preview();
            }
            event(&format!("UI_SFX {}", if value { "on" } else { "off" }));
        }
    }
    Ok(())
}

fn settings_item_label(
    item: SettingsItem,
    media_running: bool,
    media_files: usize,
    media_status: Option<&str>,
) -> String {
    match item {
        SettingsItem::Choice(kind) => choice_label(kind),
        SettingsItem::Toggle(ToggleKind::Autoplay) => i18n::format(
            "Auto-play audio: {0}",
            &[tr(if i18n::autoplay_audio() { "On" } else { "Off" })],
        ),
        SettingsItem::Toggle(ToggleKind::AutoplayVideo) => i18n::format(
            "Auto-play video: {0}",
            &[tr(if i18n::autoplay_video() { "On" } else { "Off" })],
        ),
        SettingsItem::Toggle(ToggleKind::ConsoleLeds) => i18n::format(
            "Console LEDs: {0}",
            &[tr(if i18n::console_leds() { "On" } else { "Off" })],
        ),
        SettingsItem::Toggle(ToggleKind::RatingIcon) => i18n::format(
            "Rating icon: {0}",
            &[tr(if i18n::rating_icon() { "On" } else { "Off" })],
        ),
        SettingsItem::Toggle(ToggleKind::CardCounts) => i18n::format(
            "Card counts: {0}",
            &[tr(if i18n::card_counts() { "On" } else { "Off" })],
        ),
        SettingsItem::Toggle(ToggleKind::RatingIntervals) => i18n::format(
            "Rating intervals: {0}",
            &[tr(if i18n::rating_intervals() {
                "On"
            } else {
                "Off"
            })],
        ),
        SettingsItem::Toggle(ToggleKind::UiSfx) => i18n::format(
            "UI Sound Effects: {0}",
            &[tr(if i18n::ui_sfx() { "On" } else { "Off" })],
        ),
        SettingsItem::Binding(action) => i18n::format(
            "{0}: {1}",
            &[
                tr(action.label()),
                match controls::button_for(action) {
                    Some(button) => tr(button.label()),
                    None => tr("Unbound"),
                },
            ],
        ),
        SettingsItem::ResetControls => tr("Reset to defaults").to_owned(),
        SettingsItem::Toolbar => i18n::format(
            "Toolbar: {0}",
            &[tr(if i18n::toolbar_visible() { "On" } else { "Off" })],
        ),
        SettingsItem::Animations => i18n::format(
            "Animations: {0}",
            &[tr(if i18n::animations_enabled() {
                "On"
            } else {
                "Off"
            })],
        ),
        SettingsItem::Action(Action::MediaSync, _) => {
            media_sync_label(media_running, media_files, media_status)
        }
        SettingsItem::Action(_, label) => tr(label).to_owned(),
    }
}

fn media_sync_label(media_running: bool, media_files: usize, media_status: Option<&str>) -> String {
    if media_running {
        if let Some(status) = media_status {
            i18n::format("Media sync: {0}. A stops.", &[status])
        } else {
            i18n::format(
                "Media sync: {0} files. A stops.",
                &[&media_files.to_string()],
            )
        }
    } else if let Some(status) = media_status {
        i18n::format("Media sync: {0}. A starts.", &[status])
    } else {
        i18n::format("Media sync: {0}. A starts.", &[tr("Idle")])
    }
}
const MENU_COLS: usize = 2;
const MENU_TILE_W: f32 = 146.0;
const MENU_TILE_H: f32 = 30.0;
const MENU_TILE_GAP_X: f32 = 8.0;
const MENU_TILE_GAP_Y: f32 = 6.0;
const MENU_TILE_X0: f32 = 10.0;
const MENU_TILE_Y0: f32 = 44.0;
const MENU_ROWS_MAX: usize = 5;

fn menu_tile(index: usize, rows: usize) -> (f32, f32) {
    let col = index % MENU_COLS;
    let row = index / MENU_COLS;
    let y_offset = (MENU_ROWS_MAX - rows) as f32 * (MENU_TILE_H + MENU_TILE_GAP_Y) * 0.5;
    (
        MENU_TILE_X0 + col as f32 * (MENU_TILE_W + MENU_TILE_GAP_X),
        MENU_TILE_Y0 + y_offset + row as f32 * (MENU_TILE_H + MENU_TILE_GAP_Y),
    )
}

fn menu_hit(x: f32, y: f32, rows: usize, length: usize) -> Option<usize> {
    (0..length).find(|index| {
        let (tx, ty) = menu_tile(*index, rows);
        x >= tx && x < tx + MENU_TILE_W && y >= ty && y < ty + MENU_TILE_H
    })
}

const SETTINGS_TILE_X0: f32 = 10.0;
const SETTINGS_TILE_Y0: f32 = 42.0;
const SETTINGS_TILE_W: f32 = 292.0;
const SETTINGS_TILE_H: f32 = 20.0;
const SETTINGS_TILE_GAP: f32 = 2.0;
const CHOICE_TILE_H: f32 = 18.0;
const CHOICE_TILE_GAP: f32 = 2.0;
const SETTINGS_VISIBLE_ROWS: usize = 9;
const CHOICE_VISIBLE_ROWS: usize = 9;
const TOOLBAR_ROWS: usize = 6;

fn list_start(selected: usize, length: usize) -> usize {
    if length <= SETTINGS_VISIBLE_ROWS {
        0
    } else {
        selected
            .saturating_sub(SETTINGS_VISIBLE_ROWS - 1)
            .min(length - SETTINGS_VISIBLE_ROWS)
    }
}

fn choice_start(selected: usize, length: usize) -> usize {
    if length <= CHOICE_VISIBLE_ROWS {
        0
    } else {
        selected
            .saturating_sub(CHOICE_VISIBLE_ROWS - 1)
            .min(length - CHOICE_VISIBLE_ROWS)
    }
}

fn settings_tile(row: usize) -> (f32, f32) {
    (
        SETTINGS_TILE_X0,
        SETTINGS_TILE_Y0 + row as f32 * (SETTINGS_TILE_H + SETTINGS_TILE_GAP),
    )
}

fn choice_tile(row: usize) -> (f32, f32) {
    (
        SETTINGS_TILE_X0,
        SETTINGS_TILE_Y0 + row as f32 * (CHOICE_TILE_H + CHOICE_TILE_GAP),
    )
}

fn choice_hit(x: f32, y: f32, start: usize, length: usize) -> Option<usize> {
    let row = ((y - SETTINGS_TILE_Y0) / (CHOICE_TILE_H + CHOICE_TILE_GAP)) as usize;
    let index = start + row;
    if index >= length {
        return None;
    }
    let (tx, ty) = choice_tile(row);
    (x >= tx && x < tx + SETTINGS_TILE_W && y >= ty && y < ty + CHOICE_TILE_H).then_some(index)
}

fn list_hit(x: f32, y: f32, start: usize, length: usize) -> Option<usize> {
    let row = ((y - SETTINGS_TILE_Y0) / (SETTINGS_TILE_H + SETTINGS_TILE_GAP)) as usize;
    let index = start + row;
    if index >= length {
        return None;
    }
    let (tx, ty) = settings_tile(row);
    (x >= tx && x < tx + SETTINGS_TILE_W && y >= ty && y < ty + SETTINGS_TILE_H).then_some(index)
}

fn list_step(selected: usize, length: usize, down: bool) -> usize {
    if down {
        (selected + 1).min(length.saturating_sub(1))
    } else {
        selected.saturating_sub(1)
    }
}

fn menu_label(index: usize) -> String {
    tr(MAIN_MENU[index].1).to_owned()
}

fn main() {
    event_log::begin_session();
    std::panic::set_hook(Box::new(|info| {
        event(&format!("PANIC {info}"));
    }));
    let apt = match Apt::new() {
        Ok(apt) => apt,
        Err(error) => {
            eprintln!("APT initialization failed: {error}");
            return;
        }
    };
    let mut hid = match Hid::new() {
        Ok(hid) => hid,
        Err(error) => {
            eprintln!("HID initialization failed: {error}");
            return;
        }
    };
    let _gfx = match Gfx::new() {
        Ok(gfx) => gfx,
        Err(error) => {
            eprintln!("graphics initialization failed: {error}");
            return;
        }
    };
    if !Ui::init() {
        eprintln!("Failed to initialize Citro2D/Citro3D UI engine");
        return;
    }
    use tracing_subscriber::prelude::*;
    tracing_subscriber::registry()
        .with(
            tracing_subscriber::fmt::layer()
                .without_time()
                .with_ansi(false)
                .with_writer(event_log::trace_writer)
                .with_filter(
                    tracing_subscriber::filter::Targets::new()
                        .with_target("anki::sync::collection::normal", tracing::Level::DEBUG)
                        .with_target("anki_3ds_sync_transport", tracing::Level::DEBUG),
                ),
        )
        .init();
    i18n::init();
    theme::init();
    controls::init();
    leds::init();
    // Hold one SOC handle for the app's lifetime, so the shared service never
    // drops to zero references between syncs and the static buffer is never
    // re-initialised.
    #[cfg(target_os = "horizon")]
    let _soc_guard = anki_3ds_sync_transport::soc::NativeSoc::new().ok();
    Ui::apply_theme();
    Ui::set_battery_linear(i18n::battery_linear());
    Ui::audio_set_speed(i18n::audio_speed());
    let startup_error = assets::prepare()
        .map_err(|e| e.to_string())
        .and_then(|_| {
            // `tempfile` resolves Rust-relative Horizon paths against the
            // current directory. Some launchers, including Azahar, leave it at
            // "/", so anchor it at the app directory before sync scratch files
            // are created. All application paths are device-qualified.
            #[cfg(target_os = "horizon")]
            if let Err(error) = std::env::set_current_dir("sdmc:/3ds/anki-3ds") {
                event(&format!("CWD_SWITCH_ERROR {error}"));
            }
            clock::configure(false, &apt, &mut hid, None)
        })
        .err();
    let started = Instant::now();
    let progress = std::sync::Arc::new(std::sync::Mutex::new(
        anki::progress::ProgressState::default(),
    ));
    let mut login = sync::Login::new();
    let mut auth = match auth_store::load(std::path::Path::new(auth_store::DIRECTORY)) {
        Ok(value) => {
            event(if value.is_some() {
                "AUTH_LOADED"
            } else {
                "AUTH_EMPTY"
            });
            value
        }
        Err(error) => {
            event(&format!("AUTH_LOAD_ERROR {error}"));
            None
        }
    };
    let mut job: Option<sync::Job> = None;
    let mut media_job: Option<media_sync::MediaSync> = None;
    let mut media_result: Option<String> = None;
    let mut media_notice_until = Instant::now();
    let mut last_media_changed = 0u32;
    let mut last_media_refresh = Instant::now();
    let mut exit_pending = false;
    let mut clock_job: Option<clock::Job> = None;
    let mut clock_auto_done = clock::is_known() && !clock::needs_refresh();
    let mut next_clock_check = Instant::now();
    let mut clock_connected = false;
    let mut backend = if startup_error.is_some() {
        None
    } else {
        match Backend::open_with_progress(progress.clone()) {
            Ok(b) => Some(b),
            Err(e) => {
                event(&format!("OPEN_ERROR {}", error_text(e.as_ref())));
                None
            }
        }
    };
    let mut synced_ms = i18n::sync_stamp();
    let mut current_ms = backend
        .as_mut()
        .and_then(|b| b.collection_modified_ms().ok())
        .unwrap_or(0);
    if synced_ms == 0 {
        synced_ms = current_ms;
    }
    if let Some(b) = backend.as_mut() {
        let today = clock::local_time().div_euclid(86_400);
        let existing_empty = backup::list().is_empty();
        let modified = current_ms > 0 && (current_ms != i18n::backup_stamp() || existing_empty);
        let due_today = i18n::backup_day() != Some(today) || existing_empty;
        if modified && due_today {
            match backup::create_from_backend(b, current_ms, Some(today)) {
                Ok(entry) => event(&format!("BACKUP_AUTO startup {}", entry.filename)),
                Err(error) => event(&format!(
                    "BACKUP_AUTO_ERROR startup {}",
                    error_text(error.as_ref())
                )),
            }
        }
    }
    let mut root = DeckTreeNode::default();
    let mut screen = if let Some(error) = startup_error {
        Screen::Error(error)
    } else {
        match backend.as_mut() {
            Some(b) => match b.deck_tree() {
                Ok(tree) => {
                    root = tree;
                    if i18n::quickstart_done() {
                        Screen::Decks
                    } else {
                        Screen::Quickstart {
                            step: 0,
                            selected: i18n::language_index(),
                            previous: Box::new(Screen::Decks),
                        }
                    }
                }
                Err(e) => Screen::Error(error_text(e.as_ref())),
            },
            None => Screen::Error(
                tr("Could not open prototype.anki2. See prototype-events.txt.").into(),
            ),
        }
    };
    event(&format!(
        "OPEN startup_ms={} heap_bytes={}",
        started.elapsed().as_millis(),
        anki_3ds_sqlite_vfs::heap_used()
    ));
    tracing::debug!(
        target: "anki_3ds_sync_transport",
        cwd = ?std::env::current_dir(),
        "startup working directory"
    );
    let mut collapsed = HashSet::new();
    let mut decks = Vec::new();
    collect_deck_items(&root, &collapsed, &mut decks);
    let mut selected = 0usize;
    let mut scroll_pos = 0usize;
    let mut card_scroll = 0usize;
    let mut review_scroll = 0.0f32;
    let mut review_max_scroll = 0.0f32;
    let mut review = ReviewState::default();
    let mut last_frame = Instant::now();
    let mut audio_index = 0usize;
    let mut audio_page = 0usize;
    let mut last_autoplay_card: Option<(i64, bool)> = None;
    let mut last_review_card: Option<(i64, bool)> = None;
    let mut card_media_pending = false;
    let mut prefetch = Prefetch::default();
    let mut playing_audio: Option<String> = None;
    let mut audio_queue: Vec<(String, String)> = Vec::new();
    let mut audio_queue_pos = 0usize;
    let mut frame: u64 = 0;
    let mut top_layout = TextLayout::default();
    let mut top_card = RichTextLayout::default();
    let mut ui_anim = anim::UiAnimState::new();
    let mut top_max = 0usize;
    let mut last_empty_poll = Instant::now();
    let mut repeat_at = Instant::now();
    #[cfg(feature = "emulator")]
    let script: Vec<String> = fs::read_to_string(TEST_SCRIPT)
        .unwrap_or_default()
        .lines()
        .map(str::trim)
        .filter(|s| !s.is_empty() && !s.starts_with('#'))
        .map(str::to_owned)
        .collect();
    #[cfg(not(feature = "emulator"))]
    let script: Vec<String> = Vec::new();
    let mut script_step = 0;
    let mut script_cooldown = 120;
    let mut last_state = String::new();
    let mut last_media_running = false;
    let mut last_media_processed = usize::MAX;
    let mut last_media_checked = 0usize;
    let mut last_media_result: Option<String> = None;
    let mut last_clock_known = false;
    let mut cached_media_status: Option<String> = None;
    let mut cached_footer = String::new();
    let mut daily_studied = String::new();
    let mut rating_feedback: Option<(i32, Instant)> = None;
    let mut leech_notice: Option<(String, Instant)> = None;
    let mut last_answered: Option<i64> = None;
    let trace_touch = fs::metadata("sdmc:/3ds/anki-3ds/trace-touch").is_ok();

    while apt.main_loop() {
        leds::poll_wakeup();
        if clock::ready() {
            clock_auto_done = true;
        }
        if let Some(result) = clock_job.as_mut().and_then(clock::Job::poll) {
            clock_job = None;
            match result {
                Ok(offset) => {
                    clock_auto_done = true;
                    if let Some(backend) = backend.as_mut() {
                        if let Err(error) = backend.set_clock_offset(offset) {
                            event(&format!("CLOCK_OFFSET_ERROR {error}"));
                        }
                    }
                }
                Err(error) => {
                    next_clock_check = Instant::now() + Duration::from_secs(60);
                    event(&format!("CLOCK_AUTO_ERROR {error}"));
                }
            }
        }
        let connected = clock::connected();
        if connected && !clock_connected && !clock_auto_done {
            next_clock_check = Instant::now();
        }
        clock_connected = connected;
        if !clock_auto_done
            && clock_job.is_none()
            && job.is_none()
            && media_job.is_none()
            && screen_is_startup_or_decks(&screen)
            && connected
            && clock::needs_refresh()
            && Instant::now() >= next_clock_check
        {
            next_clock_check =
                Instant::now() + Duration::from_secs(if clock::is_known() { 1800 } else { 60 });
            match clock::Job::start() {
                Ok(worker) => {
                    clock_job = Some(worker);
                    event("CLOCK_AUTO_START");
                }
                Err(error) => event(&format!("CLOCK_AUTO_ERROR {error}")),
            }
        }
        if let Some(done) = job.as_mut().and_then(sync::Job::poll) {
            job = None;
            let sync::Completed {
                backend: done_backend,
                auth: done_auth,
                media: done_media,
                media_error: done_media_error,
                result: done_result,
            } = done;
            backend = done_backend;
            auth = done_auth;
            if let Some(active) = done_media {
                media_job = Some(active);
                media_result = None;
                last_media_changed = 0;
                last_media_refresh = Instant::now();
            }
            if let Some(error) = done_media_error {
                media_result = Some(i18n::format("failed: {0}", &[&error]));
                media_notice_until = Instant::now() + Duration::from_secs(3600);
            }
            if matches!(done_result, Ok(sync::Outcome::Complete)) {
                if let Some(b) = backend.as_mut() {
                    if let Ok(ms) = b.collection_modified_ms() {
                        current_ms = ms;
                        synced_ms = ms;
                        if let Err(error) = i18n::save_sync_stamp(ms) {
                            event(&format!("SYNC_STAMP_ERROR {error}"));
                        }
                    }
                }
            }
            screen = match done_result {
                Ok(sync::Outcome::LoggedIn) => {
                    event("LOGIN_COMPLETE");
                    Screen::Info {
                        title: tr("Logged in").into(),
                        text: tr("Login saved. You can sync without entering your password again.")
                            .into(),
                        previous: Box::new(Screen::Decks),
                    }
                }
                Ok(sync::Outcome::Complete) => {
                    event("SYNC_COMPLETE");
                    Screen::Decks
                }
                Ok(sync::Outcome::FullRequired {
                    upload_ok,
                    download_ok,
                }) => {
                    event("SYNC_FULL_REQUIRED");
                    Screen::SyncChoice {
                        upload_ok,
                        download_ok,
                        download: download_ok,
                        confirmed: false,
                    }
                }
                Err(error) => Screen::Info {
                    title: tr("Sync error").into(),
                    text: error,
                    previous: Box::new(Screen::Decks),
                },
            };
            card_scroll = 0;
            if exit_pending {
                break;
            }
        }
        let mut media_finished = false;
        let mut media_refreshed_now = false;
        if let Some(active) = &media_job {
            let changed = active.changed();
            let in_review = matches!(screen, Screen::Question { .. } | Screen::Answer { .. });
            let interval = Duration::from_millis(if in_review && !card_media_pending {
                1500
            } else {
                450
            });
            if changed != last_media_changed && last_media_refresh.elapsed() >= interval {
                last_media_changed = changed;
                last_media_refresh = Instant::now();
                if !in_review || card_media_pending {
                    Ui::media_refresh();
                    top_card.invalidate();
                    media_refreshed_now = true;
                }
            }
            media_finished = !active.is_running();
        }
        if media_finished {
            if let Some(active) = media_job.take() {
                let state = active.state();
                let files = active.processed_files();
                let (text, persistent) = match state {
                    media_sync::State::Done => (
                        i18n::format("complete ({0} files)", &[&files.to_string()]),
                        false,
                    ),
                    media_sync::State::Cancelled => (tr("stopped").to_owned(), false),
                    media_sync::State::Failed(error) => {
                        (i18n::format("failed: {0}", &[&error]), true)
                    }
                    media_sync::State::Running => (tr("running").to_owned(), false),
                };
                media_result = Some(text);
                media_notice_until =
                    Instant::now() + Duration::from_secs(if persistent { 3600 } else { 15 });
            }
            Ui::media_refresh();
            top_card.invalidate();
            media_refreshed_now = true;
        }
        hid.scan_input();
        let undo_available = last_answered.is_some();
        let down = hid.keys_down();
        let held = hid.keys_held();
        if matches!(screen, Screen::Question { .. } | Screen::Answer { .. }) {
            let factor = i18n::font_scale_factor() * i18n::scroll_speed_factor();
            let mut delta = 0.0f32;
            if controls::button_for(ReviewAction::ScrollDown)
                .is_some_and(|button| button.pressed(held))
            {
                delta += 5.5 * factor;
            }
            if controls::button_for(ReviewAction::ScrollUp)
                .is_some_and(|button| button.pressed(held))
            {
                delta -= 5.5 * factor;
            }
            let (_, pad_y) = hid.circlepad_position();
            if pad_y.abs() > 24 {
                delta -= (f32::from(pad_y) / 156.0) * 9.0 * factor;
            }
            if delta != 0.0 {
                review_scroll = (review_scroll + delta).clamp(0.0, review_max_scroll);
            }
        }
        let directional_down = down.intersects(
            KeyPad::UP
                | KeyPad::DOWN
                | KeyPad::LEFT
                | KeyPad::RIGHT
                | KeyPad::CPAD_UP
                | KeyPad::CPAD_DOWN
                | KeyPad::CPAD_LEFT
                | KeyPad::CPAD_RIGHT
                | KeyPad::CSTICK_UP
                | KeyPad::CSTICK_DOWN
                | KeyPad::CSTICK_LEFT
                | KeyPad::CSTICK_RIGHT,
        );
        let repeat = if directional_down {
            repeat_at = Instant::now() + Duration::from_millis(300);
            down
        } else if Instant::now() >= repeat_at {
            repeat_at = Instant::now() + Duration::from_millis(90);
            held
        } else {
            down
        };
        let review_state = match &screen {
            Screen::Question { .. } => Some(ScreenState::Front),
            Screen::Answer { .. } => Some(ScreenState::Back),
            _ => None,
        };
        let review_action = review_state.and_then(|state| {
            controls::Button::ALL.iter().copied().find_map(|button| {
                button
                    .pressed(down)
                    .then(|| controls::action_for(button, state))
                    .flatten()
            })
        });
        let mut action = if down.contains(KeyPad::START) {
            Action::Exit
        } else if down.contains(KeyPad::SELECT) {
            Action::Menu
        } else if let Some(review_action) = review_action {
            review_action_to_action(review_action, &screen, &top_card, &review)
        } else if down.contains(KeyPad::B) {
            Action::Back
        } else if review_state.is_some() {
            if down.intersects(KeyPad::LEFT | KeyPad::CPAD_LEFT | KeyPad::CSTICK_LEFT) {
                Action::AudioPage(-1)
            } else if down.intersects(KeyPad::RIGHT | KeyPad::CPAD_RIGHT | KeyPad::CSTICK_RIGHT) {
                Action::AudioPage(1)
            } else {
                Action::None
            }
        } else if matches!(screen, Screen::Editor { .. } | Screen::Options { .. })
            && down.contains(KeyPad::X)
        {
            Action::Save
        } else if matches!(screen, Screen::Browser(_)) && down.contains(KeyPad::X) {
            Action::Search
        } else if matches!(screen, Screen::SettingsToolbar { .. }) && down.contains(KeyPad::Y) {
            Action::Left
        } else if matches!(screen, Screen::SettingsToolbar { .. }) && down.contains(KeyPad::X) {
            Action::Right
        } else if matches!(screen, Screen::Stats { .. }) && down.contains(KeyPad::L) {
            Action::Up
        } else if matches!(screen, Screen::Stats { .. }) && down.contains(KeyPad::R) {
            Action::Down
        } else if down.contains(KeyPad::A) {
            Action::Primary
        } else if repeat.intersects(KeyPad::DOWN | KeyPad::CPAD_DOWN | KeyPad::CSTICK_DOWN) {
            Action::Down
        } else if repeat.intersects(KeyPad::UP | KeyPad::CPAD_UP | KeyPad::CSTICK_UP) {
            Action::Up
        } else if repeat.intersects(KeyPad::LEFT | KeyPad::CPAD_LEFT | KeyPad::CSTICK_LEFT) {
            Action::Left
        } else if repeat.intersects(KeyPad::RIGHT | KeyPad::CPAD_RIGHT | KeyPad::CSTICK_RIGHT) {
            Action::Right
        } else {
            Action::None
        };
        let mut quickstart_touch = None;
        if down.contains(KeyPad::TOUCH) {
            let (x, y) = hid.touch_position();
            if trace_touch {
                event(&format!("TOUCH x={x} y={y}"));
            }
            let x = x as f32;
            let y = y as f32;
            anim::record_touch(x, y);
            action = match &mut screen {
                Screen::SyncWait if y >= 194.0 => Action::Back,
                Screen::SyncChoice {
                    upload_ok,
                    download_ok,
                    download,
                    confirmed,
                } if (64.0..92.0).contains(&y) => {
                    let direction = x >= 160.0;
                    if (if direction { *download_ok } else { *upload_ok }) && *download != direction
                    {
                        *download = direction;
                        *confirmed = false;
                    }
                    Action::None
                }
                Screen::SyncChoice { .. } if y >= 194.0 => {
                    if x < 160.0 {
                        Action::Primary
                    } else {
                        Action::Back
                    }
                }
                Screen::Decks if y < 42.0 => match (x / 80.0) as usize {
                    0 => Action::Add,
                    1 => Action::Edit,
                    2 => Action::Browse,
                    _ => Action::Options,
                },
                Screen::Decks if (ROW_Y..ROW_Y + ROW_H * ROWS as f32).contains(&y) && x < 304.0 => {
                    let index = scroll_pos + ((y - ROW_Y) / ROW_H) as usize;
                    if let Some(deck) = decks.get(index) {
                        if deck.has_children
                            && x >= 8.0 + deck.level.min(8) as f32 * 16.0
                            && x < 24.0 + deck.level.min(8) as f32 * 16.0
                        {
                            selected = index;
                            if collapsed.contains(&deck.id) {
                                Action::Right
                            } else {
                                Action::Left
                            }
                        } else if selected == index {
                            Action::Primary
                        } else {
                            selected = index;
                            Action::None
                        }
                    } else {
                        Action::None
                    }
                }
                Screen::Decks if x >= 304.0 && (43.0..187.0).contains(&y) => {
                    let max = decks.len().saturating_sub(ROWS);
                    scroll_pos = (((y - 43.0) / 144.0) * max as f32).round() as usize;
                    scroll_pos = scroll_pos.min(max);
                    selected = selected.clamp(
                        scroll_pos,
                        (scroll_pos + ROWS - 1).min(decks.len().saturating_sub(1)),
                    );
                    Action::None
                }
                Screen::Decks if (194.0..222.0).contains(&y) && (12.0..152.0).contains(&x) => {
                    Action::Primary
                }
                Screen::Decks if (194.0..222.0).contains(&y) && (168.0..308.0).contains(&x) => {
                    Action::Sync
                }
                Screen::Question { card, .. }
                    if card.type_answer.is_some()
                        && (96.0..124.0).contains(&y)
                        && (8.0..216.0).contains(&x) =>
                {
                    Action::TypeAnswer
                }
                Screen::Question { card, .. }
                    if (AUDIO_Y..AUDIO_Y + AUDIO_H + 8.0).contains(&y) =>
                {
                    audio_tap(&card_audio(card, false), audio_page, x)
                }
                Screen::Answer { card, .. } if (AUDIO_Y..AUDIO_Y + AUDIO_H + 8.0).contains(&y) => {
                    audio_tap(&card_audio(card, true), audio_page, x)
                }
                Screen::Question { .. }
                    if (128.0..180.0).contains(&y) && (8.0..216.0).contains(&x) =>
                {
                    Action::Reveal
                }
                Screen::Question { .. }
                    if (128.0..180.0).contains(&y) && (224.0..312.0).contains(&x) =>
                {
                    if undo_available {
                        Action::Undo
                    } else {
                        Action::None
                    }
                }
                Screen::Answer { .. }
                    if (148.0..184.0).contains(&y) && (236.0..308.0).contains(&x) =>
                {
                    if undo_available {
                        Action::Undo
                    } else {
                        Action::None
                    }
                }
                Screen::Answer { .. } if (190.0..226.0).contains(&y) => {
                    let n = ((x - 8.0) / 76.0).floor() as i32;
                    if (0..4).contains(&n)
                        && x >= 8.0 + n as f32 * 76.0
                        && x < 80.0 + n as f32 * 76.0
                    {
                        Action::Rate(n)
                    } else {
                        Action::None
                    }
                }
                Screen::Question { .. } if (184.0..202.0).contains(&y) => {
                    hint_chip_tap(&top_card, &review, x)
                }
                Screen::Question { .. } if (206.0..236.0).contains(&y) => {
                    media_bar_tap(&top_card, x, false)
                }
                Screen::Answer { .. } if (126.0..144.0).contains(&y) => {
                    hint_chip_tap(&top_card, &review, x)
                }
                Screen::Answer { .. } if (150.0..176.0).contains(&y) => {
                    media_bar_tap(&top_card, x, true)
                }
                Screen::Editor { .. } if x >= 304.0 && (43.0..187.0).contains(&y) => {
                    if y < 115.0 {
                        Action::Up
                    } else {
                        Action::Down
                    }
                }
                Screen::Browser(_) if y < 26.0 => Action::Search,
                Screen::Browser(_) if (28.0..52.0).contains(&y) => {
                    let mut hit = Action::None;
                    for (index, (cx, cw)) in browse_chip_rects().iter().enumerate() {
                        if x >= *cx && x < cx + cw {
                            hit = Action::Filter(index);
                            break;
                        }
                    }
                    hit
                }
                Screen::Browser(b) if x < 304.0 && (54.0..186.0).contains(&y) => {
                    let index = b.page * ROWS + ((y - 54.0) / 22.0) as usize;
                    if index < b.ids.len() {
                        b.selected = index;
                        Action::Primary
                    } else {
                        Action::None
                    }
                }
                Screen::Browser(_) if x >= 304.0 && (54.0..186.0).contains(&y) => {
                    if y < 120.0 {
                        Action::Up
                    } else {
                        Action::Down
                    }
                }
                Screen::Browser(_) if (194.0..222.0).contains(&y) => {
                    if x < 108.0 {
                        Action::Edit
                    } else if x < 212.0 {
                        Action::Restore
                    } else {
                        Action::Search
                    }
                }
                Screen::Editor { draft, field, .. } if (43.0..187.0).contains(&y) => {
                    let index = *field / ROWS * ROWS + ((y - 43.0) / 24.0) as usize;
                    if index <= draft.labels.len() {
                        *field = index;
                        Action::Field
                    } else {
                        Action::None
                    }
                }
                Screen::Editor { .. } | Screen::Options { .. } if (194.0..222.0).contains(&y) => {
                    if x < 160.0 {
                        Action::Save
                    } else {
                        Action::Back
                    }
                }
                Screen::Options { field, .. } if (43.0..187.0).contains(&y) => {
                    let start = *field / ROWS * ROWS;
                    *field = (start + ((y - 43.0) / 24.0) as usize).min(OPTION_ROWS - 1);
                    Action::Field
                }
                Screen::Menu { selected, .. } => {
                    if let Some(index) = menu_hit(x, y, MENU_ROWS_MAX, MAIN_MENU.len()) {
                        *selected = index;
                        Action::Primary
                    } else {
                        Action::None
                    }
                }
                Screen::Settings { selected, .. } => {
                    let start = list_start(*selected, SettingsCategory::ALL.len());
                    if let Some(index) = list_hit(x, y, start, SettingsCategory::ALL.len()) {
                        *selected = index;
                        Action::Primary
                    } else {
                        Action::None
                    }
                }
                Screen::SettingsCategory {
                    category, selected, ..
                } => {
                    let length = settings_row_count(*category);
                    let start = list_start(*selected, length);
                    if let Some(index) = list_hit(x, y, start, length) {
                        *selected = index;
                        Action::Primary
                    } else {
                        Action::None
                    }
                }
                Screen::SettingsChoice { kind, selected, .. } => {
                    let length = choice_options(*kind).len();
                    let start = choice_start(*selected, length);
                    if let Some(index) = choice_hit(x, y, start, length) {
                        *selected = index;
                        Action::Primary
                    } else {
                        Action::None
                    }
                }
                Screen::SettingsButton { selected, .. } => {
                    let length = controls::button_option_count();
                    let start = choice_start(*selected, length);
                    if let Some(index) = choice_hit(x, y, start, length) {
                        *selected = index;
                        Action::Primary
                    } else {
                        Action::None
                    }
                }
                Screen::SettingsToolbar { selected, .. } => {
                    let start = list_start(*selected, 5);
                    if let Some(index) = list_hit(x, y, start, 5) {
                        *selected = index;
                        Action::Primary
                    } else if (150.0..170.0).contains(&y) && (10.0..150.0).contains(&x) {
                        *selected = 5;
                        Action::Primary
                    } else if (194.0..222.0).contains(&y) {
                        if (12.0..152.0).contains(&x) {
                            Action::Left
                        } else if (168.0..308.0).contains(&x) {
                            Action::Right
                        } else {
                            Action::None
                        }
                    } else {
                        Action::None
                    }
                }
                Screen::SettingsAnimations { selected, .. } => {
                    let start = list_start(*selected, 5);
                    if let Some(index) = list_hit(x, y, start, 5) {
                        *selected = index;
                        Action::Primary
                    } else {
                        Action::None
                    }
                }
                Screen::SettingsBackups {
                    selected, backups, ..
                } => {
                    let length = backups.len() + 1;
                    let start = list_start(*selected, length);
                    if let Some(index) = list_hit(x, y, start, length) {
                        *selected = index;
                        Action::Primary
                    } else {
                        Action::None
                    }
                }
                Screen::BackupConfirm { .. } if (194.0..222.0).contains(&y) => {
                    if x < 160.0 {
                        Action::Primary
                    } else {
                        Action::Back
                    }
                }
                Screen::CardChoice { selected, .. } if (70.0..118.0).contains(&y) => {
                    *selected = if x < 160.0 { 0 } else { 1 };
                    Action::Primary
                }
                Screen::CardChoice { .. } if (150.0..192.0).contains(&y) => Action::Back,
                Screen::Stats { page, timeframe, .. } => match stats::touch_hit(x, y) {
                    stats::TouchAction::Page(next) => {
                        *page = next;
                        event(&format!(
                            "STATS_PAGE {} range={}",
                            *page as usize, *timeframe as usize
                        ));
                        Action::None
                    }
                    stats::TouchAction::Timeframe(next) => {
                        *timeframe = next;
                        event(&format!(
                            "STATS_PAGE {} range={}",
                            *page as usize, *timeframe as usize
                        ));
                        Action::None
                    }
                    stats::TouchAction::Back => Action::Back,
                    stats::TouchAction::None => Action::None,
                },
                Screen::Quickstart { step, .. } => {
                    quickstart_touch = Some(quickstart::touch_hit(*step, x, y));
                    Action::None
                }
                Screen::Info { .. } if (194.0..222.0).contains(&y) => Action::Back,
                Screen::Empty { .. } if (110.0..144.0).contains(&y) => Action::Back,
                _ => Action::None,
            };
            if y >= 224.0
                && !matches!(
                    screen,
                    Screen::Question { .. }
                        | Screen::Answer { .. }
                        | Screen::SyncChoice { .. }
                        | Screen::SettingsBackups { .. }
                        | Screen::BackupConfirm { .. }
                        | Screen::SettingsAnimations { .. }
                        | Screen::CardChoice { .. }
                        | Screen::Stats { .. }
                        | Screen::Quickstart { .. }
                )
            {
                action = if x < 160.0 {
                    Action::Menu
                } else {
                    Action::Options
                };
            }
        }

        let mut script_command = None;
        if script_step < script.len()
            && (job.is_none()
                || (script[script_step] == "CANCEL"
                    && job.as_ref().is_some_and(sync::Job::is_syncing)))
        {
            if script_cooldown > 0 {
                script_cooldown -= 1;
            } else {
                let cmd = script[script_step].clone();
                script_step += 1;
                script_cooldown = 120;
                event(&format!("SCRIPT_EXEC {cmd}"));
                let mut parts = cmd.splitn(2, ' ');
                let name = parts.next().unwrap();
                let value = parts.next().unwrap_or("");
                action = match name {
                    "SELECT" => {
                        selected = value.parse().unwrap_or(0);
                        Action::None
                    }
                    "STUDY" => Action::Primary,
                    "REVEAL" => Action::Reveal,
                    "ANSWER" => Action::Rate(value.parse().unwrap_or(-1)),
                    "BACK" | "CANCEL" => Action::Back,
                    "ADD" => Action::Add,
                    "EDIT" => Action::Edit,
                    "BROWSE" => Action::Browse,
                    "SAVE" => Action::Save,
                    "OPTIONS" => Action::Options,
                    "SETTINGS" => Action::Settings,
                    "QUICKSTART" => Action::Quickstart,
                    "MENU" => Action::Menu,
                    "HELP" => Action::Help,
                    "STATS" => Action::Stats,
                    "SYNC" => Action::Sync,
                    "LOGIN" => Action::Login,
                    "LOGOUT" => Action::Logout,
                    "LANGUAGE" => Action::Language,
                    "UNDO" => Action::Undo,
                    "CLOCK" => Action::Clock,
                    "EXIT" => Action::Exit,
                    "UP" => Action::Up,
                    "DOWN" => Action::Down,
                    "LEFT" => Action::Left,
                    "RIGHT" => Action::Right,
                    "FILTER" => Action::Filter(value.parse().unwrap_or(0)),
                    "RESTORE" => Action::Restore,
                    "AUDIO" => Action::PlayAudio(None),
                    "AUDIO_NEXT" => Action::AudioPage(1),
                    "AUDIO_PREV" => Action::AudioPage(-1),
                    "COLLAPSE" => {
                        selected = value.parse().unwrap_or(0);
                        Action::Left
                    }
                    "EXPAND" => {
                        selected = value.parse().unwrap_or(0);
                        Action::Right
                    }
                    "WAIT" => {
                        script_cooldown = value.parse().unwrap_or(120);
                        Action::None
                    }
                    "FIELD" | "LIMITS" | "SEARCH" => {
                        script_command = Some(cmd.clone());
                        Action::None
                    }
                    _ => Action::None,
                };
            }
        }
        if !down.contains(KeyPad::TOUCH) {
            match &action {
                Action::Rate(rating) if (0..4).contains(rating) => {
                    anim::record_button_style((*rating + 2) as u8);
                }
                Action::Primary | Action::Reveal | Action::Save | Action::Sync | Action::Field => {
                    anim::record_button_style(0);
                }
                Action::Left | Action::Right
                    if matches!(
                        screen,
                        Screen::SettingsCategory { .. }
                            | Screen::SettingsAnimations { .. }
                            | Screen::SettingsToolbar { .. }
                            | Screen::SettingsChoice { .. }
                            | Screen::Quickstart { .. }
                            | Screen::Stats { .. }
                    ) =>
                {
                    anim::record_button_style(0);
                }
                _ => {}
            }
        }
        if matches!(action, Action::Exit) {
            if let Some(active) = &media_job {
                active.cancel();
            }
            if let Some(job) = &job {
                job.cancel();
                exit_pending = true;
                action = Action::None;
            } else {
                break;
            }
        }
        selected = selected.min(decks.len().saturating_sub(1));
        let did = decks.get(selected).map(|d| d.id).unwrap_or(1);
        let deck_name = decks
            .get(selected)
            .map(|d| d.name.clone())
            .unwrap_or_default();

        let result: backend::Result<()> = (|| {
            if let Some(active) = &job {
                if matches!(action, Action::Back) {
                    active.cancel();
                }
                return Ok(());
            }
            if let Screen::Menu { selected, .. } = &screen {
                if matches!(action, Action::Primary) {
                    let chosen = MAIN_MENU[*selected].0;
                    if matches!(chosen, Action::Settings) {
                        let menu = std::mem::replace(&mut screen, Screen::Decks);
                        screen = Screen::Settings {
                            selected: 0,
                            previous: Box::new(menu),
                        };
                        card_scroll = 0;
                        return Ok(());
                    }
                    action = chosen;
                    let old = std::mem::replace(&mut screen, Screen::Decks);
                    if let Screen::Menu { previous, .. } = old {
                        screen = *previous;
                    }
                }
            }
            if let Screen::Settings { selected, .. } = &screen {
                if matches!(action, Action::Primary) {
                    let index = *selected;
                    let category =
                        SettingsCategory::ALL[index.min(SettingsCategory::ALL.len() - 1)];
                    let old = std::mem::replace(&mut screen, Screen::Decks);
                    if let Screen::Settings { selected, previous } = old {
                        let settings = Screen::Settings { selected, previous };
                        screen = match category {
                            SettingsCategory::Backups => Screen::SettingsBackups {
                                selected: 0,
                                backups: backup::list(),
                                status: None,
                                previous: Box::new(settings),
                            },
                            SettingsCategory::Language => Screen::SettingsChoice {
                                kind: ChoiceKind::Language,
                                selected: i18n::language_index(),
                                previous: Box::new(settings),
                            },
                            _ => Screen::SettingsCategory {
                                category,
                                selected: 0,
                                previous: Box::new(settings),
                            },
                        };
                    }
                    return Ok(());
                }
            }
            if let Screen::SettingsCategory {
                category, selected, ..
            } = &screen
            {
                let category = *category;
                let index = *selected;
                let items = settings_items(category);
                let item = items[index.min(items.len().saturating_sub(1))];
                match action {
                    Action::Primary => {
                        match item {
                            SettingsItem::Choice(kind) => {
                                let old = std::mem::replace(&mut screen, Screen::Decks);
                                if let Screen::SettingsCategory {
                                    category,
                                    selected,
                                    previous,
                                } = old
                                {
                                    screen = Screen::SettingsChoice {
                                        kind,
                                        selected: choice_selected(kind),
                                        previous: Box::new(Screen::SettingsCategory {
                                            category,
                                            selected,
                                            previous,
                                        }),
                                    };
                                }
                            }
                            SettingsItem::Toggle(kind) => apply_toggle(kind)?,
                            SettingsItem::Binding(action) => {
                                let old = std::mem::replace(&mut screen, Screen::Decks);
                                if let Screen::SettingsCategory {
                                    category,
                                    selected,
                                    previous,
                                } = old
                                {
                                    screen = Screen::SettingsButton {
                                        action,
                                        selected: controls::button_option_index(
                                            controls::button_for(action),
                                        ),
                                        previous: Box::new(Screen::SettingsCategory {
                                            category,
                                            selected,
                                            previous,
                                        }),
                                    };
                                }
                            }
                            SettingsItem::ResetControls => {
                                controls::reset()?;
                                let _ = i18n::set_scroll_speed_index(1);
                                event("CONTROLS_RESET");
                            }
                            SettingsItem::Toolbar => {
                                let old = std::mem::replace(&mut screen, Screen::Decks);
                                if let Screen::SettingsCategory {
                                    category,
                                    selected,
                                    previous,
                                } = old
                                {
                                    screen = Screen::SettingsToolbar {
                                        selected: 0,
                                        previous: Box::new(Screen::SettingsCategory {
                                            category,
                                            selected,
                                            previous,
                                        }),
                                    };
                                }
                            }
                            SettingsItem::Animations => {
                                let old = std::mem::replace(&mut screen, Screen::Decks);
                                if let Screen::SettingsCategory {
                                    category,
                                    selected,
                                    previous,
                                } = old
                                {
                                    screen = Screen::SettingsAnimations {
                                        selected: 0,
                                        previous: Box::new(Screen::SettingsCategory {
                                            category,
                                            selected,
                                            previous,
                                        }),
                                    };
                                }
                            }
                            SettingsItem::Action(Action::MediaSync, _) => {
                                if let Some(active) = &media_job {
                                    active.cancel();
                                    event("MEDIA_SYNC_CANCEL");
                                } else if auth.is_none() {
                                    media_result =
                                        Some(i18n::format("failed: {0}", &[&tr("Not logged in")]));
                                    media_notice_until = Instant::now() + Duration::from_secs(3600);
                                } else if !clock::connected_now() {
                                    event("WIFI_DISABLED media_sync");
                                    let old = std::mem::replace(&mut screen, Screen::Decks);
                                    if let Screen::SettingsCategory {
                                        category,
                                        selected,
                                        previous,
                                    } = old
                                    {
                                        screen = Screen::Info {
                                            title: tr("Wi-Fi disabled").into(),
                                            text: wifi_disabled_message(),
                                            previous: Box::new(Screen::SettingsCategory {
                                                category,
                                                selected,
                                                previous,
                                            }),
                                        };
                                    }
                                } else {
                                    let priority = backend
                                        .as_mut()
                                        .map(|active| active.priority_media(200))
                                        .unwrap_or_default();
                                    match backend.as_ref() {
                                        Some(active_backend) => {
                                            let index = active_backend.media_index_handle();
                                            match sync::start_media(
                                                auth.clone(),
                                                &login,
                                                priority,
                                                index,
                                            ) {
                                                Ok(job) => {
                                                    media_job = Some(job);
                                                    media_result = None;
                                                    last_media_changed = 0;
                                                    last_media_refresh = Instant::now();
                                                    event("MEDIA_SYNC_MANUAL_START");
                                                }
                                                Err(error) => {
                                                    event(&format!(
                                                        "MEDIA_SYNC_MANUAL_ERROR {error}"
                                                    ));
                                                    media_result = Some(i18n::format(
                                                        "failed: {0}",
                                                        &[&error],
                                                    ));
                                                    media_notice_until =
                                                        Instant::now() + Duration::from_secs(3600);
                                                }
                                            }
                                        }
                                        None => {
                                            media_result = Some(i18n::format(
                                                "failed: {0}",
                                                &[&tr("Collection unavailable")],
                                            ));
                                            media_notice_until =
                                                Instant::now() + Duration::from_secs(3600);
                                        }
                                    }
                                }
                            }
                            SettingsItem::Action(chosen, _) => {
                                action = chosen;
                            }
                        }
                        if !matches!(item, SettingsItem::Action(chosen, _) if !matches!(chosen, Action::MediaSync))
                        {
                            return Ok(());
                        }
                    }
                    Action::Left | Action::Right => {
                        let direction = if matches!(action, Action::Right) {
                            1
                        } else {
                            -1
                        };
                        match item {
                            SettingsItem::Choice(kind) => {
                                let count = choice_options(kind).len();
                                let current = choice_selected(kind);
                                let next = if direction < 0 {
                                    (current + count - 1) % count
                                } else {
                                    (current + 1) % count
                                };
                                apply_choice(kind, next)?;
                                if kind == ChoiceKind::FontScale {
                                    review_scroll = 0.0;
                                }
                                if kind == ChoiceKind::Language {
                                    if let Some(active) = backend.as_mut() {
                                        active.set_language(None)?;
                                    }
                                }
                            }
                            SettingsItem::Toggle(kind) => apply_toggle(kind)?,
                            SettingsItem::Animations => {
                                let value = i18n::toggle_animations()?;
                                event(&format!("ANIMATIONS {}", if value { "on" } else { "off" }));
                            }
                            SettingsItem::Action(_, _)
                            | SettingsItem::Binding(_)
                            | SettingsItem::ResetControls
                            | SettingsItem::Toolbar => {}
                        }
                        return Ok(());
                    }
                    _ => {}
                }
            }
            if let Screen::SettingsButton {
                action: review_action,
                selected,
                ..
            } = &screen
            {
                let review_action = *review_action;
                let index = *selected;
                let length = controls::button_option_count();
                match action {
                    Action::Primary => {
                        let chosen = controls::button_option(index);
                        match controls::try_assign(review_action, chosen)? {
                            None => {
                                event(&format!(
                                    "CONTROL {}={}",
                                    review_action.key(),
                                    chosen.map_or("none", controls::Button::key)
                                ));
                                let old = std::mem::replace(&mut screen, Screen::Decks);
                                if let Screen::SettingsButton { previous, .. } = old {
                                    screen = *previous;
                                }
                            }
                            Some(conflict) => {
                                let old = std::mem::replace(&mut screen, Screen::Decks);
                                if let Screen::SettingsButton {
                                    action,
                                    selected,
                                    previous,
                                } = old
                                {
                                    screen = Screen::Info {
                                        title: tr("Controls and keybinds").into(),
                                        text: i18n::format(
                                            "{0} is already used by {1} on the same screen.",
                                            &[
                                                tr(chosen
                                                    .map_or("Unbound", controls::Button::label)),
                                                tr(conflict.label()),
                                            ],
                                        ),
                                        previous: Box::new(Screen::SettingsButton {
                                            action,
                                            selected,
                                            previous,
                                        }),
                                    };
                                }
                            }
                        }
                        return Ok(());
                    }
                    Action::Back => {
                        let old = std::mem::replace(&mut screen, Screen::Decks);
                        if let Screen::SettingsButton { previous, .. } = old {
                            screen = *previous;
                        }
                        return Ok(());
                    }
                    Action::Up | Action::Down => {
                        if let Screen::SettingsButton { selected, .. } = &mut screen {
                            *selected =
                                list_step(*selected, length, matches!(action, Action::Down));
                        }
                        return Ok(());
                    }
                    _ => {}
                }
            }
            if let Screen::SettingsToolbar { selected, .. } = &screen {
                let index = *selected;
                match action {
                    Action::Primary => {
                        match index {
                            0 => {
                                let value = i18n::toggle_toolbar_visible()?;
                                event(&format!("TOOLBAR {}", if value { "on" } else { "off" }));
                            }
                            1..=4 => {
                                let item = i18n::toolbar_order()[index - 1];
                                let value = i18n::toggle_toolbar_item(item)?;
                                event(&format!(
                                    "TOOLBAR_ITEM {} {}",
                                    item.key(),
                                    if value { "on" } else { "off" }
                                ));
                            }
                            _ => {
                                let value = i18n::toggle_deck_title()?;
                                event(&format!("DECK_TITLE {}", if value { "on" } else { "off" }));
                            }
                        }
                        return Ok(());
                    }
                    Action::Left | Action::Right if (1..=4).contains(&index) => {
                        let delta = if matches!(action, Action::Right) {
                            1
                        } else {
                            -1
                        };
                        let target = i18n::move_toolbar_item(index - 1, delta)?;
                        if let Screen::SettingsToolbar { selected, .. } = &mut screen {
                            *selected = target + 1;
                        }
                        let order = i18n::toolbar_order();
                        let keys: Vec<&str> = order.iter().map(|item| item.key()).collect();
                        event(&format!("TOOLBAR_ORDER {}", keys.join(",")));
                        return Ok(());
                    }
                    _ => {}
                }
            }
            if let Screen::SettingsAnimations { selected, .. } = &mut screen {
                match action {
                    Action::Primary | Action::Left | Action::Right => {
                        if *selected == 0 {
                            let value = i18n::toggle_animations()?;
                            event(&format!("ANIMATIONS {}", if value { "on" } else { "off" }));
                        } else if let Some(kind) = i18n::AnimKind::ALL.get(*selected - 1).copied() {
                            let value = i18n::toggle_anim_item(kind)?;
                            event(&format!(
                                "ANIMATION_ITEM {} {}",
                                kind.key(),
                                if value { "on" } else { "off" }
                            ));
                        }
                        return Ok(());
                    }
                    Action::Up | Action::Down => {
                        *selected = list_step(*selected, 5, matches!(action, Action::Down));
                        return Ok(());
                    }
                    _ => {}
                }
            }
            if let Screen::SettingsChoice { kind, selected, .. } = &screen {
                let kind = *kind;
                let index = *selected;
                match action {
                    Action::Primary => {
                        apply_choice(kind, index)?;
                        if kind == ChoiceKind::FontScale {
                            review_scroll = 0.0;
                        }
                        if kind == ChoiceKind::Language {
                            if let Some(active) = backend.as_mut() {
                                active.set_language(None)?;
                            }
                        }
                        return Ok(());
                    }
                    Action::Left | Action::Right => {
                        let count = choice_options(kind).len();
                        let step = if matches!(action, Action::Right) {
                            1
                        } else {
                            -1
                        };
                        if let Screen::SettingsChoice { selected, .. } = &mut screen {
                            *selected =
                                ((*selected as i32 + step + count as i32) % count as i32) as usize;
                        }
                        return Ok(());
                    }
                    _ => {}
                }
            }
            if let Screen::SettingsBackups {
                selected,
                backups,
                status,
                ..
            } = &mut screen
            {
                match action {
                    Action::Up | Action::Down => {
                        *selected = list_step(
                            *selected,
                            backups.len() + 1,
                            matches!(action, Action::Down),
                        );
                        *status = None;
                        return Ok(());
                    }
                    Action::Primary => {
                        if *selected == 0 {
                            let b = backend.as_mut().ok_or("collection unavailable")?;
                            let ms = b.collection_modified_ms().unwrap_or(current_ms);
                            let entry = backup::create_from_backend(b, ms, None)?;
                            *backups = backup::list();
                            *status = Some(i18n::format("Backup created: {0}", &[&entry.label]));
                            event(&format!("BACKUP_CREATED {}", entry.filename));
                        } else if let Some(entry) = backups.get(*selected - 1).cloned() {
                            let old = std::mem::replace(&mut screen, Screen::Decks);
                            screen = Screen::BackupConfirm {
                                entry,
                                previous: Box::new(old),
                            };
                        }
                        return Ok(());
                    }
                    _ => {}
                }
            }
            let restore_entry = match &screen {
                Screen::BackupConfirm { entry, .. } if matches!(action, Action::Primary) => {
                    Some(entry.clone())
                }
                _ => None,
            };
            if let Some(entry) = restore_entry {
                if let Some(active) = media_job.take() {
                    active.cancel();
                }
                prefetch.stop();
                Ui::audio_stop();
                top_card.invalidate();
                if let Some(active) = backend.take() {
                    active.close()?;
                }
                if let Err(error) = backup::restore_file(&entry.path, std::path::Path::new(backend::PATH)) {
                    backend = Backend::open_with_progress(progress.clone()).ok();
                    return Err(error.into());
                }
                let mut reopened = match Backend::open_with_progress(progress.clone()) {
                    Ok(b) => b,
                    Err(error) => {
                        screen = Screen::Error(user_error(error.as_ref()));
                        return Ok(());
                    }
                };
                current_ms = reopened.collection_modified_ms().unwrap_or(0);
                let _ = i18n::save_backup_stamp(current_ms);
                root = reopened.deck_tree()?;
                collapsed.clear();
                decks.clear();
                collect_deck_items(&root, &collapsed, &mut decks);
                selected = 0;
                scroll_pos = 0;
                card_scroll = 0;
                review_scroll = 0.0;
                review_max_scroll = 0.0;
                review = ReviewState::default();
                last_autoplay_card = None;
                last_review_card = None;
                card_media_pending = false;
                playing_audio = None;
                audio_queue.clear();
                audio_queue_pos = 0;
                rating_feedback = None;
                leech_notice = None;
                last_answered = None;
                daily_studied = reopened.studied_today_text().unwrap_or_default();
                backend = Some(reopened);
                Ui::media_refresh();
                event(&format!("BACKUP_RESTORED {}", entry.filename));
                screen = Screen::Info {
                    title: tr("Backup restored").into(),
                    text: i18n::format("Restored collection from {0}.", &[&entry.label]),
                    previous: Box::new(Screen::Decks),
                };
                return Ok(());
            }
            let mut quickstart_finish = None;
            if let Screen::Quickstart { step, selected, .. } = &mut screen {
                if let Some(hit) = quickstart_touch {
                    match hit {
                        quickstart::TouchAction::SelectLanguage(idx) => {
                            *selected = idx;
                            i18n::set_language(idx)?;
                            if let Some(active) = backend.as_mut() {
                                active.set_language(None)?;
                            }
                            event(&format!("LANGUAGE {}", i18n::language()));
                            return Ok(());
                        }
                        quickstart::TouchAction::SelectRow(row) => {
                            *selected = row;
                            quickstart::apply_step_change(*step, row, true)?;
                            event(&format!("QUICKSTART_OPTION step={} row={}", *step, row));
                            return Ok(());
                        }
                        quickstart::TouchAction::Next => action = Action::Primary,
                        quickstart::TouchAction::Back => action = Action::Back,
                        quickstart::TouchAction::Skip => quickstart_finish = Some(0),
                        quickstart::TouchAction::Finish => quickstart_finish = Some(1),
                        quickstart::TouchAction::LoginSync => quickstart_finish = Some(2),
                        quickstart::TouchAction::None => return Ok(()),
                    }
                }
                if quickstart_finish.is_none() {
                    match action {
                        Action::Up | Action::Down | Action::Left | Action::Right => {
                            match *step {
                                0 => {
                                    let len = i18n::LANGUAGES.len();
                                    let next = match action {
                                        Action::Left => selected.saturating_sub(1),
                                        Action::Right => (*selected + 1).min(len - 1),
                                        Action::Up => selected.saturating_sub(3),
                                        Action::Down => (*selected + 3).min(len - 1),
                                        _ => *selected,
                                    };
                                    if next != *selected {
                                        *selected = next;
                                        i18n::set_language(next)?;
                                        if let Some(active) = backend.as_mut() {
                                            active.set_language(None)?;
                                        }
                                        event(&format!("LANGUAGE {}", i18n::language()));
                                    }
                                }
                                1 | 2 => match action {
                                    Action::Up | Action::Down => {
                                        let rows = if *step == 2 { 5 } else { 3 };
                                        *selected =
                                            list_step(*selected, rows, matches!(action, Action::Down));
                                    }
                                    Action::Left | Action::Right => {
                                        quickstart::apply_step_change(
                                            *step,
                                            *selected,
                                            matches!(action, Action::Right),
                                        )?;
                                        event(&format!(
                                            "QUICKSTART_OPTION step={} row={}",
                                            *step, *selected
                                        ));
                                    }
                                    _ => {}
                                },
                                _ => {
                                    *selected = 1 - (*selected).min(1);
                                }
                            }
                            return Ok(());
                        }
                        Action::Primary => {
                            if *step + 1 < quickstart::STEPS {
                                *step += 1;
                                *selected = 0;
                                ui_anim.trigger_menu_transition();
                                event(&format!("QUICKSTART_STEP {}", *step));
                                return Ok(());
                            } else {
                                quickstart_finish = Some(if *selected == 0 { 1 } else { 2 });
                            }
                        }
                        Action::Back => {
                            if *step > 0 {
                                *step -= 1;
                                *selected = if *step == 0 {
                                    i18n::language_index()
                                } else {
                                    0
                                };
                                ui_anim.trigger_menu_transition();
                                event(&format!("QUICKSTART_STEP {}", *step));
                                return Ok(());
                            } else {
                                quickstart_finish = Some(0);
                            }
                        }
                        _ => return Ok(()),
                    }
                }
            }
            if let Some(outcome) = quickstart_finish {
                i18n::set_quickstart_done(true)?;
                match outcome {
                    0 => {
                        event("QUICKSTART_SKIP");
                        let old = std::mem::replace(&mut screen, Screen::Decks);
                        if let Screen::Quickstart { previous, .. } = old {
                            screen = *previous;
                        }
                        return Ok(());
                    }
                    1 => {
                        event("QUICKSTART_FINISH");
                        screen = Screen::Decks;
                        return Ok(());
                    }
                    _ => {
                        event("QUICKSTART_LOGIN_SYNC");
                        screen = Screen::Decks;
                        action = Action::Login;
                    }
                }
            }
            let mut bury = None;
            if let Screen::CardChoice {
                card_id, selected, ..
            } = &screen
            {
                let card_id = *card_id;
                let selected = *selected;
                match action {
                    Action::Up | Action::Down | Action::Left | Action::Right => {
                        let choice = if matches!(action, Action::Down | Action::Right) {
                            1
                        } else {
                            0
                        };
                        let old = std::mem::replace(&mut screen, Screen::Decks);
                        if let Screen::CardChoice {
                            card_id, previous, ..
                        } = old
                        {
                            screen = Screen::CardChoice {
                                card_id,
                                selected: choice,
                                previous,
                            };
                        }
                        return Ok(());
                    }
                    Action::Back => {
                        let old = std::mem::replace(&mut screen, Screen::Decks);
                        if let Screen::CardChoice { previous, .. } = old {
                            screen = *previous;
                        }
                        card_scroll = 0;
                        return Ok(());
                    }
                    Action::Primary => bury = Some((card_id, selected)),
                    _ => {}
                }
            }
            if let Some((card_id, selected)) = bury {
                let mode = if selected == 0 { 2 } else { 0 };
                let b = backend.as_mut().ok_or("collection unavailable")?;
                b.bury_or_suspend(card_id, mode)?;
                event(&format!("BURY card={card_id} mode={mode}"));
                let deck_name = deck_name.clone();
                screen = match b.next()? {
                    Some(card) => {
                        ui_anim.trigger_next_card();
                        Screen::Question { card, deck_name }
                    }
                    None => Screen::Empty { deck_name },
                };
                card_scroll = 0;
                return Ok(());
            }
            let mut full_mode = None;
            if let Screen::SyncChoice {
                upload_ok,
                download_ok,
                download,
                confirmed,
            } = &mut screen
            {
                match action {
                    Action::Back => {
                        screen = Screen::Decks;
                    }
                    Action::Up | Action::Down | Action::Left | Action::Right => {
                        let direction = matches!(action, Action::Down | Action::Right);
                        if if direction { *download_ok } else { *upload_ok } {
                            *download = direction;
                            *confirmed = false;
                        }
                    }
                    Action::Primary if if *download { *download_ok } else { *upload_ok } => {
                        if *confirmed {
                            full_mode = Some(if *download {
                                sync::Mode::Download
                            } else {
                                sync::Mode::Upload
                            });
                        } else {
                            *confirmed = true;
                        }
                    }
                    _ => {}
                }
                if full_mode.is_none() {
                    return Ok(());
                }
            }
            if matches!(screen, Screen::Error(_)) && matches!(action, Action::Primary) {
                action = Action::Clock;
            }
            if matches!(action, Action::Sync | Action::Login) || full_mode.is_some() {
                if !clock::connected_now() {
                    event("WIFI_DISABLED sync");
                    let previous = std::mem::replace(&mut screen, Screen::Decks);
                    screen = Screen::Info {
                        title: tr("Wi-Fi disabled").into(),
                        text: wifi_disabled_message(),
                        previous: Box::new(previous),
                    };
                    return Ok(());
                }
                let offset = match clock::configure(
                    clock::needs_refresh(),
                    &apt,
                    &mut hid,
                    clock_job.take(),
                ) {
                    Ok(offset) => offset,
                    Err(error) => {
                        event(&format!("SYNC_ERROR {error}"));
                        screen = Screen::Info {
                            title: tr("Sync error").into(),
                            text: error,
                            previous: Box::new(Screen::Decks),
                        };
                        return Ok(());
                    }
                };
                backend
                    .as_mut()
                    .ok_or("collection unavailable")?
                    .set_clock_offset(offset)?;
                if auth.is_none() {
                    if login.username.is_empty() {
                        let Some(username) = Ui::keyboard(
                            if login.is_test() {
                                tr("Test server username (dev)")
                            } else {
                                tr("AnkiWeb username")
                            },
                            "",
                        )?
                        else {
                            return Ok(());
                        };
                        login.username = username;
                    }
                    if login.password.is_empty() {
                        let Some(password) = Ui::keyboard_secret(if login.is_test() {
                            tr("Test server password (disposable)")
                        } else {
                            tr("AnkiWeb password")
                        })?
                        else {
                            return Ok(());
                        };
                        login.password = password;
                    }
                }
                if let Some(active) = media_job.take() {
                    active.cancel();
                }
                media_result = None;
                media_notice_until = Instant::now();
                if full_mode.is_some() {
                    if let Some(b) = backend.as_mut() {
                        let ms = b.collection_modified_ms().unwrap_or(current_ms);
                        match backup::create_from_backend(b, ms, None) {
                            Ok(entry) => event(&format!("BACKUP_AUTO full_sync {}", entry.filename)),
                            Err(error) => event(&format!(
                                "BACKUP_AUTO_ERROR full_sync {}",
                                error_text(error.as_ref())
                            )),
                        }
                    }
                }
                let collection = backend.take().ok_or("collection unavailable")?;
                match sync::Job::start(
                    collection,
                    auth.clone(),
                    &login,
                    full_mode.unwrap_or(if matches!(action, Action::Login) {
                        sync::Mode::Login
                    } else {
                        sync::Mode::Normal
                    }),
                    progress.clone(),
                ) {
                    Ok(active) => {
                        job = Some(active);
                        screen = Screen::SyncWait;
                        event("SYNC_START");
                    }
                    Err(e) => {
                        backend = Backend::open_with_progress(progress.clone()).ok();
                        return Err(e.into());
                    }
                }
                unsafe {
                    login.password.as_bytes_mut().fill(0);
                }
                login.password.clear();
                return Ok(());
            }
            if let Some(cmd) = script_command {
                let mut parts = cmd.splitn(3, ' ');
                let name = parts.next().unwrap();
                let index = parts.next().unwrap_or("");
                let value = parts.next().unwrap_or("");
                match (&mut screen, name) {
                    (Screen::Editor { draft, .. }, "FIELD") => {
                        let index: usize = index.parse()?;
                        draft.note.set_field(index, value)?;
                    }
                    (Screen::Options { options, .. }, "LIMITS") => {
                        options.new = index.parse()?;
                        options.reviews = value.parse()?;
                    }
                    (Screen::Browser(_), "SEARCH") => {
                        screen = Screen::Browser(Browser::load(
                            backend.as_mut().ok_or("collection unavailable")?,
                            cmd.strip_prefix("SEARCH ").unwrap_or("").into(),
                        )?);
                    }
                    _ => return Err("test script command does not match screen".into()),
                }
            }
            if matches!(action, Action::Menu) {
                let previous = std::mem::replace(&mut screen, Screen::Decks);
                screen = if let Screen::Menu { previous, .. } = previous {
                    *previous
                } else {
                    Screen::Menu {
                        selected: 0,
                        previous: Box::new(previous),
                    }
                };
                card_scroll = 0;
                return Ok(());
            }
            match action {
                Action::Sync | Action::Login => unreachable!(),
                Action::Logout => {
                    auth_store::clear(std::path::Path::new(auth_store::DIRECTORY))?;
                    auth = None;
                    login.username.clear();
                    event("LOGOUT_COMPLETE");
                    screen = Screen::Info {
                        title: tr("Logged out").into(),
                        text: tr("Saved login removed from this SD card.").into(),
                        previous: Box::new(Screen::Decks),
                    };
                }
                Action::Language => {
                    i18n::toggle()?;
                    let card = match &mut screen {
                        Screen::Question { card, .. } | Screen::Answer { card, .. } => Some(card),
                        _ => None,
                    };
                    backend
                        .as_mut()
                        .ok_or("collection unavailable")?
                        .set_language(card)?;
                }
                Action::Clock => {
                    if !clock::connected_now() {
                        event("WIFI_DISABLED clock");
                        let previous = std::mem::replace(&mut screen, Screen::Decks);
                        screen = Screen::Info {
                            title: tr("Wi-Fi disabled").into(),
                            text: wifi_disabled_message(),
                            previous: Box::new(previous),
                        };
                        return Ok(());
                    }
                    if media_job
                        .as_ref()
                        .is_some_and(media_sync::MediaSync::is_running)
                    {
                        let previous = std::mem::replace(&mut screen, Screen::Decks);
                        screen = Screen::Info {
                            title: tr("Update clock").into(),
                            text: tr("Media sync is running. Try again when it finishes.").into(),
                            previous: Box::new(previous),
                        };
                        return Ok(());
                    }
                    let offset = match clock::configure(true, &apt, &mut hid, clock_job.take()) {
                        Ok(offset) => offset,
                        Err(error) => {
                            let previous = std::mem::replace(&mut screen, Screen::Decks);
                            screen = Screen::Info {
                                title: tr("Update clock").into(),
                                text: i18n::format("Clock setup failed: {0}", &[&error]),
                                previous: Box::new(previous),
                            };
                            return Ok(());
                        }
                    };
                    if backend.is_none() {
                        backend = Some(Backend::open_with_progress(progress.clone())?);
                        root = backend.as_mut().unwrap().deck_tree()?;
                        decks.clear();
                        collect_deck_items(&root, &collapsed, &mut decks);
                        screen = Screen::Decks;
                    }
                    backend.as_mut().unwrap().set_clock_offset(offset)?;
                }
                Action::Quickstart => {
                    let previous = std::mem::replace(&mut screen, Screen::Decks);
                    screen = Screen::Quickstart {
                        step: 0,
                        selected: i18n::language_index(),
                        previous: Box::new(previous),
                    };
                    card_scroll = 0;
                }
                Action::Help => {
                    let previous = std::mem::replace(&mut screen, Screen::Decks);
                    screen = Screen::Info {
                        title: tr("Help").into(),
                        text: tr("Decks: UP/DOWN selects. A studies. LEFT/RIGHT expands or collapses.\nReview: use the buttons assigned in Settings > Controls and keybinds.\nScroll the top screen with the assigned scroll buttons or the Circle Pad.\nSELECT opens Menu. B returns. START closes and exits.\nSettings also has appearance, audio, controls and account options.\nAdd, Edit and Browse manage notes. Save confirms changes; Cancel discards them.").into(),
                        previous: Box::new(previous),
                    };
                    card_scroll = 0;
                }
                Action::Stats => {
                    if let Some(active) = &media_job {
                        active.set_paused(true);
                    }
                    let stats_result = match backend.as_mut() {
                        Some(b) => b.statistics(),
                        None => Err("collection unavailable".into()),
                    };
                    if let Some(active) = &media_job {
                        active.set_paused(matches!(
                            screen,
                            Screen::Question { .. } | Screen::Answer { .. }
                        ));
                    }
                    let data = stats_result?;
                    let previous = std::mem::replace(&mut screen, Screen::Decks);
                    screen = Screen::Stats {
                        data: Box::new(data),
                        page: stats::Page::Overview,
                        timeframe: stats::Timeframe::OneMonth,
                        previous: Box::new(previous),
                    };
                    card_scroll = 0;
                }
                Action::Add => {
                    screen = Screen::Editor {
                        draft: backend
                            .as_mut()
                            .ok_or("collection unavailable")?
                            .new_draft(did)?,
                        field: 0,
                        return_query: None,
                    };
                    card_scroll = 0;
                }
                Action::Browse => {
                    screen = Screen::Browser(Browser::load(
                        backend.as_mut().ok_or("collection unavailable")?,
                        String::new(),
                    )?);
                    card_scroll = 0;
                }
                Action::Edit => {
                    let (id, query) = match &screen {
                        Screen::Question { card, .. } | Screen::Answer { card, .. } => {
                            (Some(card.id), None)
                        }
                        Screen::Browser(b) => {
                            (b.ids.get(b.selected).copied(), Some(b.query.clone()))
                        }
                        _ => (None, None),
                    };
                    if let Some(id) = id {
                        screen = Screen::Editor {
                            draft: backend
                                .as_mut()
                                .ok_or("collection unavailable")?
                                .edit_draft(id)?,
                            field: 0,
                            return_query: query,
                        };
                    } else {
                        screen = Screen::Browser(Browser::load(
                            backend.as_mut().ok_or("collection unavailable")?,
                            format!(
                                "deck:\"{}\"",
                                deck_name.replace('\\', "\\\\").replace('"', "\\\"")
                            ),
                        )?);
                    }
                    card_scroll = 0;
                }
                Action::Search => {
                    let initial = match &screen {
                        Screen::Browser(b) => Some(b.query.clone()),
                        _ => None,
                    };
                    if let Some(initial) = initial {
                        if let Some(query) = Ui::keyboard(tr("Search"), &initial)
                            .map_err(|e| -> Box<dyn std::error::Error> { e.into() })?
                        {
                            screen = Screen::Browser(Browser::load(
                                backend.as_mut().ok_or("collection unavailable")?,
                                query,
                            )?);
                            card_scroll = 0;
                        }
                    }
                }
                Action::Filter(index) => {
                    let query = browse_filter_query(index, &deck_name);
                    screen = Screen::Browser(Browser::load(
                        backend.as_mut().ok_or("collection unavailable")?,
                        query,
                    )?);
                    card_scroll = 0;
                }
                Action::Restore => {
                    let target = match &screen {
                        Screen::Browser(b) => {
                            Some((b.ids.get(b.selected).copied(), b.query.clone()))
                        }
                        _ => None,
                    };
                    if let Some((Some(id), query)) = target {
                        let b = backend.as_mut().ok_or("collection unavailable")?;
                        b.restore_card(id)?;
                        event(&format!("RESTORE card={id}"));
                        screen = Screen::Browser(Browser::load(b, query)?);
                    }
                }
                Action::Options => {
                    screen = Screen::Options {
                        options: backend
                            .as_mut()
                            .ok_or("collection unavailable")?
                            .options(did)?,
                        field: 1,
                    };
                    card_scroll = 0;
                }
                Action::Undo => {
                    let review_deck = match &screen {
                        Screen::Question { deck_name, .. }
                        | Screen::Answer { deck_name, .. }
                        | Screen::Empty { deck_name } => Some(deck_name.clone()),
                        Screen::Menu { previous, .. } => match &**previous {
                            Screen::Question { deck_name, .. }
                            | Screen::Answer { deck_name, .. }
                            | Screen::Empty { deck_name } => Some(deck_name.clone()),
                            _ => None,
                        },
                        _ => None,
                    };
                    let b = backend.as_mut().ok_or("collection unavailable")?;
                    match (review_deck, last_answered.take()) {
                        (Some(deck_name), Some(card_id)) => {
                            b.undo()?;
                            event(&format!("UNDO_REVIEW card={card_id}"));
                            screen = match b.reopen_card(card_id)? {
                                Some(card) => {
                                    ui_anim.trigger_next_card();
                                    Screen::Question { card, deck_name }
                                }
                                None => Screen::Empty { deck_name },
                            };
                        }
                        _ => {
                            b.undo()?;
                            event("UNDO_OK");
                            screen = Screen::Decks;
                        }
                    }
                    card_scroll = 0;
                }
                Action::Bury => {
                    let card_id = match &screen {
                        Screen::Question { card, .. } | Screen::Answer { card, .. } => {
                            Some(card.id)
                        }
                        _ => None,
                    };
                    let previous = std::mem::replace(&mut screen, Screen::Decks);
                    screen = match card_id {
                        Some(card_id) => Screen::CardChoice {
                            card_id,
                            selected: 0,
                            previous: Box::new(previous),
                        },
                        None => Screen::Info {
                            title: tr("Bury / Suspend").into(),
                            text: tr("Open a card to bury or suspend.").into(),
                            previous: Box::new(previous),
                        },
                    };
                    card_scroll = 0;
                }
                Action::PlayAudio(index) => {
                    let (card, answer) = match &screen {
                        Screen::Question { card, .. } => (card, false),
                        Screen::Answer { card, .. } => (card, true),
                        _ => return Ok(()),
                    };
                    let audio = match index {
                        Some(_) => card_audio(card, answer),
                        None => replay_audio(card, answer),
                    };
                    if audio.is_empty() {
                        return Ok(());
                    }
                    let mut queue = Vec::with_capacity(audio.len());
                    for item in &audio {
                        queue.push((item.filename.clone(), audio_path(backend.as_ref(), item)));
                    }
                    let chosen = match index {
                        Some(value) => value.min(queue.len() - 1),
                        None => {
                            let playing = playing_audio.is_some() && Ui::audio_is_playing();
                            if playing && audio_queue_pos + 1 < queue.len() {
                                audio_queue_pos + 1
                            } else {
                                0
                            }
                        }
                    };
                    audio_queue = queue;
                    audio_queue_pos = chosen;
                    let (filename, resolved) = audio_queue[chosen].clone();
                    if let Some(side_pos) = card_audio(card, answer)
                        .iter()
                        .position(|item| item.filename == filename)
                    {
                        audio_index = side_pos;
                        audio_page = side_pos / AUDIO_VISIBLE;
                    }
                    match Ui::audio_play(&resolved) {
                        AudioAttempt::Done(true) => {
                            if let Some(active) = backend.as_ref() {
                                active.mark_media_present(&filename);
                            }
                            playing_audio = Some(filename.clone());
                            event(&format!(
                                "AUDIO play index={chosen} ok=true file={filename}"
                            ));
                        }
                        AudioAttempt::Done(false) => {
                            if let AudioAttempt::Done(decoded) = Ui::audio_decode(&resolved) {
                                let diag = media_diag(&resolved);
                                log_media_once(
                                    &format!("audio|{resolved}|{diag}"),
                                    &format!(
                                        "AUDIO play index={chosen} ok=false decode={decoded} file={filename} {diag}"
                                    ),
                                );
                            }
                            audio_queue.clear();
                            audio_queue_pos = 0;
                        }
                        AudioAttempt::Busy => {
                            audio_queue.clear();
                            audio_queue_pos = 0;
                        }
                    }
                }
                Action::AudioPage(delta) => {
                    let total = match &screen {
                        Screen::Question { card, .. } => card_audio(card, false).len(),
                        Screen::Answer { card, .. } => card_audio(card, true).len(),
                        _ => 0,
                    };
                    let pages = audio_page_count(total);
                    audio_page = (audio_page as i32 + delta).clamp(0, pages as i32 - 1) as usize;
                }
                Action::TypeAnswer => {
                    let (field, initial) = match &screen {
                        Screen::Question { card, .. } => match &card.type_answer {
                            Some(answer) => (
                                answer.field.clone(),
                                review.typed.clone().unwrap_or_default(),
                            ),
                            None => return Ok(()),
                        },
                        _ => return Ok(()),
                    };
                    if let Some(value) = Ui::keyboard(&field, &initial)
                        .map_err(|e| -> Box<dyn std::error::Error> { e.into() })?
                    {
                        review.typed = Some(value);
                        event("TYPE_ANSWER typed");
                    }
                }
                Action::HintToggle(id) => {
                    review.toggle_hint(id);
                    event(&format!(
                        "HINT toggle={id} open={}",
                        review.hints_open.contains(&id)
                    ));
                }
                Action::MediaToggle => {
                    let items = top_card.media_items();
                    if let Some(anim) =
                        items.get(review.media_sel.min(items.len().saturating_sub(1)))
                    {
                        if anim.supported {
                            review.media_playing = !review.media_playing;
                            if review.media_playing && Ui::anim_finished(anim.id) {
                                Ui::anim_restart(anim.id);
                            }
                            Ui::anim_set_playing(anim.id, review.media_playing);
                            event(&format!("ANIM toggle playing={}", review.media_playing));
                        }
                    }
                }
                Action::MediaReplay => {
                    let items = top_card.media_items();
                    if let Some(anim) =
                        items.get(review.media_sel.min(items.len().saturating_sub(1)))
                    {
                        if anim.supported {
                            Ui::anim_restart(anim.id);
                            Ui::anim_set_playing(anim.id, true);
                            review.media_playing = true;
                            event("ANIM replay");
                        }
                    }
                }
                Action::MediaSelect(delta) => {
                    let len = top_card.media_items().len();
                    if len > 0 {
                        let previous = review.media_sel;
                        review.media_sel =
                            (review.media_sel as i32 + delta).rem_euclid(len as i32) as usize;
                        if review.media_playing && previous != review.media_sel {
                            if let Some(old) = top_card.media_items().get(previous) {
                                if old.supported {
                                    Ui::anim_set_playing(old.id, false);
                                }
                            }
                            if let Some(new) = top_card.media_items().get(review.media_sel) {
                                if new.supported {
                                    if Ui::anim_finished(new.id) {
                                        Ui::anim_restart(new.id);
                                    }
                                    Ui::anim_set_playing(new.id, true);
                                }
                            }
                        }
                    }
                }
                Action::FullSync => {
                    screen = Screen::SyncChoice {
                        upload_ok: true,
                        download_ok: true,
                        download: false,
                        confirmed: false,
                    };
                    card_scroll = 0;
                }
                Action::Back => {
                    let old = std::mem::replace(&mut screen, Screen::Decks);
                    screen = match old {
                        Screen::Info { previous, .. }
                        | Screen::Menu { previous, .. }
                        | Screen::Settings { previous, .. }
                        | Screen::SettingsCategory { previous, .. }
                        | Screen::SettingsChoice { previous, .. }
                        | Screen::SettingsButton { previous, .. }
                        | Screen::SettingsToolbar { previous, .. }
                        | Screen::SettingsAnimations { previous, .. }
                        | Screen::SettingsBackups { previous, .. }
                        | Screen::BackupConfirm { previous, .. }
                        | Screen::CardChoice { previous, .. }
                        | Screen::Stats { previous, .. } => *previous,
                        Screen::Editor {
                            return_query: Some(query),
                            ..
                        } => Screen::Browser(Browser::load(
                            backend.as_mut().ok_or("collection unavailable")?,
                            query,
                        )?),
                        _ => Screen::Decks,
                    };
                    card_scroll = 0;
                }
                Action::Save => {
                    match &mut screen {
                        Screen::Editor {
                            draft,
                            return_query,
                            ..
                        } => {
                            let b = backend.as_mut().ok_or("collection unavailable")?;
                            let id = b.save_draft(draft)?;
                            event(&format!("NOTE_SAVED id={id}"));
                            screen = if let Some(query) = return_query.clone() {
                                Screen::Browser(Browser::load(b, query)?)
                            } else {
                                Screen::Decks
                            };
                        }
                        Screen::Options { options, .. } => {
                            backend
                                .as_mut()
                                .ok_or("collection unavailable")?
                                .save_options(options)?;
                            event("OPTIONS_SAVED");
                            screen = Screen::Decks;
                        }
                        _ => {}
                    }
                    card_scroll = 0;
                }
                Action::Field | Action::Primary | Action::Reveal | Action::Rate(_) => {
                    match &mut screen {
                        Screen::Decks if !decks.is_empty() => {
                            last_answered = None;
                            screen = match backend
                                .as_mut()
                                .ok_or("collection unavailable")?
                                .select(did)?
                            {
                                Some(card) => {
                                    ui_anim.trigger_next_card();
                                    Screen::Question { card, deck_name }
                                }
                                None => Screen::Empty { deck_name },
                            };
                            card_scroll = 0;
                        }
                        Screen::Question { .. } => {
                            let old = std::mem::replace(&mut screen, Screen::Decks);
                            if let Screen::Question { card, deck_name } = old {
                                // Compare the typed answer once and cache the
                                // reveal HTML for the answer screen.
                                if let Some(answer) = card.type_answer.clone() {
                                    if let Some(active) = backend.as_mut() {
                                        let typed = review.typed.clone().unwrap_or_default();
                                        let compared = if answer.ignore_case {
                                            active.compare_answer_full(
                                                &answer.expected,
                                                &typed,
                                                answer.combining,
                                                true,
                                            )
                                        } else {
                                            active.compare_answer(
                                                &answer.expected,
                                                &typed,
                                                answer.combining,
                                            )
                                        };
                                        let base = card
                                            .cloze_answer_html
                                            .as_deref()
                                            .unwrap_or(&card.answer_html);
                                        review.revealed = Some(RevealedHtml {
                                            card_id: card.id,
                                            answer: backend::fill_type_answer(base, &compared),
                                        });
                                        event("TYPE_ANSWER compared");
                                    }
                                }
                                screen = Screen::Answer { card, deck_name };
                                ui_anim.trigger_card_reveal();
                                sfx::play(sfx::SfxKind::Reveal);
                            }
                            card_scroll = 0;
                            event("QUESTION -> ANSWER");
                        }
                        Screen::Answer { .. }
                            if matches!(action, Action::Primary | Action::Rate(_)) =>
                        {
                            let rating = if let Action::Rate(r) = action { r } else { 2 };
                            if !(0..4).contains(&rating) {
                                return Err("invalid rating".into());
                            }
                            let old = std::mem::replace(&mut screen, Screen::Decks);
                            if let Screen::Answer { card, deck_name } = old {
                                let b = backend.as_mut().ok_or("collection unavailable")?;
                                let outcome = b.answer(card, rating)?;
                                last_answered = Some(outcome.card_id);
                                rating_feedback = Some((rating, Instant::now()));
                                if let Some(leech) = outcome.leech {
                                    leech_notice = Some((leech_message(leech), Instant::now()));
                                }
                                event(&format!(
                                    "REVIEW card={} rating={rating} leech={:?}",
                                    outcome.card_id, outcome.leech
                                ));
                                screen = match b.next()? {
                                    Some(card) => {
                                        sfx::play_rating(rating);
                                        ui_anim.trigger_next_card();
                                        Screen::Question { card, deck_name }
                                    }
                                    None => Screen::Empty { deck_name },
                                };
                            }
                            card_scroll = 0;
                        }
                        Screen::Browser(b) => {
                            b.refresh(backend.as_mut().ok_or("collection unavailable")?)?;
                            card_scroll = 0;
                        }
                        Screen::Info { .. } | Screen::Empty { .. } => {
                            let old = std::mem::replace(&mut screen, Screen::Decks);
                            if let Screen::Info { previous, .. } = old {
                                screen = *previous;
                            }
                            card_scroll = 0;
                        }
                        Screen::Editor { draft, field, .. } => {
                            let (label, initial) = if *field < draft.labels.len() {
                                (
                                    draft.labels[*field].clone(),
                                    draft.note.fields()[*field].clone(),
                                )
                            } else {
                                (
                                    tr("Tags (space-separated)").into(),
                                    draft.note.tags.join(" "),
                                )
                            };
                            if initial.chars().count() > 2047 {
                                return Err(
                                    tr("Field too long for keyboard. It was not changed.").into()
                                );
                            }
                            if let Some(value) = Ui::keyboard(&label, &initial)
                                .map_err(|e| -> Box<dyn std::error::Error> { e.into() })?
                            {
                                if *field < draft.labels.len() {
                                    draft.note.set_field(*field, value)?;
                                } else {
                                    draft.note.tags =
                                        value.split_whitespace().map(str::to_owned).collect();
                                }
                            }
                        }
                        Screen::Options { options, field } => {
                            let kind = option_rows(options)
                                .get(*field)
                                .map(|row| row.2)
                                .unwrap_or(OptField::Section);
                            match kind {
                                OptField::Section => {}
                                OptField::LimitScope => {
                                    options.set_scope(options.limit_scope.cycle(1))
                                }
                                OptField::NewLimit
                                | OptField::ReviewLimit
                                | OptField::LeechThreshold
                                | OptField::Retention => {
                                    let (label, initial) = match kind {
                                        OptField::NewLimit => {
                                            (tr("New cards/day"), options.new.to_string())
                                        }
                                        OptField::ReviewLimit => {
                                            (tr("Maximum reviews/day"), options.reviews.to_string())
                                        }
                                        OptField::LeechThreshold => (
                                            tr("Leech threshold"),
                                            options.leech_threshold.to_string(),
                                        ),
                                        _ => (
                                            tr("Desired retention (%)"),
                                            ((options.desired_retention * 100.0).round() as i32)
                                                .to_string(),
                                        ),
                                    };
                                    if let Some(value) = Ui::keyboard(label, &initial)
                                        .map_err(|e| -> Box<dyn std::error::Error> { e.into() })?
                                    {
                                        let (min, max, message) = match kind {
                                            OptField::NewLimit | OptField::ReviewLimit => (
                                                0,
                                                9999,
                                                tr("Enter a whole number from 0 to 9999."),
                                            ),
                                            OptField::LeechThreshold => {
                                                (1, 99, tr("Enter a whole number from 1 to 99."))
                                            }
                                            _ => {
                                                (50, 99, tr("Enter a whole number from 50 to 99."))
                                            }
                                        };
                                        let value = parse_limited_u32(&value, min, max, message)?;
                                        match kind {
                                            OptField::NewLimit => options.new = value,
                                            OptField::ReviewLimit => options.reviews = value,
                                            OptField::LeechThreshold => {
                                                options.leech_threshold = value
                                            }
                                            _ => options.desired_retention = (value as f32) / 100.0,
                                        }
                                    }
                                }
                                OptField::LearnSteps | OptField::RelearnSteps => {
                                    let (label, current) = if kind == OptField::LearnSteps {
                                        (tr("Learning steps"), format_steps(&options.learn_steps))
                                    } else {
                                        (
                                            tr("Relearning steps"),
                                            format_steps(&options.relearn_steps),
                                        )
                                    };
                                    if let Some(value) = Ui::keyboard(label, &current)
                                        .map_err(|e| -> Box<dyn std::error::Error> { e.into() })?
                                    {
                                        let steps = parse_steps(&value).ok_or_else(
                                            || -> Box<dyn std::error::Error> {
                                                tr("Invalid steps, e.g. 10m").into()
                                            },
                                        )?;
                                        if kind == OptField::LearnSteps {
                                            options.learn_steps = steps
                                        } else {
                                            options.relearn_steps = steps
                                        }
                                    }
                                }
                                OptField::InsertOrder => {
                                    options.insert_order = (options.insert_order + 1) % 2
                                }
                                OptField::LeechAction => {
                                    options.leech_action = (options.leech_action + 1) % 2
                                }
                                OptField::NewMix => options.new_mix = (options.new_mix + 1) % 3,
                                OptField::Fsrs => options.fsrs = !options.fsrs,
                                OptField::BuryNew => options.bury_new = !options.bury_new,
                                OptField::BuryReviews => {
                                    options.bury_reviews = !options.bury_reviews
                                }
                            }
                        }
                        _ => {}
                    }
                }
                Action::Up | Action::Down => {
                    let next = |index: usize, length: usize| {
                        if matches!(action, Action::Down) {
                            (index + 1).min(length.saturating_sub(1))
                        } else {
                            index.saturating_sub(1)
                        }
                    };
                    if matches!(screen, Screen::Question { .. } | Screen::Answer { .. }) {
                        let step = 19.0 * i18n::font_scale_factor() * i18n::scroll_speed_factor();
                        let delta = if matches!(action, Action::Down) {
                            step
                        } else {
                            -step
                        };
                        review_scroll = (review_scroll + delta).clamp(0.0, review_max_scroll);
                    } else {
                        match &mut screen {
                            Screen::Decks => {
                                selected = next(selected, decks.len());
                                if selected < scroll_pos {
                                    scroll_pos = selected
                                } else if selected >= scroll_pos + ROWS {
                                    scroll_pos = selected + 1 - ROWS
                                };
                            }
                            Screen::Browser(b) => {
                                b.selected = next(b.selected, b.ids.len());
                                b.refresh(backend.as_mut().ok_or("collection unavailable")?)?;
                                card_scroll = 0;
                            }
                            Screen::Editor { draft, field, .. } => {
                                *field = next(*field, draft.labels.len() + 1);
                                card_scroll = 0;
                            }
                            Screen::Options { options, field } => {
                                let rows = option_rows(options);
                                let length = rows.len() as i32;
                                let step = if matches!(action, Action::Down) {
                                    1
                                } else {
                                    -1
                                };
                                let mut index = *field as i32;
                                loop {
                                    index += step;
                                    if index < 0 {
                                        index = 0;
                                        break;
                                    }
                                    if index >= length {
                                        index = length - 1;
                                        break;
                                    }
                                    if rows[index as usize].2 != OptField::Section {
                                        break;
                                    }
                                }
                                *field = index as usize;
                            }
                            Screen::Menu { selected, .. } => {
                                let length = MAIN_MENU.len();
                                *selected = if matches!(action, Action::Down) {
                                    (*selected + MENU_COLS).min(length - 1)
                                } else {
                                    selected.saturating_sub(MENU_COLS)
                                };
                            }
                            Screen::Settings { selected, .. } => {
                                *selected = list_step(
                                    *selected,
                                    SettingsCategory::ALL.len(),
                                    matches!(action, Action::Down),
                                );
                            }
                            Screen::SettingsCategory {
                                category, selected, ..
                            } => {
                                *selected = list_step(
                                    *selected,
                                    settings_row_count(*category),
                                    matches!(action, Action::Down),
                                );
                            }
                            Screen::SettingsChoice { kind, selected, .. } => {
                                *selected = list_step(
                                    *selected,
                                    choice_options(*kind).len(),
                                    matches!(action, Action::Down),
                                );
                            }
                            Screen::SettingsToolbar { selected, .. } => {
                                *selected = list_step(
                                    *selected,
                                    TOOLBAR_ROWS,
                                    matches!(action, Action::Down),
                                );
                            }
                            Screen::Stats { page, timeframe, .. } => {
                                *page = page.step(matches!(action, Action::Down));
                                event(&format!(
                                    "STATS_PAGE {} range={}",
                                    *page as usize, *timeframe as usize
                                ));
                            }
                            _ => {
                                card_scroll = next(card_scroll, top_max + 1);
                            }
                        }
                    }
                }
                Action::Left | Action::Right => {
                    if matches!(screen, Screen::Decks) {
                        if let Some(deck) = decks.get(selected) {
                            if deck.has_children {
                                let id = deck.id;
                                if matches!(action, Action::Left) {
                                    collapsed.insert(id);
                                } else {
                                    collapsed.remove(&id);
                                }
                                decks.clear();
                                collect_deck_items(&root, &collapsed, &mut decks);
                                selected = decks.iter().position(|d| d.id == id).unwrap_or(0);
                                scroll_pos = scroll_pos.min(decks.len().saturating_sub(ROWS));
                            }
                        }
                    } else if let Screen::Options { options, field } = &mut screen {
                        let kind = option_rows(options)
                            .get(*field)
                            .map(|row| row.2)
                            .unwrap_or(OptField::Section);
                        if kind == OptField::LimitScope {
                            let delta = if matches!(action, Action::Right) {
                                1
                            } else {
                                -1
                            };
                            options.set_scope(options.limit_scope.cycle(delta));
                        }
                    } else if let Screen::Menu { selected, .. } = &mut screen {
                        *selected = if matches!(action, Action::Right) {
                            (*selected + 1).min(MAIN_MENU.len() - 1)
                        } else {
                            selected.saturating_sub(1)
                        };
                    } else if let Screen::Settings { selected, .. } = &mut screen {
                        *selected = if matches!(action, Action::Right) {
                            (*selected + 1).min(SettingsCategory::ALL.len() - 1)
                        } else {
                            selected.saturating_sub(1)
                        };
                    } else if let Screen::Stats { page, timeframe, .. } = &mut screen {
                        *timeframe = timeframe.step(matches!(action, Action::Right));
                        event(&format!(
                            "STATS_PAGE {} range={}",
                            *page as usize, *timeframe as usize
                        ));
                    } else if matches!(
                        screen,
                        Screen::Info { .. } | Screen::Browser(_) | Screen::Editor { .. }
                    ) {
                        card_scroll = if matches!(action, Action::Right) {
                            card_scroll.saturating_add(5).min(top_max)
                        } else {
                            card_scroll.saturating_sub(5)
                        };
                    }
                }
                Action::Settings => {
                    if !matches!(screen, Screen::Settings { .. }) {
                        let previous = std::mem::replace(&mut screen, Screen::Decks);
                        screen = Screen::Settings {
                            selected: 0,
                            previous: Box::new(previous),
                        };
                        card_scroll = 0;
                    }
                }
                Action::None
                | Action::Menu
                | Action::MediaSync
                | Action::Exit => {}
            }
            Ok(())
        })();
        if let Err(e) = result {
            let text = error_text(e.as_ref());
            event(&format!("ACTION_ERROR {text}"));
            let previous = std::mem::replace(&mut screen, Screen::Decks);
            screen = Screen::Info {
                title: tr("Error").into(),
                text,
                previous: Box::new(previous),
            };
            card_scroll = 0;
        }
        if matches!(action, Action::Exit) {
            break;
        }
        if !matches!(action, Action::None) {
            if let Some(b) = backend.as_mut() {
                if let Ok(ms) = b.collection_modified_ms() {
                    current_ms = ms;
                }
            }
        }
        let sync_dirty = current_ms > synced_ms;
        let sync_state = if job.is_some() {
            2
        } else if sync_dirty {
            1
        } else {
            0
        };

        if matches!(screen, Screen::Empty { .. })
            && last_empty_poll.elapsed() >= Duration::from_secs(1)
        {
            last_empty_poll = Instant::now();
            if let Screen::Empty { deck_name } = &screen {
                let name = deck_name.clone();
                if let Some(b) = backend.as_mut() {
                    match b.next() {
                        Ok(Some(card)) => {
                            ui_anim.trigger_next_card();
                            screen = Screen::Question {
                                card,
                                deck_name: name,
                            };
                            card_scroll = 0;
                            event("EMPTY -> QUESTION");
                        }
                        Ok(None) => {}
                        Err(e) => screen = Screen::Error(error_text(e.as_ref())),
                    }
                }
            }
        }
        let review_card = match &screen {
            Screen::Question { card, .. } => Some((card.id, false)),
            Screen::Answer { card, .. } => Some((card.id, true)),
            _ => None,
        };
        if let Some(active) = &media_job {
            active.set_paused(review_card.is_some());
        }
        if playing_audio.is_some() && !Ui::audio_is_playing() {
            playing_audio = None;
            if audio_queue_pos + 1 < audio_queue.len() {
                audio_queue_pos += 1;
                let (filename, resolved) = audio_queue[audio_queue_pos].clone();
                match Ui::audio_play(&resolved) {
                    AudioAttempt::Done(true) => {
                        if let Some(active) = backend.as_ref() {
                            active.mark_media_present(&filename);
                        }
                        playing_audio = Some(filename.clone());
                        if let Some((card, answer)) = match &screen {
                            Screen::Question { card, .. } => Some((card, false)),
                            Screen::Answer { card, .. } => Some((card, true)),
                            _ => None,
                        } {
                            if let Some(side_pos) = card_audio(card, answer)
                                .iter()
                                .position(|item| item.filename == filename)
                            {
                                audio_index = side_pos;
                                audio_page = side_pos / AUDIO_VISIBLE;
                            }
                        }
                        event(&format!(
                            "AUDIO queue index={audio_queue_pos} ok=true file={filename}"
                        ));
                    }
                    AudioAttempt::Done(_) | AudioAttempt::Busy => {
                        audio_queue.clear();
                        audio_queue_pos = 0;
                    }
                }
            } else {
                audio_queue.clear();
                audio_queue_pos = 0;
            }
        }
        let card_changed = review_card != last_review_card;
        if card_changed {
            review_scroll = 0.0;
            audio_index = 0;
            audio_page = 0;
            playing_audio = None;
            audio_queue.clear();
            audio_queue_pos = 0;
            Ui::audio_stop();
            if media_job
                .as_ref()
                .is_some_and(media_sync::MediaSync::is_running)
            {
                Ui::media_refresh();
            }
            let current = match &screen {
                Screen::Question { card, .. } | Screen::Answer { card, .. } => Some(card),
                _ => None,
            };
            if let (Some(card), Some(active_backend)) = (current, backend.as_ref()) {
                active_backend.sync_media_presence(card);
            }
            if let (Some(active), Some(card), Some(active_backend)) =
                (media_job.as_ref(), current, backend.as_ref())
            {
                active.set_focus(&active_backend.card_all_files(card));
                let mut files = active_backend.card_missing_files(card, false);
                files.extend(active_backend.card_missing_files(card, true));
                if !files.is_empty() {
                    active.prioritize(&files);
                }
            }
            if let (Some(card), Some(active_backend)) = (current, backend.as_ref()) {
                prefetch.request(prefetch_for_card(active_backend, card));
            } else {
                prefetch.request(Vec::new());
            }
        }
        if media_refreshed_now {
            if let (Some((_, _)), Some(active_backend)) = (review_card, backend.as_ref()) {
                let current = match &screen {
                    Screen::Question { card, .. } | Screen::Answer { card, .. } => Some(card),
                    _ => None,
                };
                if let Some(card) = current {
                    let discovered = active_backend.sync_media_presence(card);
                    if card_media_pending || discovered {
                        top_card.invalidate();
                        prefetch.request(prefetch_for_card(active_backend, card));
                    }
                }
            }
        }
        if card_changed || media_refreshed_now {
            card_media_pending = match (&screen, backend.as_ref()) {
                (Screen::Question { card, .. }, Some(active_backend)) => {
                    active_backend.card_media_missing(card, false)
                }
                (Screen::Answer { card, .. }, Some(active_backend)) => {
                    active_backend.card_media_missing(card, true)
                }
                _ => false,
            };
        }
        if review_card.map(|entry| entry.0) != last_review_card.map(|entry| entry.0) {
            // New card (or left review): drop the typed answer, open hints,
            // media selection and cached reveal HTML, and release animation
            // slots. The question/answer transition of the same card keeps
            // them.
            review = ReviewState::default();
            Ui::anim_close_all();
        }
        if card_changed {
            last_review_card = review_card;
        }
        if let Some((id, answer)) = review_card {
            if last_autoplay_card != Some((id, answer)) {
                if i18n::autoplay_audio() {
                    let audio = match &screen {
                        Screen::Question { card, .. } => card_audio(card, false),
                        Screen::Answer { card, .. } => card_audio(card, true),
                        _ => Vec::new(),
                    };
                    if let Some(first) = audio.first() {
                        audio_queue.clear();
                        audio_queue_pos = 0;
                        let resolved = backend
                            .as_ref()
                            .map(|active| active.media_path(&first.filename))
                            .unwrap_or_else(|| first.path());
                        match Ui::audio_play(&resolved) {
                            AudioAttempt::Done(true) => {
                                if let Some(active) = backend.as_ref() {
                                    active.mark_media_present(&first.filename);
                                }
                                playing_audio = Some(first.filename.clone());
                                event(&format!("AUDIO auto ok=true file={}", first.filename));
                                last_autoplay_card = Some((id, answer));
                            }
                            AudioAttempt::Done(false) => {
                                if let AudioAttempt::Done(decoded) = Ui::audio_decode(&resolved) {
                                    let diag = media_diag(&resolved);
                                    log_media_once(
                                        &format!("audio|{resolved}|{diag}"),
                                        &format!(
                                            "AUDIO auto ok=false decode={decoded} file={} {diag}",
                                            first.filename
                                        ),
                                    );
                                }
                                last_autoplay_card = Some((id, answer));
                            }
                            AudioAttempt::Busy => {}
                        }
                    } else {
                        last_autoplay_card = Some((id, answer));
                    }
                } else {
                    last_autoplay_card = Some((id, answer));
                }
            }
        } else {
            last_autoplay_card = None;
        }
        let state = match &screen {
            Screen::Decks => "DECKS",
            Screen::SyncWait => "SYNC_WAIT",
            Screen::SyncChoice { .. } => "SYNC_CHOICE",
            Screen::Question { .. } => "QUESTION",
            Screen::Answer { .. } => "ANSWER",
            Screen::Empty { .. } => "EMPTY",
            Screen::Browser(_) => "BROWSER",
            Screen::Editor { .. } => "EDITOR",
            Screen::Options { .. } => "OPTIONS",
            Screen::Menu { .. } => "MENU",
            Screen::Settings { .. } => "SETTINGS",
            Screen::SettingsCategory { .. } => "SETTINGS_CATEGORY",
            Screen::SettingsChoice { .. } => "SETTINGS_CHOICE",
            Screen::SettingsButton { .. } => "SETTINGS_BUTTON",
            Screen::SettingsToolbar { .. } => "SETTINGS_TOOLBAR",
            Screen::SettingsAnimations { .. } => "SETTINGS_ANIMATIONS",
            Screen::SettingsBackups { .. } => "SETTINGS_BACKUPS",
            Screen::BackupConfirm { .. } => "BACKUP_CONFIRM",
            Screen::CardChoice { .. } => "CARD_CHOICE",
            Screen::Stats { .. } => "STATS",
            Screen::Quickstart { .. } => "QUICKSTART",
            Screen::Info { .. } => "INFO",
            Screen::Error(_) => "ERROR",
        };
        if state != last_state {
            if !last_state.is_empty()
                && !matches!(
                    (last_state.as_str(), state),
                    ("QUESTION", "ANSWER") | ("ANSWER", "QUESTION")
                )
            {
                ui_anim.trigger_menu_transition();
            }
            if !matches!(state, "QUESTION" | "ANSWER") {
                ui_anim.clear_card_transition();
            }
            if state == "EMPTY" {
                ui_anim.trigger_celebration();
                sfx::play(sfx::SfxKind::DeckComplete);
            }
            if state == "DECKS" {
                if let Some(b) = backend.as_mut() {
                    daily_studied = b.studied_today_text().unwrap_or_default();
                }
                if let Some(b) = backend.as_mut() {
                    match b.deck_tree() {
                        Ok(tree) => {
                            root = tree;
                            decks.clear();
                            collect_deck_items(&root, &collapsed, &mut decks);
                        }
                        Err(e) => {
                            screen = Screen::Error(error_text(e.as_ref()));
                        }
                    }
                }
                selected = selected.min(decks.len().saturating_sub(1));
                scroll_pos = scroll_pos.min(decks.len().saturating_sub(ROWS));
            }
            event(&format!("{state} screen"));
            last_state = state.into();
        }

        let media_running_now = media_job.as_ref().is_some_and(|active| active.is_running());
        let media_processed_now = media_job
            .as_ref()
            .map(|active| active.processed_files())
            .unwrap_or(0);
        let media_checked_now = media_job
            .as_ref()
            .map(|active| active.checked_files())
            .unwrap_or(0);
        let media_result_now = media_result
            .as_ref()
            .and_then(|text| (Instant::now() < media_notice_until).then(|| text.as_str()));
        let clock_known_now = clock::is_known();
        if media_running_now != last_media_running
            || media_processed_now != last_media_processed
            || media_checked_now != last_media_checked
            || media_result_now != last_media_result.as_deref()
            || clock_known_now != last_clock_known
        {
            last_media_running = media_running_now;
            last_media_processed = media_processed_now;
            last_media_checked = media_checked_now;
            last_media_result = media_result_now.map(str::to_owned);
            last_clock_known = clock_known_now;
            cached_media_status = if media_running_now {
                Some(if media_processed_now > 0 {
                    i18n::format("{0} files", &[&media_processed_now.to_string()])
                } else if media_checked_now > 0 {
                    i18n::format("checking {0} files", &[&media_checked_now.to_string()])
                } else {
                    tr("starting").to_owned()
                })
            } else {
                last_media_result.clone()
            };
            cached_footer = if let Some(status) = cached_media_status.as_deref() {
                i18n::format("Media sync: {0}", &[status])
            } else if clock_known_now {
                tr("Menu [SELECT]                      Options").to_owned()
            } else {
                tr("Clock will update when connected").to_owned()
            };
        }
        let media_status = cached_media_status.as_deref();
        Ui::frame_begin();
        frame = frame.wrapping_add(1);
        Ui::target_top(current_theme().bg_top);
        top_max = 0;
        review_max_scroll = 0.0;
        match &screen {
            Screen::SyncWait => {
                header(
                    400.0,
                    job.as_ref().map(sync::Job::title).unwrap_or(tr("Syncing")),
                );
                top_layout.draw(
                    &job.as_ref().map(sync::Job::label).unwrap_or_default(),
                    360.0,
                    200.0,
                    65.0,
                    5,
                    0,
                );
                let phase = (started.elapsed().as_millis() / 120 % 8) as usize;
                for index in 0..8 {
                    let angle = index as f32 * std::f32::consts::TAU / 8.0;
                    Ui::draw_rect(
                        196.0 + angle.cos() * 15.0,
                        184.0 + angle.sin() * 15.0,
                        6.0,
                        6.0,
                        if index == phase {
                            current_theme().new
                        } else {
                            current_theme().border
                        },
                    );
                }
            }
            Screen::SyncChoice {
                upload_ok,
                download_ok,
                download,
                confirmed: _,
            } => {
                header(400.0, tr("Full sync"));
                let selected = if *download {
                    tr("Download (replaces local)")
                } else {
                    tr("Upload (replaces remote)")
                };
                let text = i18n::format(
                    "Full sync replaces one collection.\nUpload allowed: {0}\nDownload allowed: {1}\nSelected: {2}",
                    &[
                        tr(if *upload_ok { "Yes" } else { "No" }),
                        tr(if *download_ok { "Yes" } else { "No" }),
                        selected,
                    ],
                );
                top_layout.draw(&text, 360.0, 200.0, 60.0, 8, 0);
            }
            Screen::Decks => {
                Ui::draw_text_centered(200.0, 32.0, 0.65, current_theme().text, tr("Decks"));
                Ui::draw_line(20.0, 60.0, 380.0, 60.0, 1.0, current_theme().border);
                if let Some(deck) = decks.get(selected) {
                    Ui::fit_text(
                        200.0,
                        82.0,
                        0.68,
                        360.0,
                        current_theme().text,
                        &deck.leaf_name,
                    );
                    Ui::fit_text(
                        200.0,
                        108.0,
                        0.44,
                        360.0,
                        current_theme().text_muted,
                        &deck.name,
                    );
                    counts(deck.new_count, deck.learn_count, deck.review_count, 136.0);
                    if !daily_studied.is_empty() {
                        Ui::fit_text(
                            200.0,
                            176.0,
                            0.40,
                            360.0,
                            current_theme().text_muted,
                            &daily_studied,
                        );
                    }
                    let due = deck.new_count + deck.learn_count + deck.review_count;
                    Ui::fit_text(
                        200.0,
                        194.0,
                        0.44,
                        360.0,
                        current_theme().text,
                        &i18n::format("Cards due: {0}", &[&due.to_string()]),
                    );
                } else {
                    Ui::draw_text_centered(
                        200.0,
                        100.0,
                        0.60,
                        current_theme().text,
                        tr("No decks"),
                    );
                }
            }
            Screen::Question { card, .. } | Screen::Answer { card, .. } => {
                let answer = matches!(screen, Screen::Answer { .. });
                let owned_html = if !answer && card.type_answer.is_some() {
                    // Show the input box where the `[[type:...]]`
                    // placeholder was so it never leaks onto the card.
                    Some(backend::fill_type_answer(
                        &card.question_html,
                        "<input id=typeans>",
                    ))
                } else {
                    None
                };
                let html = if answer {
                    match &review.revealed {
                        Some(revealed) if revealed.card_id == card.id => revealed.answer.as_str(),
                        _ => card
                            .cloze_answer_html
                            .as_deref()
                            .unwrap_or(&card.answer_html),
                    }
                } else {
                    owned_html.as_deref().unwrap_or(card.question_html.as_str())
                };
                let media = if answer {
                    &card.answer_media
                } else {
                    &card.question_media
                };
                let images: Vec<String> = media
                    .iter()
                    .filter(|item| item.kind == backend::MediaKind::Image)
                    .filter(|item| {
                        backend
                            .as_ref()
                            .is_some_and(|active| active.media_exists(&item.filename))
                    })
                    .map(|item| {
                        backend
                            .as_ref()
                            .map(|active| active.media_path(&item.filename))
                            .unwrap_or_else(|| item.path())
                    })
                    .collect();
                top_card.set_text(
                    html,
                    &card.css,
                    theme::is_dark(),
                    &images,
                    CARD_WIDTH,
                    i18n::font_scale_factor(),
                    &review.hints_set(),
                    review.typed.as_deref().unwrap_or(""),
                    media_running_now,
                );
                if !review.media_ready {
                    // Adopt the autoplay policy once per card: animated
                    // images loop on their own, video starts only with an
                    // autoplay attribute or the matching setting.
                    review.media_ready = true;
                    review.media_sel = 0;
                    for (index, anim) in top_card.media_items().iter().enumerate() {
                        if !anim.supported {
                            continue;
                        }
                        let want = anim.autoplay || (anim.video && i18n::autoplay_video());
                        Ui::anim_set_playing(anim.id, want);
                        if index == review.media_sel {
                            review.media_playing = want;
                        }
                    }
                }
                let now = Instant::now();
                let elapsed = now.duration_since(last_frame).as_millis().min(1000) as u32;
                last_frame = now;
                top_card.tick_anims(elapsed);
                review_max_scroll = top_card.max_scroll();
                review_scroll = review_scroll.min(review_max_scroll);
                let (offset_x, offset_y, fade) = ui_anim.card_transition();
                top_card.draw_offset(review_scroll, offset_x, offset_y);
                anim::draw_card_transition_overlay(fade, answer);
                Ui::draw_scroll_indicator(
                    TOP_WIDTH - 4.0,
                    0.0,
                    3.0,
                    TOP_HEIGHT,
                    review_scroll,
                    review_max_scroll,
                );
            }
            Screen::Browser(b) => {
                Ui::draw_text_centered(200.0, 26.0, 0.55, current_theme().text, tr("Preview"));
                let status = browse_status(&b.status);
                let text = format!("{status}\n\n{}\n\n{}", b.preview.0, b.preview.1);
                top_max = top_layout.draw(&text, 360.0, 200.0, 56.0, 7, card_scroll);
            }
            Screen::Editor { draft, field, .. } => {
                let (label, value) = if *field < draft.labels.len() {
                    (&draft.labels[*field], draft.note.fields()[*field].clone())
                } else {
                    (&String::from(tr("Tags")), draft.note.tags.join(" "))
                };
                Ui::fit_text(200.0, 30.0, 0.60, 360.0, current_theme().text, label);
                top_max = top_layout.draw(&value, 360.0, 200.0, 64.0, 7, card_scroll);
            }
            Screen::Options { options, field } => {
                let rows = option_rows(options);
                let label = rows.get(*field).map(|row| row.0.as_str()).unwrap_or("");
                let value = rows.get(*field).map(|row| row.1.as_str()).unwrap_or("");
                Ui::draw_text_centered(200.0, 40.0, 0.62, current_theme().text, label);
                if !value.is_empty() {
                    Ui::draw_text_centered(200.0, 80.0, 0.52, current_theme().text_muted, value);
                }
                top_max = top_layout.draw(
                    tr("Changes apply to this deck's preset."),
                    360.0,
                    200.0,
                    118.0,
                    6,
                    0,
                );
            }
            Screen::Menu { .. } => {
                Ui::draw_text_centered(200.0, 70.0, 0.65, current_theme().text, tr("Menu"));
                Ui::draw_text_centered(
                    200.0,
                    112.0,
                    0.48,
                    current_theme().text_muted,
                    tr("Select an action. B returns."),
                );
            }
            Screen::Settings { .. } => {
                Ui::draw_text_centered(200.0, 70.0, 0.65, current_theme().text, tr("Settings"));
                Ui::draw_text_centered(
                    200.0,
                    112.0,
                    0.48,
                    current_theme().text_muted,
                    tr("Choose a category. B returns."),
                );
            }
            Screen::SettingsCategory {
                category, selected, ..
            } => {
                if matches!(settings_items(*category).get(*selected), Some(SettingsItem::Animations)) {
                    anim::draw_settings_preview(0, frame);
                } else {
                    Ui::fit_text(
                        200.0,
                        38.0,
                        0.60,
                        360.0,
                        current_theme().text,
                        tr(category.label()),
                    );
                    Ui::fit_text(
                        200.0,
                        86.0,
                        0.48,
                        360.0,
                        current_theme().text_muted,
                        tr(category.hint()),
                    );
                    Ui::draw_text_centered(
                        200.0,
                        130.0,
                        0.44,
                        current_theme().text_faint,
                        tr("A opens or changes. B returns."),
                    );
                }
            }
            Screen::SettingsButton { action, .. } => {
                Ui::fit_text(
                    200.0,
                    34.0,
                    0.62,
                    360.0,
                    current_theme().text,
                    tr(action.label()),
                );
                let current = controls::button_for(*action)
                    .map_or(tr("Unbound").to_owned(), |button| {
                        tr(button.label()).to_owned()
                    });
                Ui::fit_text(
                    200.0,
                    76.0,
                    0.50,
                    360.0,
                    current_theme().text_muted,
                    &i18n::format("Current: {0}", &[&current]),
                );
                Ui::draw_text_centered(
                    200.0,
                    128.0,
                    0.46,
                    current_theme().text_muted,
                    tr("Choose a button. A assigns. B returns."),
                );
            }
            Screen::SettingsChoice { kind, .. } => {
                Ui::fit_text(
                    200.0,
                    34.0,
                    0.62,
                    360.0,
                    current_theme().text,
                    tr(choice_title(*kind)),
                );
                Ui::fit_text(
                    200.0,
                    76.0,
                    0.50,
                    360.0,
                    current_theme().text_muted,
                    &choice_label(*kind),
                );
                Ui::draw_text_centered(
                    200.0,
                    128.0,
                    0.46,
                    current_theme().text_muted,
                    tr("Choose an option. A applies. B returns."),
                );
            }
            Screen::SettingsToolbar { .. } => {
                Ui::fit_text(
                    200.0,
                    34.0,
                    0.62,
                    360.0,
                    current_theme().text,
                    tr("Toolbar"),
                );
                Ui::fit_text(
                    200.0,
                    76.0,
                    0.50,
                    360.0,
                    current_theme().text_muted,
                    tr("A toggles. LEFT/RIGHT reorders. B returns."),
                );
                Ui::draw_text_centered(
                    200.0,
                    118.0,
                    0.46,
                    current_theme().text_muted,
                    tr("Move up [Y]"),
                );
                Ui::draw_text_centered(
                    200.0,
                    136.0,
                    0.46,
                    current_theme().text_muted,
                    tr("Move down [X]"),
                );
            }
            Screen::SettingsAnimations { selected, .. } => {
                anim::draw_settings_preview(*selected, frame);
            }
            Screen::SettingsBackups {
                selected,
                backups,
                status,
                ..
            } => {
                Ui::fit_text(
                    200.0,
                    34.0,
                    0.62,
                    360.0,
                    current_theme().text,
                    tr("Backups"),
                );
                let subtitle = if let Some(text) = status.as_deref() {
                    text.to_owned()
                } else if *selected == 0 {
                    tr("Create a backup or select one to restore.").to_owned()
                } else if let Some(entry) = backups.get(*selected - 1) {
                    i18n::format("Backup: {0}", &[&entry.label])
                } else {
                    String::new()
                };
                Ui::fit_text(
                    200.0,
                    78.0,
                    0.48,
                    360.0,
                    current_theme().text_muted,
                    &subtitle,
                );
                Ui::draw_text_centered(
                    200.0,
                    128.0,
                    0.44,
                    current_theme().text_faint,
                    if backups.is_empty() {
                        tr("No backups saved yet.")
                    } else {
                        tr("A creates or restores. B returns.")
                    },
                );
            }
            Screen::BackupConfirm { entry, .. } => {
                header(400.0, tr("Restore backup"));
                top_layout.draw(
                    &i18n::format(
                        "Replace the current collection with backup {0}? Unsynced changes since this backup will be lost.",
                        &[&entry.label],
                    ),
                    360.0,
                    200.0,
                    64.0,
                    7,
                    0,
                );
            }
            Screen::CardChoice { .. } => {
                Ui::draw_text_centered(
                    200.0,
                    70.0,
                    0.65,
                    current_theme().text,
                    tr("Bury / Suspend"),
                );
                Ui::draw_text_centered(
                    200.0,
                    112.0,
                    0.48,
                    current_theme().text_muted,
                    tr("Choose an action for the current card."),
                );
            }
            Screen::Stats {
                data,
                page,
                timeframe,
                ..
            } => {
                stats::draw_top(data, *page, *timeframe);
            }
            Screen::Quickstart { step, selected, .. } => {
                quickstart::draw_top(*step, *selected);
            }
            Screen::Info { title, text, .. } => {
                Ui::fit_text(200.0, 30.0, 0.60, 360.0, current_theme().text, title);
                top_max = top_layout.draw(text, 360.0, 200.0, 64.0, 7, card_scroll);
            }
            Screen::Empty { deck_name } => {
                if i18n::anim_active(i18n::AnimKind::DeckCelebration) {
                    ui_anim.draw_deck_celebration(deck_name, &daily_studied);
                    top_max = 0;
                } else {
                    Ui::draw_text_centered(
                        200.0,
                        65.0,
                        0.58,
                        current_theme().text,
                        tr("No cards available now"),
                    );
                    top_max = top_layout.draw(
                        &i18n::format("Waiting for learning cards in {0}.", &[deck_name]),
                        360.0,
                        200.0,
                        106.0,
                        5,
                        0,
                    );
                }
            }
            Screen::Error(err) => {
                Ui::draw_text_centered(200.0, 30.0, 0.60, current_theme().learn, tr("Error"));
                top_max = top_layout.draw(err, 360.0, 200.0, 62.0, 7, card_scroll);
            }
        }
        draw_rating_badge(rating_feedback);
        draw_leech_notice(leech_notice.as_ref());
        ui_anim.draw_top_overlay();
        Ui::target_bottom(current_theme().bg_bottom);
        match &screen {
            Screen::SyncWait => {
                header(
                    320.0,
                    job.as_ref().map(sync::Job::title).unwrap_or(tr("Syncing")),
                );
                button(60.0, 194.0, 200.0, tr("Cancel [B]"), None, 6);
            }
            Screen::SyncChoice {
                upload_ok,
                download_ok,
                download,
                confirmed,
            } => {
                header(
                    320.0,
                    if *confirmed {
                        tr("Confirm replacement")
                    } else {
                        tr("Full sync")
                    },
                );
                button(
                    12.0,
                    64.0,
                    140.0,
                    tr("Upload"),
                    None,
                    if *upload_ok && !*download { 0 } else { 6 },
                );
                button(
                    168.0,
                    64.0,
                    140.0,
                    tr("Download"),
                    None,
                    if *download_ok && *download { 0 } else { 6 },
                );
                button(
                    12.0,
                    194.0,
                    140.0,
                    if *confirmed {
                        tr("Confirm [A]")
                    } else {
                        tr("Continue [A]")
                    },
                    None,
                    0,
                );
                button(168.0, 194.0, 140.0, tr("Cancel [B]"), None, 6);
            }
            Screen::Decks => {
                let actions = [
                    (tr("Add"), 1),
                    (tr("Edit"), 2),
                    (tr("Browse"), 3),
                    (tr("Options"), 4),
                ];
                for (i, (label, icon)) in actions.iter().enumerate() {
                    Ui::draw_nav_tab(i as f32 * 80.0, 0.0, 80.0, 42.0, label, *icon, false);
                }
                for (row_index, index) in (scroll_pos..decks.len()).take(ROWS).enumerate() {
                    let d = &decks[index];
                    Ui::draw_deck_row(
                        ROW_Y + row_index as f32 * ROW_H,
                        304.0,
                        &d.leaf_name,
                        d.level.min(8),
                        d.has_children,
                        !collapsed.contains(&d.id),
                        d.review_count,
                        d.new_count,
                        d.learn_count,
                        index == selected,
                    );
                }
                Ui::draw_scrollbar(
                    304.0,
                    ROW_Y,
                    16.0,
                    ROW_H * ROWS as f32,
                    decks.len(),
                    ROWS,
                    scroll_pos,
                );
                button(12.0, 194.0, 140.0, tr("Study [A]"), None, 0);
                draw_sync_button(168.0, 194.0, 140.0, 28.0, sync_state);
            }
            Screen::Question { card, deck_name } => {
                draw_review_header(deck_name, sync_state);
                if i18n::card_counts() {
                    counts_compact(card.new_count, card.learn_count, card.review_count, 48.0);
                }
                let audio = card_audio(card, false);
                let states = audio_states(&audio, backend.as_ref(), playing_audio.as_deref());
                draw_audio_buttons(&audio, &states, frame, audio_page);
                draw_media_strip(
                    media_status.as_deref(),
                    card_media_pending
                        && media_job
                            .as_ref()
                            .is_some_and(media_sync::MediaSync::is_running),
                );
                if card.type_answer.is_some() {
                    let sub = review.typed.clone().filter(|typed| !typed.is_empty());
                    Ui::draw_button(
                        8.0,
                        96.0,
                        208.0,
                        28.0,
                        tr("Type answer"),
                        sub.as_deref(),
                        6,
                        false,
                    );
                }
                draw_hint_chips(&top_card, &review, 184.0);
                draw_media_bar(&top_card, &review, false);
                Ui::draw_button(8.0, 128.0, 208.0, 52.0, tr("Show answer"), None, 0, false);
                draw_undo_button(224.0, 128.0, 88.0, 52.0, undo_available);
            }
            Screen::Answer { card, deck_name } => {
                draw_review_header(deck_name, sync_state);
                if i18n::card_counts() {
                    counts_compact(card.new_count, card.learn_count, card.review_count, 48.0);
                }
                let audio = card_audio(card, true);
                let states = audio_states(&audio, backend.as_ref(), playing_audio.as_deref());
                draw_audio_buttons(&audio, &states, frame, audio_page);
                draw_media_strip(
                    media_status.as_deref(),
                    card_media_pending
                        && media_job
                            .as_ref()
                            .is_some_and(media_sync::MediaSync::is_running),
                );
                draw_hint_chips(&top_card, &review, 126.0);
                draw_media_bar(&top_card, &review, true);
                draw_undo_button(236.0, 148.0, 72.0, 36.0, undo_available);
                let rating_actions = [
                    ReviewAction::Again,
                    ReviewAction::Hard,
                    ReviewAction::Good,
                    ReviewAction::Easy,
                ];
                for (i, label) in [tr("Again"), tr("Hard"), tr("Good"), tr("Easy")]
                    .iter()
                    .enumerate()
                {
                    let interval = card.intervals.get(i).map(String::as_str).unwrap_or("?");
                    let key = controls::button_for(rating_actions[i])
                        .map_or("--", controls::Button::label);
                    let hint = if i18n::rating_intervals() {
                        format!("{interval} [{key}]")
                    } else {
                        format!("[{key}]")
                    };
                    let hint = Ui::ellipsize(&hint, 0.42, 68.0);
                    let x = 8.0 + i as f32 * 76.0;
                    Ui::draw_button(
                        x,
                        190.0,
                        72.0,
                        36.0,
                        label,
                        Some(&hint),
                        i as i32 + 2,
                        false,
                    );
                }
            }
            Screen::Browser(b) => {
                let title = if b.query.is_empty() {
                    tr("All").to_owned()
                } else {
                    b.query.clone()
                };
                Ui::draw_text(
                    10.0,
                    6.0,
                    0.46,
                    current_theme().text,
                    &Ui::ellipsize(&format!("{}: {title}", tr("Search")), 0.46, 300.0),
                );
                for (i, (label, (cx, cw))) in
                    BROWSE_FILTERS.iter().zip(browse_chip_rects()).enumerate()
                {
                    browse_chip(cx, 28.0, cw, 20.0, tr(*label), i == b.filter);
                }
                for (i, title) in b.titles.iter().enumerate() {
                    browse_row(
                        54.0 + i as f32 * 22.0,
                        title,
                        b.page * ROWS + i == b.selected,
                    );
                }
                Ui::draw_scrollbar(
                    304.0,
                    54.0,
                    16.0,
                    132.0,
                    b.ids.len(),
                    ROWS,
                    b.page.saturating_mul(ROWS),
                );
                button(8.0, 194.0, 98.0, tr("Edit note"), None, 0);
                button(110.0, 194.0, 98.0, tr("Restore"), None, 0);
                button(214.0, 194.0, 98.0, tr("Search"), None, 0);
            }
            Screen::Editor { draft, field, .. } => {
                header(
                    320.0,
                    if draft.note.id.0 == 0 {
                        tr("Add note")
                    } else {
                        tr("Edit note")
                    },
                );
                let start = field / ROWS * ROWS;
                for index in (start..draft.labels.len() + 1).take(ROWS) {
                    let label = if index < draft.labels.len() {
                        draft.labels[index].as_str()
                    } else {
                        tr("Tags")
                    };
                    row(43.0 + (index - start) as f32 * 24.0, label, index == *field);
                }
                Ui::draw_scrollbar(
                    304.0,
                    43.0,
                    16.0,
                    144.0,
                    draft.labels.len() + 1,
                    ROWS,
                    start,
                );
                button(12.0, 194.0, 140.0, tr("Save [X]"), None, 0);
                button(168.0, 194.0, 140.0, tr("Cancel [B]"), None, 6);
            }
            Screen::Options { options, field } => {
                header(320.0, tr("Deck options"));
                let rows = option_rows(options);
                let start = (*field / ROWS * ROWS).min(rows.len().saturating_sub(ROWS));
                for (i, (label, value, kind)) in rows.iter().enumerate().skip(start).take(ROWS) {
                    option_row(
                        43.0 + (i - start) as f32 * 24.0,
                        label,
                        value,
                        *kind == OptField::Section,
                        i == *field,
                    );
                }
                Ui::draw_scrollbar(304.0, 43.0, 16.0, 144.0, rows.len(), ROWS, start);
                button(12.0, 194.0, 140.0, tr("Save [X]"), None, 0);
                button(168.0, 194.0, 140.0, tr("Cancel [B]"), None, 6);
            }
            Screen::Menu { selected, .. } => {
                header(320.0, tr("Menu"));
                for i in 0..MAIN_MENU.len() {
                    let (x, y) = menu_tile(i, MENU_ROWS_MAX);
                    let style = if i == *selected { 0 } else { 6 };
                    let label = Ui::ellipsize(&menu_label(i), 0.50, MENU_TILE_W - 12.0);
                    Ui::draw_button(x, y, MENU_TILE_W, MENU_TILE_H, &label, None, style, false);
                }
            }
            Screen::Settings { selected, .. } => {
                header(320.0, tr("Settings"));
                for (index, category) in SettingsCategory::ALL.iter().enumerate() {
                    let (x, y) = settings_tile(index);
                    let style = if index == *selected { 0 } else { 6 };
                    let label = Ui::ellipsize(tr(category.label()), 0.50, SETTINGS_TILE_W - 12.0);
                    Ui::draw_button(
                        x,
                        y,
                        SETTINGS_TILE_W,
                        SETTINGS_TILE_H,
                        &label,
                        None,
                        style,
                        false,
                    );
                }
            }
            Screen::SettingsCategory {
                category, selected, ..
            } => {
                header(320.0, tr(category.label()));
                let (media_running, media_files) = media_job
                    .as_ref()
                    .map(|active| (active.is_running(), active.processed_files()))
                    .unwrap_or((false, 0));
                let media_status = if media_running {
                    media_status.as_deref()
                } else {
                    media_result.as_deref()
                };
                let items = settings_items(*category);
                let start = list_start(*selected, items.len());
                for (index, item) in items
                    .iter()
                    .enumerate()
                    .skip(start)
                    .take(SETTINGS_VISIBLE_ROWS)
                {
                    let (x, y) = settings_tile(index - start);
                    let style = if index == *selected { 0 } else { 6 };
                    let label =
                        settings_item_label(*item, media_running, media_files, media_status);
                    let label = Ui::ellipsize(&label, 0.50, SETTINGS_TILE_W - 12.0);
                    Ui::draw_button(
                        x,
                        y,
                        SETTINGS_TILE_W,
                        SETTINGS_TILE_H,
                        &label,
                        None,
                        style,
                        false,
                    );
                }
            }
            Screen::SettingsChoice { kind, selected, .. } => {
                header(320.0, tr(choice_title(*kind)));
                let active = choice_selected(*kind);
                let options = choice_options(*kind);
                let start = choice_start(*selected, options.len());
                for (index, option) in options
                    .iter()
                    .enumerate()
                    .skip(start)
                    .take(CHOICE_VISIBLE_ROWS)
                {
                    let (x, y) = choice_tile(index - start);
                    let style = if index == *selected { 0 } else { 6 };
                    let label = Ui::ellipsize(option, 0.48, SETTINGS_TILE_W - 28.0);
                    Ui::draw_button(
                        x,
                        y,
                        SETTINGS_TILE_W,
                        CHOICE_TILE_H,
                        &label,
                        None,
                        style,
                        false,
                    );
                    if index == active {
                        Ui::draw_sync_icon(
                            x + SETTINGS_TILE_W - 20.0,
                            y + (CHOICE_TILE_H - 10.0) * 0.5,
                            10.0,
                            0,
                        );
                    }
                }
            }
            Screen::SettingsButton {
                action, selected, ..
            } => {
                header(320.0, tr("Controls and keybinds"));
                let active = controls::button_option_index(controls::button_for(*action));
                let length = controls::button_option_count();
                let start = choice_start(*selected, length);
                for index in start..(start + CHOICE_VISIBLE_ROWS).min(length) {
                    let (x, y) = choice_tile(index - start);
                    let style = if index == *selected { 0 } else { 6 };
                    let button = controls::button_option(index);
                    let mut label = button.map_or(tr("Unbound").to_owned(), |button| {
                        tr(button.label()).to_owned()
                    });
                    if button
                        .is_some_and(|button| controls::conflict_for(*action, button).is_some())
                    {
                        label = i18n::format("{0} (used)", &[&label]);
                    }
                    let label = Ui::ellipsize(&label, 0.48, SETTINGS_TILE_W - 28.0);
                    Ui::draw_button(
                        x,
                        y,
                        SETTINGS_TILE_W,
                        CHOICE_TILE_H,
                        &label,
                        None,
                        style,
                        false,
                    );
                    if index == active {
                        Ui::draw_sync_icon(
                            x + SETTINGS_TILE_W - 20.0,
                            y + (CHOICE_TILE_H - 10.0) * 0.5,
                            10.0,
                            0,
                        );
                    }
                }
            }
            Screen::SettingsToolbar { selected, .. } => {
                header(320.0, tr("Toolbar"));
                let order = i18n::toolbar_order();
                for index in 0..5 {
                    let (x, y) = settings_tile(index);
                    let style = if index == *selected { 0 } else { 6 };
                    let label = if index == 0 {
                        i18n::format(
                            "Toolbar: {0}",
                            &[tr(if i18n::toolbar_visible() { "On" } else { "Off" })],
                        )
                    } else {
                        let item = order[index - 1];
                        i18n::format(
                            "{0}: {1}",
                            &[
                                tr(item.label()),
                                tr(if i18n::toolbar_item_visible(item) {
                                    "On"
                                } else {
                                    "Off"
                                }),
                            ],
                        )
                    };
                    let label = Ui::ellipsize(&label, 0.50, SETTINGS_TILE_W - 12.0);
                    Ui::draw_button(
                        x,
                        y,
                        SETTINGS_TILE_W,
                        SETTINGS_TILE_H,
                        &label,
                        None,
                        style,
                        false,
                    );
                }
                let style = if *selected == 5 { 0 } else { 6 };
                let deck_label = i18n::format(
                    "{0}: {1}",
                    &[
                        tr("Current deck"),
                        tr(if i18n::deck_title_visible() {
                            "On"
                        } else {
                            "Off"
                        }),
                    ],
                );
                let deck_label = Ui::ellipsize(&deck_label, 0.46, 128.0);
                Ui::draw_button(10.0, 150.0, 140.0, 20.0, &deck_label, None, style, false);
                let (items, count) = i18n::active_toolbar_items();
                Ui::draw_status_compact(255.0, 150.0, 0.48, sync_state, &items[..count]);
                let can_move = (1..=4).contains(selected);
                let up_style = if can_move && *selected > 1 { 0 } else { 6 };
                let down_style = if can_move && *selected < 4 { 0 } else { 6 };
                button(12.0, 194.0, 140.0, tr("Move up [Y]"), None, up_style);
                button(168.0, 194.0, 140.0, tr("Move down [X]"), None, down_style);
            }
            Screen::SettingsAnimations { selected, .. } => {
                header(320.0, tr("Animations"));
                let start = list_start(*selected, 5);
                for index in start..(start + SETTINGS_VISIBLE_ROWS).min(5) {
                    let (x, y) = settings_tile(index - start);
                    let style = if index == *selected { 0 } else { 6 };
                    let text = if index == 0 {
                        i18n::format(
                            "Animations: {0}",
                            &[tr(if i18n::animations_enabled() {
                                "On"
                            } else {
                                "Off"
                            })],
                        )
                    } else {
                        let kind = i18n::AnimKind::ALL[index - 1];
                        i18n::format(
                            "{0}: {1}",
                            &[
                                tr(kind.label()),
                                tr(if i18n::anim_active(kind) {
                                    "On"
                                } else {
                                    "Off"
                                }),
                            ],
                        )
                    };
                    let label = Ui::ellipsize(&text, 0.50, SETTINGS_TILE_W - 12.0);
                    Ui::draw_button(
                        x,
                        y,
                        SETTINGS_TILE_W,
                        SETTINGS_TILE_H,
                        &label,
                        None,
                        style,
                        false,
                    );
                }
                Ui::draw_text_centered(
                    160.0,
                    206.0,
                    0.42,
                    current_theme().text_muted,
                    tr("A or LEFT/RIGHT toggles. B returns."),
                );
            }
            Screen::SettingsBackups {
                selected, backups, ..
            } => {
                header(320.0, tr("Backups"));
                let length = backups.len() + 1;
                let start = list_start(*selected, length);
                for index in start..(start + SETTINGS_VISIBLE_ROWS).min(length) {
                    let (x, y) = settings_tile(index - start);
                    let style = if index == *selected { 0 } else { 6 };
                    let text = if index == 0 {
                        tr("Create backup now").to_owned()
                    } else {
                        backups[index - 1].row_label()
                    };
                    let label = Ui::ellipsize(&text, 0.50, SETTINGS_TILE_W - 12.0);
                    Ui::draw_button(
                        x,
                        y,
                        SETTINGS_TILE_W,
                        SETTINGS_TILE_H,
                        &label,
                        None,
                        style,
                        false,
                    );
                }
            }
            Screen::BackupConfirm { .. } => {
                header(320.0, tr("Confirm replacement"));
                button(12.0, 194.0, 140.0, tr("Confirm [A]"), None, 0);
                button(168.0, 194.0, 140.0, tr("Cancel [B]"), None, 6);
            }
            Screen::CardChoice { selected, .. } => {
                header(320.0, tr("Bury / Suspend"));
                Ui::draw_button(
                    12.0,
                    70.0,
                    146.0,
                    48.0,
                    tr("Bury"),
                    None,
                    if *selected == 0 { 0 } else { 6 },
                    false,
                );
                Ui::draw_button(
                    162.0,
                    70.0,
                    146.0,
                    48.0,
                    tr("Suspend"),
                    None,
                    if *selected == 1 { 0 } else { 6 },
                    false,
                );
                button(60.0, 160.0, 200.0, tr("Cancel [B]"), None, 6);
            }
            Screen::Stats {
                page, timeframe, ..
            } => {
                stats::draw_bottom(*page, *timeframe);
            }
            Screen::Quickstart { step, selected, .. } => {
                quickstart::draw_bottom(*step, *selected);
            }
            Screen::Info { title, .. } => {
                header(320.0, title);
                button(60.0, 194.0, 200.0, tr("Back [B]"), None, 0);
            }
            Screen::Empty { .. } => {
                header(320.0, tr("Review"));
                Ui::draw_button(
                    60.0,
                    110.0,
                    200.0,
                    34.0,
                    tr("Back to Decks [B]"),
                    None,
                    0,
                    false,
                );
            }
            Screen::Error(_) => {
                header(320.0, tr("Error"));
                Ui::draw_text_centered(160.0, 90.0, 0.50, current_theme().text, tr("Retry [A]"));
                Ui::draw_text_centered(160.0, 120.0, 0.50, current_theme().text, tr("START: Exit"));
            }
        }
        let hide_footer = matches!(
            screen,
            Screen::Question { .. }
                | Screen::Answer { .. }
                | Screen::SyncChoice { .. }
                | Screen::CardChoice { .. }
                | Screen::Stats { .. }
                | Screen::Quickstart { .. }
                | Screen::Settings { .. }
                | Screen::SettingsCategory { .. }
                | Screen::SettingsChoice { .. }
                | Screen::SettingsButton { .. }
                | Screen::SettingsToolbar { .. }
                | Screen::SettingsAnimations { .. }
                | Screen::SettingsBackups { .. }
                | Screen::BackupConfirm { .. }
        );
        if !hide_footer {
            footer(&cached_footer);
        }
        ui_anim.draw_bottom_overlay();
        Ui::frame_end();
    }
    if let Some(active) = &media_job {
        active.cancel();
    }
    if let Some(active) = job.take() {
        if let Some(done) = active.finish() {
            backend = done.backend;
        }
    }
    drop(clock_job);
    drop(media_job.take());
    prefetch.stop();
    Ui::fini();
    leds::shutdown();
    if let Some(backend) = backend {
        match backend.close() {
            Ok(()) => event("CLOSE_OK"),
            Err(e) => event(&format!("CLOSE_ERROR {}", error_text(e.as_ref()))),
        }
    }
}
