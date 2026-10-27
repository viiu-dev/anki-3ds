// Copyright: Anki 3DS contributors
// License: GNU AGPL, version 3 or later; http://www.gnu.org/licenses/agpl.html

//! Rendered-card text handling.
//!
//! Cards arrive as rendered HTML. The console has no browser engine, so this
//! module turns the small but useful HTML subset produced by Anki templates
//! into styled blocks. It also understands both HTML ruby and Anki's
//! `kanji[reading]` furigana notation. Layout and drawing live in `ui.rs`.

pub const CLOZE_START: char = '\u{e000}';
pub const CLOZE_END: char = '\u{e001}';

pub fn plain(text: &str) -> String {
    text.chars()
        .filter(|ch| !matches!(*ch, CLOZE_START | CLOZE_END))
        .collect()
}

/// Only Anki's generated cloze span changes color; other HTML remains intact
/// for the rich renderer. Track nested spans so a formatting span inside a
/// cloze cannot end its emphasis early.
pub fn mark_clozes(html: &str) -> Option<String> {
    let mut output = String::new();
    let mut spans = Vec::new();
    let mut remaining = html;
    let mut found = false;
    while let Some(start) = remaining.find('<') {
        output.push_str(&remaining[..start]);
        remaining = &remaining[start..];
        let mut quote = None;
        let end = remaining.char_indices().find_map(|(index, ch)| {
            if let Some(delimiter) = quote {
                if ch == delimiter {
                    quote = None;
                }
            } else if ch == '\'' || ch == '"' {
                quote = Some(ch);
            } else if ch == '>' {
                return Some(index + 1);
            }
            None
        });
        let Some(end) = end else { break };
        let tag = &remaining[..end];
        if tag.starts_with("</span") {
            if spans.pop().unwrap_or(false) {
                output.push(CLOZE_END);
            }
            output.push_str(tag);
        } else if tag.starts_with("<span ") || tag == "<span>" {
            // This exact class attribute is emitted by upstream cloze
            // rendering.
            let cloze = tag.starts_with("<span class=\"cloze\"");
            spans.push(cloze);
            output.push_str(tag);
            if cloze {
                output.push(CLOZE_START);
                found = true;
            }
        } else {
            output.push_str(tag);
        }
        remaining = &remaining[end..];
    }
    output.push_str(remaining);
    found.then_some(output)
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct PitchStyle {
    pub high: bool,
    pub low: bool,
    pub downstep: bool,
    pub upstep: bool,
    pub accent: Option<u8>,
}

/// Visual attributes for an inline run.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Style {
    pub bold: bool,
    pub italic: bool,
    pub underline: bool,
    pub sub: bool,
    pub sup: bool,
    pub cloze: bool,
    pub color: Option<[u8; 3]>,
    pub highlight: Option<[u8; 3]>,
    pub pitch: PitchStyle,
}

impl Style {
    fn apply(&mut self, other: &Style) {
        self.bold |= other.bold;
        self.italic |= other.italic;
        self.underline |= other.underline;
        self.sub |= other.sub;
        self.sup |= other.sup;
        self.cloze |= other.cloze;
        if other.color.is_some() {
            self.color = other.color;
        }
        if other.highlight.is_some() {
            self.highlight = other.highlight;
        }
        self.pitch.high |= other.pitch.high;
        self.pitch.low |= other.pitch.low;
        self.pitch.downstep |= other.pitch.downstep;
        self.pitch.upstep |= other.pitch.upstep;
        if other.pitch.accent.is_some() {
            self.pitch.accent = other.pitch.accent;
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub enum PitchMarker {
    High,
    Up,
    Down,
    Accent(u8),
    Graph(Vec<(f32, f32)>),
}

#[derive(Clone, Debug, PartialEq)]
pub enum Inline {
    Text {
        text: String,
        style: Style,
    },
    Ruby {
        base: String,
        reading: String,
        style: Style,
    },
    PitchWord {
        base: String,
        accent: u8,
        style: Style,
    },
    Marker(PitchMarker),
    Break,
}

#[derive(Clone, Debug, PartialEq)]
pub enum Block {
    Paragraph(Vec<Inline>),
    Section(Vec<Inline>),
    ListItem {
        marker: String,
        content: Vec<Inline>,
    },
    Rule,
}

#[derive(Clone, Debug)]
struct ListState {
    ordered: bool,
    next: usize,
}

#[derive(Clone, Debug)]
struct RubyState {
    base: String,
    reading: String,
    chunks: Vec<(String, String)>,
    in_rt: bool,
    in_rp: bool,
    style: Style,
}

/// An inline-block span used by AJT-style Japanese pitch-accent markup.
/// The visible text lives in one child span and an empty, absolutely
/// positioned sibling draws the high-pitch line and optional downstep.
#[derive(Clone, Copy, Debug)]
struct SpanGroup {
    wrapper: bool,
    start: usize,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
enum BlockKind {
    #[default]
    Paragraph,
    Section,
    ListItem,
}

#[derive(Default)]
struct Parser {
    blocks: Vec<Block>,
    current: Vec<Inline>,
    current_marker: Option<String>,
    block_kind: BlockKind,
    styles: Vec<(String, Style)>,
    style: Style,
    span_groups: Vec<SpanGroup>,
    cloze_depth: usize,
    lists: Vec<ListState>,
    ruby: Option<RubyState>,
    svg: Option<String>,
    skip: Option<String>,
}

/// Parse the rendered HTML used by Anki cards into simple display blocks.
pub fn parse_html(html: &str) -> Vec<Block> {
    let mut parser = Parser::default();
    let mut position = 0usize;
    while let Some(relative) = html[position..].find('<') {
        let start = position + relative;
        parser.handle_text(&html[position..start]);
        let rest = &html[start..];
        if rest.starts_with("<!--") {
            position = rest
                .find("-->")
                .map(|index| start + index + 3)
                .unwrap_or(html.len());
            continue;
        }
        if rest.starts_with("<!") {
            position = rest
                .find('>')
                .map(|index| start + index + 1)
                .unwrap_or(html.len());
            continue;
        }
        let Some(end) = tag_end(rest) else {
            parser.handle_text(rest);
            position = html.len();
            break;
        };
        parser.handle_tag(&rest[..end]);
        position = start + end;
    }
    if position < html.len() {
        parser.handle_text(&html[position..]);
    }
    parser.finish();
    parser.blocks
}

fn tag_end(text: &str) -> Option<usize> {
    let mut quote = None;
    for (index, ch) in text.char_indices() {
        if let Some(delimiter) = quote {
            if ch == delimiter {
                quote = None;
            }
        } else if ch == '\'' || ch == '"' {
            quote = Some(ch);
        } else if ch == '>' {
            return Some(index + 1);
        }
    }
    None
}

impl Parser {
    fn handle_tag(&mut self, raw: &str) {
        let mut inner = raw.trim();
        if inner.starts_with('<') {
            inner = &inner[1..];
        }
        if inner.ends_with('>') {
            inner = &inner[..inner.len() - 1];
        }
        inner = inner.trim();
        if inner.is_empty() || inner.starts_with('!') || inner.starts_with('?') {
            return;
        }
        let closing = inner.starts_with('/');
        if closing {
            inner = inner[1..].trim_start();
        }
        let self_closing = inner.ends_with('/');
        if self_closing {
            inner = inner[..inner.len() - 1].trim_end();
        }
        let name_end = inner
            .find(|ch: char| ch.is_whitespace())
            .unwrap_or(inner.len());
        let name = inner[..name_end].to_ascii_lowercase();
        if name.is_empty() {
            return;
        }

        if self.svg.is_some() {
            if closing && name == "svg" {
                let svg = self.svg.take().unwrap_or_default();
                if let Some(points) = parse_svg_points(&svg) {
                    self.current
                        .push(Inline::Marker(PitchMarker::Graph(points)));
                }
            } else if let Some(svg) = self.svg.as_mut() {
                svg.push_str(raw);
            }
            return;
        }
        if !closing && name == "svg" {
            self.svg = Some(String::new());
            return;
        }

        if let Some(skipping) = self.skip.clone() {
            if closing && name == skipping {
                self.skip = None;
            }
            return;
        }
        if !closing && matches!(name.as_str(), "style" | "script" | "head" | "title") {
            self.skip = Some(name);
            return;
        }

        if name == "ruby" {
            if closing {
                self.finish_ruby();
            } else if self.ruby.is_none() {
                self.ruby = Some(RubyState {
                    base: String::new(),
                    reading: String::new(),
                    chunks: Vec::new(),
                    in_rt: false,
                    in_rp: false,
                    style: self.style.clone(),
                });
            }
            return;
        }
        if self.ruby.is_some() {
            match name.as_str() {
                "rt" => {
                    if let Some(ruby) = self.ruby.as_mut() {
                        if closing {
                            if let Some(chunk) = ruby.chunks.last_mut() {
                                if chunk.1.is_empty() {
                                    chunk.1 = ruby.reading.trim().to_owned();
                                }
                            }
                            ruby.reading.clear();
                            ruby.in_rt = false;
                        } else {
                            if !ruby.base.trim().is_empty() {
                                ruby.chunks
                                    .push((ruby.base.trim().to_owned(), String::new()));
                                ruby.base.clear();
                            }
                            ruby.reading.clear();
                            ruby.in_rt = true;
                            ruby.in_rp = false;
                        }
                    }
                }
                "rp" => {
                    if let Some(ruby) = self.ruby.as_mut() {
                        ruby.in_rp = !closing;
                    }
                }
                // rb and formatting tags do not change how the ruby text is
                // collected; their text remains part of the base.
                _ => {}
            }
            return;
        }

        match name.as_str() {
            "br" => {
                if !closing {
                    self.current.push(Inline::Break);
                }
            }
            "hr" => {
                if !closing {
                    self.flush();
                    self.blocks.push(Block::Rule);
                }
            }
            "ul" | "ol" => {
                self.flush();
                if closing {
                    self.lists.pop();
                } else {
                    self.lists.push(ListState {
                        ordered: name == "ol",
                        next: 1,
                    });
                }
            }
            "li" => {
                self.flush();
                if !closing {
                    let marker = if let Some(list) = self.lists.last_mut() {
                        if list.ordered {
                            let marker = format!("{}. ", list.next);
                            list.next += 1;
                            marker
                        } else {
                            "• ".to_owned()
                        }
                    } else {
                        "• ".to_owned()
                    };
                    self.current_marker = Some(marker);
                    self.block_kind = BlockKind::ListItem;
                }
            }
            "p" => {
                self.flush();
                if !closing {
                    self.block_kind = BlockKind::Paragraph;
                }
            }
            "div" | "section" | "article" | "header" | "footer" | "main" | "nav" | "blockquote"
            | "address" | "aside" | "figure" | "figcaption" | "pre" | "table" | "tr" | "h1"
            | "h2" | "h3" | "h4" | "h5" | "h6" => {
                self.flush();
                if !closing {
                    self.block_kind = BlockKind::Section;
                }
            }
            _ => {
                let attributes = parse_attributes(inner, name_end);
                if closing {
                    if name == "span" {
                        self.span_groups.pop();
                    }
                    self.pop_style(&name);
                } else {
                    if name == "span" {
                        self.begin_span(&attributes);
                    }
                    let mut delta = Style::default();
                    apply_tag_style(&name, &attributes, &mut delta);
                    if has_style_effect(&name, &attributes) {
                        let mut style = self.style.clone();
                        style.apply(&delta);
                        self.styles.push((name.clone(), style.clone()));
                        self.style = style;
                    }
                    if self_closing && name == "span" {
                        self.span_groups.pop();
                    }
                }
            }
        }
    }

    fn begin_span(&mut self, attributes: &[(String, String)]) {
        let wrapper = is_pitch_wrapper(attributes);
        if is_pitch_overlay(attributes) {
            self.apply_pitch_overlay(attributes);
        }
        let start = self.current.len();
        self.span_groups.push(SpanGroup { wrapper, start });
        if wrapper {
            // Adjacent text runs with identical styles are normally merged to
            // keep the tree small. Start the group with an empty run so text
            // inside the wrapper cannot merge with mora before it.
            self.current.push(Inline::Text {
                text: String::new(),
                style: self.style.clone(),
            });
        }
    }

    fn apply_pitch_overlay(&mut self, attributes: &[(String, String)]) {
        let Some(css) = attribute(attributes, "style") else {
            return;
        };
        let mut effect = Style::default();
        apply_css(&mut effect, css);
        let high = effect.pitch.high;
        let downstep = effect.pitch.downstep;
        let upstep = effect.pitch.upstep;
        if !high && !downstep && !upstep {
            return;
        }
        let Some(start) = self
            .span_groups
            .iter()
            .rev()
            .find(|group| group.wrapper)
            .map(|group| group.start)
        else {
            return;
        };
        if start >= self.current.len() {
            return;
        }
        let mut last_text = None;
        for (offset, inline) in self.current[start..].iter_mut().enumerate() {
            if let Some(style) = inline_style_mut(inline) {
                if high {
                    style.pitch.high = true;
                }
                if upstep {
                    style.pitch.upstep = true;
                }
                last_text = Some(start + offset);
            }
        }
        if downstep {
            if let Some(index) = last_text {
                if let Some(style) = inline_style_mut(&mut self.current[index]) {
                    style.pitch.downstep = true;
                }
            }
        }
    }

    fn pop_style(&mut self, name: &str) {
        if let Some(index) = self.styles.iter().rposition(|(tag, _)| tag == name) {
            self.styles.truncate(index);
            self.style = if index == 0 {
                Style::default()
            } else {
                self.styles[index - 1].1.clone()
            };
        }
    }

    fn handle_text(&mut self, text: &str) {
        if text.is_empty() || self.skip.is_some() {
            return;
        }
        if let Some(svg) = self.svg.as_mut() {
            svg.push_str(text);
            return;
        }
        let decoded = decode_entities(text);
        if let Some(ruby) = self.ruby.as_mut() {
            if ruby.in_rp {
                return;
            }
            if ruby.in_rt {
                ruby.reading.push_str(&decoded);
            } else {
                ruby.base.push_str(&decoded);
            }
            return;
        }
        let mut segment = String::new();
        for ch in decoded.chars() {
            match ch {
                CLOZE_START => {
                    self.push_text_segment(&segment);
                    segment.clear();
                    self.cloze_depth += 1;
                }
                CLOZE_END => {
                    self.push_text_segment(&segment);
                    segment.clear();
                    self.cloze_depth = self.cloze_depth.saturating_sub(1);
                }
                _ => segment.push(ch),
            }
        }
        self.push_text_segment(&segment);
    }

    fn push_text_segment(&mut self, text: &str) {
        if text.is_empty() {
            return;
        }
        let mut style = self.style.clone();
        if self.cloze_depth > 0 {
            style.cloze = true;
        }
        push_text_with_furigana(&mut self.current, text, style);
    }

    fn finish_ruby(&mut self) {
        let Some(mut ruby) = self.ruby.take() else {
            return;
        };
        if !ruby.base.trim().is_empty() {
            ruby.chunks
                .push((ruby.base.trim().to_owned(), String::new()));
        }
        for (base, reading) in ruby.chunks {
            if base.is_empty() {
                continue;
            }
            if reading.is_empty() {
                self.current.push(Inline::Text {
                    text: base,
                    style: ruby.style.clone(),
                });
            } else {
                self.current.push(Inline::Ruby {
                    base,
                    reading,
                    style: ruby.style.clone(),
                });
            }
        }
    }

    fn flush(&mut self) {
        while matches!(self.current.first(), Some(Inline::Break)) {
            self.current.remove(0);
        }
        while matches!(self.current.last(), Some(Inline::Break)) {
            self.current.pop();
        }
        self.current.retain(|inline| match inline {
            Inline::Text { text, .. } => !text.is_empty(),
            _ => true,
        });
        if self.current.is_empty() {
            self.current_marker = None;
            self.block_kind = BlockKind::Paragraph;
            return;
        }
        let content = std::mem::take(&mut self.current);
        if let Some(marker) = self.current_marker.take() {
            self.blocks.push(Block::ListItem { marker, content });
        } else if self.block_kind == BlockKind::Section {
            self.blocks.push(Block::Section(content));
        } else {
            self.blocks.push(Block::Paragraph(content));
        }
        self.block_kind = BlockKind::Paragraph;
    }

    fn finish(&mut self) {
        self.finish_ruby();
        self.flush();
    }
}

fn has_style_effect(tag: &str, attributes: &[(String, String)]) -> bool {
    matches!(
        tag,
        "b" | "strong"
            | "i"
            | "em"
            | "u"
            | "ins"
            | "sub"
            | "sup"
            | "mark"
            | "font"
            | "span"
            | "small"
            | "s"
            | "strike"
            | "del"
            | "code"
    ) || attributes
        .iter()
        .any(|(name, _)| matches!(name.as_str(), "style" | "color" | "bgcolor" | "class"))
}

fn apply_tag_style(tag: &str, attributes: &[(String, String)], style: &mut Style) {
    match tag {
        "b" | "strong" => style.bold = true,
        "i" | "em" => style.italic = true,
        "u" | "ins" => style.underline = true,
        "sub" => style.sub = true,
        "sup" => style.sup = true,
        "mark" => style.highlight = Some([255, 235, 120]),
        _ => {}
    }
    if tag == "span"
        && attributes.iter().any(|(name, value)| {
            name == "class" && value.split_whitespace().any(|class| class == "cloze")
        })
    {
        style.cloze = true;
    }
    for (name, value) in attributes {
        match name.as_str() {
            "style" => apply_css(style, value),
            "class" => apply_pitch_classes(style, value),
            "color" => {
                if let Some(color) = parse_color(value) {
                    style.color = Some(color);
                }
            }
            "bgcolor" => {
                if let Some(color) = parse_color(value) {
                    style.highlight = Some(color);
                }
            }
            "data-accent" | "data-pitch-accent" => {
                if let Ok(accent) = value.trim().parse::<u8>() {
                    if accent <= 20 {
                        style.pitch.accent = Some(accent);
                    }
                }
            }
            "data-pitch" => apply_pitch_word(style, value),
            _ => {}
        }
    }
}

fn apply_pitch_classes(style: &mut Style, value: &str) {
    for class in value.split_whitespace() {
        apply_pitch_word(style, class);
    }
}

fn apply_pitch_word(style: &mut Style, value: &str) {
    let value = value.trim().to_ascii_lowercase();
    let value = value.trim_start_matches("pitch-");
    if matches!(value, "heiban") {
        style.pitch.accent = Some(0);
    } else if matches!(value, "atamadaka") {
        style.pitch.accent = Some(1);
    } else if matches!(value, "nakadaka" | "odaka") {
        style.pitch.high = true;
        style.pitch.downstep = true;
    } else if matches!(value, "high" | "top" | "accent-high" | "accent-top") {
        style.pitch.high = true;
    } else if matches!(value, "low" | "bottom" | "accent-low") {
        style.pitch.low = true;
    } else if matches!(value, "downstep" | "down" | "drop" | "accent-drop" | "fall") {
        style.pitch.downstep = true;
    } else if matches!(
        value,
        "upstep" | "up" | "rise" | "accent-rise" | "accent-up"
    ) {
        style.pitch.upstep = true;
    } else if let Some(number) = value
        .strip_prefix("accent-")
        .or_else(|| value.strip_prefix("accent_"))
        .and_then(|number| number.parse::<u8>().ok())
        .or_else(|| value.parse::<u8>().ok())
    {
        if number <= 20 {
            style.pitch.accent = Some(number);
        }
    }
}

fn apply_css(style: &mut Style, css: &str) {
    for declaration in css.split(';') {
        let Some((key, value)) = declaration.split_once(':') else {
            continue;
        };
        let key = key.trim().to_ascii_lowercase();
        let value = value.trim().trim_end_matches("!important").trim();
        match key.as_str() {
            "color" => {
                if let Some(color) = parse_color(value) {
                    style.color = Some(color);
                }
            }
            "background-color" | "background" => {
                let color = parse_color(value)
                    .or_else(|| value.split_whitespace().find_map(|part| parse_color(part)));
                if let Some(color) = color {
                    style.highlight = Some(color);
                }
            }
            "font-weight" => {
                if value.eq_ignore_ascii_case("bold")
                    || value
                        .parse::<u32>()
                        .map(|weight| weight >= 600)
                        .unwrap_or(false)
                {
                    style.bold = true;
                }
            }
            "font-style" => {
                if value.eq_ignore_ascii_case("italic") || value.eq_ignore_ascii_case("oblique") {
                    style.italic = true;
                }
            }
            "text-decoration" | "text-decoration-line" => {
                let value = value.to_ascii_lowercase();
                if value.contains("underline") {
                    style.underline = true;
                }
                if value.contains("overline") {
                    style.pitch.high = true;
                }
            }
            "border-top"
            | "border-top-width"
            | "border-top-style"
            | "border-block-start"
            | "border-block-start-width"
            | "border-block-start-style" => {
                if !matches!(
                    value.to_ascii_lowercase().as_str(),
                    "none" | "hidden" | "0" | "0px"
                ) {
                    style.pitch.high = true;
                }
            }
            "border-right"
            | "border-right-width"
            | "border-right-style"
            | "border-inline-end"
            | "border-inline-end-width"
            | "border-inline-end-style" => {
                if !matches!(
                    value.to_ascii_lowercase().as_str(),
                    "none" | "hidden" | "0" | "0px"
                ) {
                    style.pitch.downstep = true;
                }
            }
            _ => {}
        }
    }
}

fn parse_color(value: &str) -> Option<[u8; 3]> {
    let value = value
        .trim()
        .trim_matches(|ch| ch == '\'' || ch == '"')
        .trim();
    if let Some(hex) = value.strip_prefix('#') {
        let digits: Vec<u8> = hex
            .chars()
            .map(|ch| ch.to_digit(16).map(|digit| digit as u8))
            .collect::<Option<_>>()?;
        return match digits.len() {
            3 => Some([digits[0] * 17, digits[1] * 17, digits[2] * 17]),
            4 => Some([digits[0] * 17, digits[1] * 17, digits[2] * 17]),
            6 => Some([
                digits[0] * 16 + digits[1],
                digits[2] * 16 + digits[3],
                digits[4] * 16 + digits[5],
            ]),
            8 => Some([
                digits[0] * 16 + digits[1],
                digits[2] * 16 + digits[3],
                digits[4] * 16 + digits[5],
            ]),
            _ => None,
        };
    }
    let lower = value.to_ascii_lowercase();
    if let Some(inner) = lower
        .strip_prefix("rgb(")
        .and_then(|rest| rest.strip_suffix(')'))
    {
        let parts: Vec<u8> = inner
            .split(',')
            .filter_map(|part| part.trim().split_whitespace().next())
            .filter_map(|part| part.trim_end_matches('%').parse::<u16>().ok())
            .map(|part| (part.min(255)) as u8)
            .collect();
        if parts.len() >= 3 {
            return Some([parts[0], parts[1], parts[2]]);
        }
    }
    Some(match lower.as_str() {
        "black" => [0, 0, 0],
        "white" => [255, 255, 255],
        "red" => [220, 40, 30],
        "green" => [30, 140, 50],
        "blue" => [30, 80, 210],
        "royalblue" => [65, 105, 225],
        "yellow" => [255, 235, 60],
        "orange" => [235, 140, 30],
        "purple" => [130, 60, 170],
        "gray" | "grey" => [120, 120, 120],
        "brown" => [130, 85, 45],
        "pink" => [235, 130, 170],
        "cyan" | "aqua" => [30, 180, 190],
        "magenta" | "fuchsia" => [200, 50, 170],
        "lime" => [70, 210, 70],
        "navy" => [20, 35, 110],
        "teal" => [20, 120, 120],
        "maroon" => [120, 25, 35],
        "olive" => [120, 120, 30],
        "silver" => [190, 190, 190],
        "gold" => [210, 170, 40],
        _ => return None,
    })
}

fn parse_attributes(inner: &str, name_end: usize) -> Vec<(String, String)> {
    let mut attributes = Vec::new();
    let mut rest = inner[name_end..].trim();
    while !rest.is_empty() {
        let name_end = rest
            .find(|ch: char| ch.is_whitespace() || ch == '=')
            .unwrap_or(rest.len());
        let name = rest[..name_end].to_ascii_lowercase();
        rest = rest[name_end..].trim_start();
        if name.is_empty() {
            break;
        }
        if let Some(after_eq) = rest.strip_prefix('=') {
            let after_eq = after_eq.trim_start();
            let (value, remainder) =
                if let Some(quote) = after_eq.chars().next().filter(|c| *c == '"' || *c == '\'') {
                    let quoted = &after_eq[quote.len_utf8()..];
                    let end = quoted.find(quote).unwrap_or(quoted.len());
                    (&quoted[..end], &quoted[end + quote.len_utf8()..])
                } else {
                    let end = after_eq
                        .find(|ch: char| ch.is_whitespace())
                        .unwrap_or(after_eq.len());
                    (&after_eq[..end], &after_eq[end..])
                };
            attributes.push((name, value.to_owned()));
            rest = remainder.trim_start();
        } else {
            attributes.push((name, String::new()));
        }
    }
    attributes
}

fn attribute<'a>(attributes: &'a [(String, String)], name: &str) -> Option<&'a str> {
    attributes
        .iter()
        .find(|(key, _)| key.eq_ignore_ascii_case(name))
        .map(|(_, value)| value.as_str())
}

fn inline_style_mut(inline: &mut Inline) -> Option<&mut Style> {
    match inline {
        Inline::Text { style, .. }
        | Inline::Ruby { style, .. }
        | Inline::PitchWord { style, .. } => Some(style),
        Inline::Marker(_) | Inline::Break => None,
    }
}

fn css_property<'a>(css: &'a str, name: &str) -> Option<&'a str> {
    css.split(';').find_map(|declaration| {
        let (key, value) = declaration.split_once(':')?;
        if key.trim().eq_ignore_ascii_case(name) {
            Some(value.trim().trim_end_matches("!important").trim())
        } else {
            None
        }
    })
}

fn is_pitch_wrapper(attributes: &[(String, String)]) -> bool {
    let Some(css) = attribute(attributes, "style") else {
        return false;
    };
    css_property(css, "display").is_some_and(|value| value.eq_ignore_ascii_case("inline-block"))
        && css_property(css, "position").is_some_and(|value| value.eq_ignore_ascii_case("relative"))
}

fn is_pitch_overlay(attributes: &[(String, String)]) -> bool {
    let Some(css) = attribute(attributes, "style") else {
        return false;
    };
    if !css_property(css, "position").is_some_and(|value| value.eq_ignore_ascii_case("absolute")) {
        return false;
    }
    let mut effect = Style::default();
    apply_css(&mut effect, css);
    effect.pitch.high || effect.pitch.downstep || effect.pitch.upstep
}

fn push_text_with_furigana(output: &mut Vec<Inline>, text: &str, style: Style) {
    let mut rest = text;
    while let Some(open) = rest.find('[') {
        let after = &rest[open + 1..];
        let Some(close) = after.find(']') else {
            push_text(output, rest, style.clone());
            return;
        };
        let reading = after[..close].trim();
        let before = &rest[..open];
        if let Some(accent) = reading.parse::<u8>().ok().filter(|accent| *accent <= 20) {
            if let Some(start) = furigana_base_start(before) {
                let base = before[start..].trim_end();
                if !base.is_empty() {
                    push_text(output, &before[..start], style.clone());
                    output.push(Inline::PitchWord {
                        base: base.to_owned(),
                        accent,
                        style: style.clone(),
                    });
                    rest = &after[close + 1..];
                    continue;
                }
            }
        }
        let valid = !reading.is_empty()
            && !reading.starts_with("sound:")
            && !reading.starts_with("anki:")
            && !reading.contains('<')
            && !reading.contains('>');
        if valid {
            if let Some(start) = furigana_base_start(before) {
                let base = before[start..].trim_end();
                if !base.is_empty() {
                    push_text(output, &before[..start], style.clone());
                    output.push(Inline::Ruby {
                        base: base.to_owned(),
                        reading: reading.to_owned(),
                        style: style.clone(),
                    });
                    rest = &after[close + 1..];
                    continue;
                }
            }
        }
        let end = open + 1 + close + 1;
        push_text(output, &rest[..end], style.clone());
        rest = &rest[end..];
    }
    if !rest.is_empty() {
        push_text(output, rest, style);
    }
}

fn push_text(output: &mut Vec<Inline>, text: &str, style: Style) {
    if text.is_empty() {
        return;
    }
    let mut rest = text;
    while let Some((index, ch, marker)) = rest
        .char_indices()
        .find_map(|(index, ch)| pitch_symbol(ch).map(|marker| (index, ch, marker)))
    {
        push_text_plain(output, &rest[..index], style.clone());
        output.push(Inline::Marker(marker));
        rest = &rest[index + ch.len_utf8()..];
    }
    push_text_plain(output, rest, style);
}

fn push_text_plain(output: &mut Vec<Inline>, text: &str, style: Style) {
    if text.is_empty() {
        return;
    }
    if let Some(Inline::Text {
        text: previous,
        style: previous_style,
    }) = output.last_mut()
    {
        if *previous_style == style {
            previous.push_str(text);
            return;
        }
    }
    output.push(Inline::Text {
        text: text.to_owned(),
        style,
    });
}

fn pitch_symbol(ch: char) -> Option<PitchMarker> {
    Some(match ch {
        '↗' | '↑' | '⤴' | '／' => PitchMarker::Up,
        '↘' | '↓' | '⤵' | '＼' => PitchMarker::Down,
        '‾' | '¯' | '\u{0304}' => PitchMarker::High,
        '①'..='⑳' => PitchMarker::Accent((ch as u32 - 0x2460 + 1) as u8),
        _ => return None,
    })
}

fn furigana_base_start(text: &str) -> Option<usize> {
    // Anki's bracket syntax is attached directly to the base. A space before
    // the bracket means this is ordinary bracketed text.
    if text
        .chars()
        .next_back()
        .map(char::is_whitespace)
        .unwrap_or(true)
    {
        return None;
    }
    let (last_index, last) = text.char_indices().next_back()?;
    let mut start = last_index;
    if is_kanji(last) {
        for (index, ch) in text[..last_index].char_indices().rev() {
            if is_kanji(ch) || matches!(ch, '\u{3005}' | '\u{3006}' | '\u{3007}' | '\u{30fc}') {
                start = index;
            } else {
                break;
            }
        }
    } else if last.is_alphanumeric() {
        for (index, ch) in text[..last_index].char_indices().rev() {
            if ch.is_alphanumeric() || matches!(ch, '\'' | '’' | '-' | '_') {
                start = index;
            } else {
                break;
            }
        }
    } else {
        return None;
    }
    Some(start)
}

fn is_kanji(ch: char) -> bool {
    matches!(
        ch as u32,
        0x3400..=0x4dbf
            | 0x4e00..=0x9fff
            | 0xf900..=0xfaff
            | 0x20000..=0x2fa1f
    )
}

fn decode_entities(text: &str) -> String {
    let mut output = String::with_capacity(text.len());
    let mut rest = text;
    while let Some(index) = rest.find('&') {
        output.push_str(&rest[..index]);
        rest = &rest[index..];
        let Some(end) = rest.find(';') else {
            output.push_str(rest);
            return output;
        };
        let entity = &rest[1..end];
        let decoded = if let Some(number) = entity.strip_prefix('#') {
            let number = if let Some(hex) = number
                .strip_prefix('x')
                .or_else(|| number.strip_prefix('X'))
            {
                u32::from_str_radix(hex, 16).ok()
            } else {
                number.parse::<u32>().ok()
            };
            number.and_then(char::from_u32)
        } else {
            named_entity(entity)
        };
        if let Some(ch) = decoded {
            if ch == '\u{00a0}' {
                output.push(' ');
            } else {
                output.push(ch);
            }
            rest = &rest[end + 1..];
        } else {
            output.push('&');
            rest = &rest[1..];
        }
    }
    output.push_str(rest);
    output
}

fn named_entity(entity: &str) -> Option<char> {
    Some(match entity {
        "amp" => '&',
        "lt" => '<',
        "gt" => '>',
        "quot" => '"',
        "apos" => '\'',
        "nbsp" => ' ',
        "ensp" => '\u{2002}',
        "emsp" => '\u{2003}',
        "thinsp" => '\u{2009}',
        "ndash" => '–',
        "mdash" => '—',
        "hellip" => '…',
        "lsquo" => '‘',
        "rsquo" => '’',
        "sbquo" => '‚',
        "ldquo" => '“',
        "rdquo" => '”',
        "bdquo" => '„',
        "laquo" => '«',
        "raquo" => '»',
        "copy" => '©',
        "reg" => '®',
        "trade" => '™',
        "deg" => '°',
        "plusmn" => '±',
        "times" => '×',
        "divide" => '÷',
        "frac12" => '½',
        "frac14" => '¼',
        "frac34" => '¾',
        "euro" => '€',
        "pound" => '£',
        "yen" => '¥',
        "cent" => '¢',
        "sect" => '§',
        "para" => '¶',
        "middot" => '·',
        "bull" => '•',
        "dagger" => '†',
        "Dagger" => '‡',
        "larr" => '←',
        "uarr" => '↑',
        "rarr" => '→',
        "darr" => '↓',
        "harr" => '↔',
        "spades" => '♠',
        "clubs" => '♣',
        "hearts" => '♥',
        "diams" => '♦',
        "micro" => 'µ',
        "sup1" => '¹',
        "sup2" => '²',
        "sup3" => '³',
        _ => return None,
    })
}

fn parse_svg_points(svg: &str) -> Option<Vec<(f32, f32)>> {
    let data = attribute_value(svg, "points").or_else(|| attribute_value(svg, "d"))?;
    let values = svg_numbers(data);
    if values.len() < 4 {
        return None;
    }
    let mut points: Vec<(f32, f32)> = values
        .chunks_exact(2)
        .take(64)
        .map(|pair| (pair[0], pair[1]))
        .collect();
    let min_x = points
        .iter()
        .map(|point| point.0)
        .fold(f32::INFINITY, f32::min);
    let max_x = points
        .iter()
        .map(|point| point.0)
        .fold(f32::NEG_INFINITY, f32::max);
    let min_y = points
        .iter()
        .map(|point| point.1)
        .fold(f32::INFINITY, f32::min);
    let max_y = points
        .iter()
        .map(|point| point.1)
        .fold(f32::NEG_INFINITY, f32::max);
    let width = (max_x - min_x).max(1.0);
    let height = (max_y - min_y).max(1.0);
    for point in &mut points {
        point.0 = (point.0 - min_x) / width;
        point.1 = 1.0 - (point.1 - min_y) / height;
    }
    Some(points)
}

fn attribute_value<'a>(text: &'a str, name: &str) -> Option<&'a str> {
    for (index, _) in text.match_indices(name) {
        let before = text[..index].chars().next_back();
        if before.is_some_and(|ch| ch.is_alphanumeric() || matches!(ch, '-' | '_')) {
            continue;
        }
        let after = text[index + name.len()..].trim_start();
        let Some(after) = after.strip_prefix('=') else {
            continue;
        };
        let after = after.trim_start();
        let quote = after.chars().next().filter(|ch| *ch == '"' || *ch == '\'');
        if let Some(quote) = quote {
            let quoted = &after[quote.len_utf8()..];
            let end = quoted.find(quote)?;
            return Some(&quoted[..end]);
        }
        let end = after
            .find(|ch: char| ch.is_whitespace() || ch == '>')
            .unwrap_or(after.len());
        return Some(&after[..end]);
    }
    None
}

fn svg_numbers(data: &str) -> Vec<f32> {
    let bytes = data.as_bytes();
    let mut values = Vec::new();
    let mut index = 0usize;
    while index < bytes.len() {
        while index < bytes.len() && !matches!(bytes[index], b'0'..=b'9' | b'+' | b'-' | b'.') {
            index += 1;
        }
        let start = index;
        if index < bytes.len() && matches!(bytes[index], b'+' | b'-') {
            index += 1;
        }
        let mut digits = false;
        while index < bytes.len() && bytes[index].is_ascii_digit() {
            digits = true;
            index += 1;
        }
        if index < bytes.len() && bytes[index] == b'.' {
            index += 1;
            while index < bytes.len() && bytes[index].is_ascii_digit() {
                digits = true;
                index += 1;
            }
        }
        if digits && index < bytes.len() && matches!(bytes[index], b'e' | b'E') {
            index += 1;
            if index < bytes.len() && matches!(bytes[index], b'+' | b'-') {
                index += 1;
            }
            while index < bytes.len() && bytes[index].is_ascii_digit() {
                index += 1;
            }
        }
        if digits {
            if let Ok(value) = data[start..index].parse::<f32>() {
                values.push(value);
            }
        } else {
            index = index.max(start + 1);
        }
    }
    values
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn preserves_nested_markup_and_multiple_upstream_clozes() {
        let html = "A <span class=\"cloze\" data-ordinal=\"1\">blue <span style=\"x>y\">nested</span></span> and <span class=\"cloze\" data-ordinal=\"1\">two</span>";
        let marked = mark_clozes(html).unwrap();
        assert_eq!(plain(&marked), html);
        assert!(marked.contains("\u{e000}blue <span style=\"x>y\">nested</span>\u{e001}</span>"));
        assert_eq!(marked.matches(CLOZE_START).count(), 2);
        assert_eq!(marked.matches(CLOZE_END).count(), 2);
    }

    #[test]
    fn basic_cards_do_not_change() {
        assert!(mark_clozes("plain <span>text</span>").is_none());
        assert!(mark_clozes("<span class=\"not-cloze\">x</span>").is_none());
    }

    #[test]
    fn entities_and_inline_styles_are_decoded() {
        let blocks = parse_html(
            "<b>A &amp; B</b> <i>c</i> <u>d</u> <sub>2</sub><sup>3</sup> &lt;ok&gt; &#65; &#x42;",
        );
        let Block::Paragraph(content) = &blocks[0] else {
            panic!("expected paragraph");
        };
        let text: String = content
            .iter()
            .map(|inline| match inline {
                Inline::Text { text, .. } => text.as_str(),
                _ => "",
            })
            .collect();
        assert_eq!(text, "A & B c d 23 <ok> A B");
        assert!(content.iter().any(
            |inline| matches!(inline, Inline::Text { text, style, .. } if text == "A & B" && style.bold)
        ));
        assert!(content.iter().any(
            |inline| matches!(inline, Inline::Text { text, style, .. } if text == "c" && style.italic)
        ));
        assert!(content.iter().any(
            |inline| matches!(inline, Inline::Text { text, style, .. } if text == "d" && style.underline)
        ));
        assert!(content.iter().any(
            |inline| matches!(inline, Inline::Text { text, style, .. } if text == "2" && style.sub)
        ));
        assert!(content.iter().any(
            |inline| matches!(inline, Inline::Text { text, style, .. } if text == "3" && style.sup)
        ));
    }

    #[test]
    fn html_ruby_ignores_rp_and_supports_multiple_bases() {
        let blocks = parse_html("<ruby>漢<rp>(</rp><rt>かん</rt><rp>)</rp>字<rt>じ</rt></ruby>");
        let Block::Paragraph(content) = &blocks[0] else {
            panic!("expected paragraph");
        };
        assert_eq!(
            content,
            &[
                Inline::Ruby {
                    base: "漢".into(),
                    reading: "かん".into(),
                    style: Style::default()
                },
                Inline::Ruby {
                    base: "字".into(),
                    reading: "じ".into(),
                    style: Style::default()
                }
            ]
        );
    }

    #[test]
    fn anki_bracket_furigana_and_plain_brackets_are_distinguished() {
        let blocks = parse_html("私は漢字[かんじ]です。 See [notes].");
        let Block::Paragraph(content) = &blocks[0] else {
            panic!("expected paragraph");
        };
        assert!(matches!(
            &content[1],
            Inline::Ruby { base, reading, .. } if base == "漢字" && reading == "かんじ"
        ));
        let text: String = content
            .iter()
            .map(|inline| match inline {
                Inline::Text { text, .. } => text.as_str(),
                Inline::Ruby { base, reading, .. } => {
                    Box::leak(format!("{base}[{reading}]").into_boxed_str())
                }
                _ => "",
            })
            .collect();
        assert!(text.contains("See [notes]."));
    }

    #[test]
    fn block_elements_lists_and_rules_are_preserved() {
        let blocks = parse_html(
            "<p>one<br>two</p><ul><li>alpha</li><li>beta</li></ul><hr><ol><li>first</li></ol><div>last</div>",
        );
        assert!(matches!(blocks[0], Block::Paragraph(_)));
        assert!(matches!(
            blocks[1],
            Block::ListItem { ref marker, .. } if marker == "• "
        ));
        assert!(matches!(
            blocks[2],
            Block::ListItem { ref marker, .. } if marker == "• "
        ));
        assert!(matches!(blocks[3], Block::Rule));
        assert!(matches!(
            blocks[4],
            Block::ListItem { ref marker, .. } if marker == "1. "
        ));
        assert!(matches!(blocks[5], Block::Section(_)));
    }

    #[test]
    fn pitch_accents_and_symbols_are_parsed() {
        let blocks = parse_html(
            "<span class=\"pitch-high\">高</span><span style=\"border-right: 1px solid\">低</span> <span class=\"atamadaka\">箸</span> 箸[1] ①",
        );
        let Block::Paragraph(content) = &blocks[0] else {
            panic!("expected paragraph");
        };
        assert!(content.iter().any(|inline| matches!(
            inline,
            Inline::Text { text, style, .. } if text == "高" && style.pitch.high
        )));
        assert!(content.iter().any(|inline| matches!(
            inline,
            Inline::Text { text, style, .. } if text == "低" && style.pitch.downstep
        )));
        assert!(content.iter().any(|inline| matches!(
            inline,
            Inline::Text { text, style, .. } if text == "箸" && style.pitch.accent == Some(1)
        )));
        assert!(content.iter().any(|inline| matches!(
            inline,
            Inline::PitchWord { base, accent, .. } if base == "箸" && *accent == 1
        )));
        assert!(content
            .iter()
            .any(|inline| matches!(inline, Inline::Marker(PitchMarker::Accent(1)))));

        let blocks = parse_html("箸[1]");
        let Block::Paragraph(content) = &blocks[0] else {
            panic!("expected paragraph");
        };
        assert!(content
            .iter()
            .any(|inline| matches!(inline, Inline::PitchWord { base, accent, .. } if base == "箸" && *accent == 1)));
        assert!(!content
            .iter()
            .any(|inline| matches!(inline, Inline::Marker(_))));
    }

    #[test]
    fn ajt_pitch_overlays_apply_to_the_preceding_group() {
        fn runs(html: &str) -> Vec<(String, bool, bool, Option<[u8; 3]>)> {
            let blocks = parse_html(html);
            let Block::Paragraph(content) = &blocks[0] else {
                panic!("expected paragraph");
            };
            content
                .iter()
                .filter_map(|inline| match inline {
                    Inline::Text { text, style } if !text.is_empty() => Some((
                        text.clone(),
                        style.pitch.high,
                        style.pitch.downstep,
                        style.color,
                    )),
                    _ => None,
                })
                .collect()
        }

        assert_eq!(
            runs(
                r#"ワ<span style="display:inline-block;position:relative;"><span style="display:inline;">タシ</span><span style="border-color:currentColor;display:block;user-select:none;pointer-events:none;position:absolute;top:0.1em;left:0;right:0;height:0;border-top-width:0.1em;border-top-style:solid;"></span></span>"#
            ),
            vec![
                ("ワ".into(), false, false, None),
                ("タシ".into(), true, false, None),
            ]
        );

        assert_eq!(
            runs(
                r#"ア<span style="display:inline-block;position:relative;padding-right:0.1em;margin-right:0.1em;"><span style="display:inline;">ナ</span><span style="border-color:currentColor;display:block;user-select:none;pointer-events:none;position:absolute;top:0.1em;left:0;right:0;height:0;border-top-width:0.1em;border-top-style:solid;right:-0.1em;height:0.4em;border-right-width:0.1em;border-right-style:solid;"></span></span>タ"#
            ),
            vec![
                ("ア".into(), false, false, None),
                ("ナ".into(), true, true, None),
                ("タ".into(), false, false, None),
            ]
        );

        assert_eq!(
            runs(
                r#"<span style="color: royalblue;">ス</span><span style="display:inline-block;position:relative;padding-right:0.1em;margin-right:0.1em;"><span style="display:inline;">キ</span><span style="border-color:currentColor;display:block;user-select:none;pointer-events:none;position:absolute;top:0.1em;left:0;right:0;height:0;border-top-width:0.1em;border-top-style:solid;right:-0.1em;height:0.4em;border-right-width:0.1em;border-right-style:solid;"></span></span>"#
            ),
            vec![
                ("ス".into(), false, false, Some([65, 105, 225])),
                ("キ".into(), true, true, None),
            ]
        );

        assert_eq!(
            runs(
                r#"ニ<span style="display:inline-block;position:relative;"><span style="display:inline;">ホンコ<span style="color: red;">°</span></span><span style="border-color:currentColor;display:block;user-select:none;pointer-events:none;position:absolute;top:0.1em;left:0;right:0;height:0;border-top-width:0.1em;border-top-style:solid;"></span></span>"#
            ),
            vec![
                ("ニ".into(), false, false, None),
                ("ホンコ".into(), true, false, None),
                ("°".into(), true, false, Some([220, 40, 30])),
            ]
        );

        assert_eq!(
            runs(r#"ベ<span style="text-decoration:overline;">ンキョー</span>"#),
            vec![
                ("ベ".into(), false, false, None),
                ("ンキョー".into(), true, false, None),
            ]
        );
    }

    #[test]
    fn inline_svg_pitch_graph_is_parsed() {
        let blocks = parse_html(
            "<svg viewBox=\"0 0 100 20\"><polyline points=\"0,15 20,15 20,5 80,5 80,15\"/></svg>",
        );
        let Block::Paragraph(content) = &blocks[0] else {
            panic!("expected paragraph");
        };
        assert!(content.iter().any(|inline| match inline {
            Inline::Marker(PitchMarker::Graph(points)) => {
                points.len() == 5 && points[0].1 < 0.001 && (points[2].1 - 1.0).abs() < 0.001
            }
            _ => false,
        }));

        let blocks = parse_html("<svg><path id=\"curve\" d=\"M0 10 L10 10 L10 5\"/></svg>");
        let Block::Paragraph(content) = &blocks[0] else {
            panic!("expected paragraph");
        };
        assert!(content.iter().any(|inline| matches!(
            inline,
            Inline::Marker(PitchMarker::Graph(points)) if points.len() == 3
        )));
    }

    #[test]
    fn style_script_and_head_text_is_not_rendered() {
        let blocks = parse_html(
            "<style>.card { color: red }</style><script>alert('x')</script><head><title>Hidden</title></head><p>Visible</p>",
        );
        let content: String = blocks
            .iter()
            .flat_map(|block| match block {
                Block::Paragraph(content) => content.as_slice(),
                _ => &[],
            })
            .map(|inline| match inline {
                Inline::Text { text, .. } => text.as_str(),
                _ => "",
            })
            .collect();
        assert_eq!(content, "Visible");
    }

    #[test]
    fn unclosed_tag_text_is_emitted_once() {
        let blocks = parse_html("before <broken");
        let Block::Paragraph(content) = &blocks[0] else {
            panic!("expected paragraph");
        };
        let text: String = content
            .iter()
            .map(|inline| match inline {
                Inline::Text { text, .. } => text.as_str(),
                _ => "",
            })
            .collect();
        assert_eq!(text, "before <broken");
    }

    #[test]
    fn colors_and_cloze_markers_are_captured() {
        let html = "<span style=\"color: #ff0000; background-color: yellow\">red</span> <span class=\"cloze\">\u{e000}answer\u{e001}</span>";
        let blocks = parse_html(html);
        let Block::Paragraph(content) = &blocks[0] else {
            panic!("expected paragraph");
        };
        assert!(content.iter().any(|inline| matches!(
            inline,
            Inline::Text { text, style, .. }
                if text == "red"
                    && style.color == Some([255, 0, 0])
                    && style.highlight == Some([255, 235, 60])
        )));
        assert!(content.iter().any(|inline| matches!(
            inline,
            Inline::Text { text, style, .. } if text == "answer" && style.cloze
        )));
    }
}
