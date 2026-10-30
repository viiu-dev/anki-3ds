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
///
/// Both the classic `class="cloze"` markup and newer variants (extra classes,
/// single quotes, `data-cloze`/`data-ordinal` attributes) are recognised, as
/// are `cloze-inactive` spans, which are tracked for nesting but get no
/// markers of their own (the parser styles them muted via the class). Cloze
/// hints such as `[...]` sit inside the active span, so they keep the active
/// style automatically.
pub fn mark_clozes(html: &str) -> Option<String> {
    #[derive(Clone, Copy, PartialEq, Eq)]
    enum Span {
        Other,
        Active,
        Inactive,
    }
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
        if is_span_close(tag) {
            if spans.pop().unwrap_or(Span::Other) == Span::Active {
                output.push(CLOZE_END);
            }
            output.push_str(tag);
        } else if is_span_open(tag) {
            let kind = if tag_is_active_cloze(tag) {
                Span::Active
            } else if tag_is_inactive_cloze(tag) {
                Span::Inactive
            } else {
                Span::Other
            };
            spans.push(kind);
            output.push_str(tag);
            if kind == Span::Active {
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

fn tag_name_is(tag: &str, name: &str) -> bool {
    let bytes = tag.as_bytes();
    if bytes.first() != Some(&b'<') {
        return false;
    }
    let mut rest = &tag[1..];
    if let Some(stripped) = rest.strip_prefix('/') {
        rest = stripped.trim_start();
    }
    rest.len() >= name.len()
        && rest[..name.len()].eq_ignore_ascii_case(name)
        && rest[name.len()..]
            .chars()
            .next()
            .is_none_or(|ch| !(ch.is_alphanumeric() || ch == '-' || ch == '_'))
}

fn is_span_open(tag: &str) -> bool {
    tag_name_is(tag, "span") && !tag.trim_start_matches('<').starts_with('/')
}

fn is_span_close(tag: &str) -> bool {
    tag_name_is(tag, "span") && tag.trim_start_matches('<').starts_with('/')
}

/// Read a `name=value` attribute from a raw tag. Handles double quotes,
/// single quotes and bare values; the name match is case-insensitive.
fn tag_attribute<'a>(tag: &'a str, name: &str) -> Option<&'a str> {
    let lower = tag.to_ascii_lowercase();
    let mut search = lower.as_str();
    while let Some(mut index) = search.find(name.to_lowercase().as_str()) {
        // `search` borrows `lower`, so map the index back onto `tag`.
        index += lower.len() - search.len();
        let before = tag[..index].chars().next_back();
        if before.is_some_and(|ch| ch.is_alphanumeric() || matches!(ch, '-' | '_' | ':')) {
            search = &lower[index + name.len()..];
            continue;
        }
        let after = tag[index + name.len()..].trim_start();
        let Some(value) = after.strip_prefix('=') else {
            search = &lower[index + name.len()..];
            continue;
        };
        let value = value.trim_start();
        let start = tag.len() - value.len();
        if let Some(quote) = value.chars().next().filter(|c| *c == '"' || *c == '\'') {
            let quoted = &value[quote.len_utf8()..];
            let end = quoted.find(quote).unwrap_or(quoted.len());
            return Some(&tag[start + quote.len_utf8()..start + quote.len_utf8() + end]);
        }
        let end = value
            .find(|ch: char| ch.is_whitespace() || ch == '>')
            .unwrap_or(value.len());
        return Some(&tag[start..start + end]);
    }
    None
}

fn tag_is_active_cloze(tag: &str) -> bool {
    if let Some(class) = tag_attribute(tag, "class") {
        let mut active = false;
        for token in class.split_whitespace() {
            if token.eq_ignore_ascii_case("cloze-inactive") {
                return false;
            }
            if token.eq_ignore_ascii_case("cloze") {
                active = true;
            }
        }
        if active {
            return true;
        }
    }
    tag_attribute(tag, "data-cloze").is_some() || tag_attribute(tag, "data-ordinal").is_some()
}

fn tag_is_inactive_cloze(tag: &str) -> bool {
    tag_attribute(tag, "class").is_some_and(|class| {
        class
            .split_whitespace()
            .any(|token| token.eq_ignore_ascii_case("cloze-inactive"))
    })
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct PitchStyle {
    pub high: bool,
    pub low: bool,
    pub downstep: bool,
    pub upstep: bool,
    pub nasal: bool,
    pub nasal_color: Option<[u8; 3]>,
    pub accent: Option<u8>,
}

/// Baseline shift for an inline run. The parser keeps the legacy `sub`/`sup`
/// booleans on [`Style`] in sync with this value.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Script {
    #[default]
    Normal,
    Sub,
    Sup,
}

/// Diff colouring for type-answer comparisons (`typeGood`/`typeBad`/
/// `typeMissed` spans emitted by the backend's answer comparison).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DiffClass {
    Good,
    Bad,
    Missed,
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
    /// Active cloze deletion. [`Style::cloze`] mirrors this field.
    pub cloze_active: bool,
    /// Inactive cloze deletion (muted colour, no cloze background).
    pub cloze_inactive: bool,
    /// Monospace (`<code>` or a type-answer comparison block).
    pub mono: bool,
    pub script: Script,
    pub diff: Option<DiffClass>,
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
        self.mono |= other.mono;
        if other.script != Script::Normal {
            self.script = other.script;
        }
        if other.diff.is_some() {
            self.diff = other.diff;
        }
        if other.cloze_inactive {
            self.cloze_inactive = true;
            self.cloze = false;
            self.cloze_active = false;
        }
        if other.cloze || other.cloze_active {
            self.cloze = true;
            self.cloze_active = true;
            self.cloze_inactive = false;
        }
        // Keep the legacy flags in sync with the structured fields.
        match self.script {
            Script::Sub => self.sub = true,
            Script::Sup => self.sup = true,
            Script::Normal => {}
        }
        if self.sub && self.script == Script::Normal {
            self.script = Script::Sub;
        } else if self.sup && self.script == Script::Normal {
            self.script = Script::Sup;
        }
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
        self.pitch.nasal |= other.pitch.nasal;
        if other.pitch.nasal_color.is_some() {
            self.pitch.nasal_color = other.pitch.nasal_color;
        }
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
    /// Placeholder box for a `[[type:...]]` answer field on the question
    /// side. The typed text is supplied at layout time.
    TypeInput,
    /// Pill button that toggles a [`Block::Hint`]. The `id` is the order the
    /// hint appeared in the card HTML.
    HintToggle {
        id: u16,
        label: String,
    },
    /// Animated or video media (`<video>`, `<img src=*.gif|*.webp>` and
    /// synthesized `<video>` markers for video `[sound:...]` tags).
    Media {
        path: String,
        kind: MediaKind,
        autoplay: bool,
        loop_: bool,
    },
}

/// How a media reference should be presented. The backend classifies every
/// `[sound:...]`/`<img>` reference with [`media_kind_for_filename`]; the
/// parser uses the same helper for inline `<video>`/`<source>`/`<img>` tags.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum MediaKind {
    #[default]
    Image,
    Audio,
    Video,
}

/// Classify a media filename by extension. Video extensions (mp4, webm, avi,
/// mov, mkv) and animated images (gif, webp) are `Video` so that
/// `[sound:clip.mp4]` is routed to the animated-media player instead of the
/// audio/image paths.
pub fn media_kind_for_filename(filename: &str) -> MediaKind {
    let ext = filename
        .rsplit('.')
        .next()
        .unwrap_or("")
        .to_ascii_lowercase();
    match ext.as_str() {
        "mp4" | "webm" | "avi" | "mov" | "mkv" | "gif" | "webp" => MediaKind::Video,
        "mp3" | "ogg" | "oga" | "opus" | "wav" | "wave" => MediaKind::Audio,
        _ => MediaKind::Image,
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct Cell {
    pub header: bool,
    pub colspan: u8,
    pub content: Vec<Inline>,
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
    Table {
        rows: Vec<Vec<Cell>>,
    },
    /// `<pre>` content with whitespace preserved exactly.
    Code {
        text: String,
    },
    /// Hidden hint body. Drawn only when its id is in the open set; toggled
    /// by [`Inline::HintToggle`].
    Hint {
        id: u16,
        content: Vec<Block>,
    },
    /// `<blockquote>` content. The nesting depth is the number of enclosing
    /// `Quote` blocks.
    Quote(Vec<Block>),
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

#[derive(Clone, Debug, Default)]
struct CellDraft {
    header: bool,
    colspan: u8,
}

#[derive(Clone, Debug, Default)]
struct TableBuilder {
    rows: Vec<Vec<Cell>>,
    current_row: Vec<Cell>,
    in_cell: Option<CellDraft>,
    /// Depth of nested `<table>` tags whose content is flattened into text.
    nested: usize,
}

#[derive(Clone, Debug)]
struct HintLabel {
    id: u16,
    label: String,
}

#[derive(Clone, Debug)]
struct HintBody {
    id: u16,
    depth: usize,
    html: String,
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
    table_stack: Vec<TableBuilder>,
    /// Raw text collected inside `<pre>`; whitespace is preserved exactly.
    pre: Option<String>,
    /// Outer block buffers while inside `<blockquote>` elements.
    quote_stack: Vec<Vec<Block>>,
    /// Label text collected inside `<a class=hint>`.
    hint_label: Option<HintLabel>,
    /// Raw HTML collected inside a hint `<div>`.
    hint_body: Option<HintBody>,
    /// Id of the most recent hint toggle waiting for its body `<div>`.
    pending_hint: Option<u16>,
    hints_seen: u16,
    /// Inside a `<video>` element that already produced media.
    in_video: bool,
}

impl Parser {
    fn push_block(&mut self, block: Block) {
        self.blocks.push(block);
    }

    fn in_table_cell(&self) -> bool {
        self.table_stack
            .last()
            .is_some_and(|builder| builder.in_cell.is_some())
    }
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

        if self.pre.is_some() && !(closing && name == "pre") {
            return;
        }
        if self.hint_body.is_some() {
            let finished = closing
                && name == "div"
                && self.hint_body.as_ref().is_some_and(|body| body.depth == 0);
            if finished {
                let body = self.hint_body.take().expect("hint body present");
                self.finish_hint_body(body);
            } else {
                if let Some(body) = self.hint_body.as_mut() {
                    if name == "div" {
                        if closing {
                            body.depth = body.depth.saturating_sub(1);
                        } else {
                            body.depth += 1;
                        }
                    }
                    body.html.push_str(raw);
                }
            }
            return;
        }
        if self.hint_label.is_some() {
            if closing && name == "a" {
                if let Some(cap) = self.hint_label.take() {
                    self.finish_hint_label(cap);
                }
            }
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

        if name == "a" && !closing {
            let attributes = parse_attributes(inner, name_end);
            let is_hint = attributes.iter().any(|(key, value)| {
                key == "class"
                    && value
                        .split_whitespace()
                        .any(|class| class.eq_ignore_ascii_case("hint"))
            });
            if is_hint {
                let id = self.hints_seen;
                self.hints_seen = self.hints_seen.saturating_add(1);
                self.hint_label = Some(HintLabel {
                    id,
                    label: String::new(),
                });
                return;
            }
        }
        if name == "div" && !closing && self.hint_body.is_none() {
            // Hint bodies inside table cells degrade to normal content:
            // a block-level body cannot live inside a cell.
            if let Some(id) = self.pending_hint.filter(|_| !self.in_table_cell()) {
                let attributes = parse_attributes(inner, name_end);
                let is_hint_div = attribute(&attributes, "id")
                    .is_some_and(|value| value.starts_with("hint"))
                    || attributes.iter().any(|(key, value)| {
                        key == "class"
                            && value
                                .split_whitespace()
                                .any(|class| class.eq_ignore_ascii_case("hint"))
                    });
                if is_hint_div {
                    self.pending_hint = None;
                    self.hint_body = Some(HintBody {
                        id,
                        depth: 0,
                        html: String::new(),
                    });
                    return;
                }
            }
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
                    self.push_block(Block::Rule);
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
            "div" | "section" | "article" | "header" | "footer" | "main" | "nav" | "address"
            | "aside" | "figure" | "figcaption" | "h1" | "h2" | "h3" | "h4" | "h5" | "h6" => {
                self.flush();
                if !closing {
                    self.block_kind = BlockKind::Section;
                }
            }
            "table" | "tr" | "th" | "td" => {
                let attributes = parse_attributes(inner, name_end);
                self.handle_table_tag(&name, closing, &attributes);
            }
            "thead" | "tbody" | "tfoot" | "caption" | "colgroup" | "col" => {
                // Structural table tags carry no content of their own.
            }
            "pre" => {
                if closing {
                    if let Some(text) = self.pre.take() {
                        if self.in_table_cell() {
                            let mut style = self.style.clone();
                            style.mono = true;
                            push_text_with_furigana(&mut self.current, &text, style);
                        } else {
                            self.flush();
                            self.push_block(Block::Code { text });
                        }
                    }
                } else {
                    self.flush();
                    self.pre = Some(String::new());
                }
            }
            "blockquote" => {
                self.flush();
                if closing {
                    if !self.quote_stack.is_empty() {
                        let inner = std::mem::take(&mut self.blocks);
                        if let Some(mut outer) = self.quote_stack.pop() {
                            if !inner.is_empty() {
                                outer.push(Block::Quote(inner));
                            }
                            self.blocks = outer;
                        }
                    }
                } else {
                    let buffer = std::mem::take(&mut self.blocks);
                    self.quote_stack.push(buffer);
                }
            }
            "input" => {
                if !closing {
                    let attributes = parse_attributes(inner, name_end);
                    if attribute(&attributes, "id")
                        .is_some_and(|id| id.eq_ignore_ascii_case("typeans"))
                    {
                        self.current.push(Inline::TypeInput);
                    }
                }
            }
            "video" => {
                if closing {
                    self.in_video = false;
                } else {
                    let attributes = parse_attributes(inner, name_end);
                    let src = attribute(&attributes, "src").unwrap_or("").to_owned();
                    if !src.trim().is_empty() {
                        self.push_media_tag(&src, MediaKind::Video, &attributes);
                    }
                    self.in_video = true;
                }
            }
            "source" => {
                if !closing && !self.in_video {
                    let attributes = parse_attributes(inner, name_end);
                    let src = attribute(&attributes, "src").unwrap_or("").to_owned();
                    if !src.trim().is_empty() {
                        self.push_media_tag(&src, MediaKind::Video, &attributes);
                    }
                }
            }
            "img" => {
                if !closing {
                    let attributes = parse_attributes(inner, name_end);
                    let src = attribute(&attributes, "src").unwrap_or("").to_owned();
                    if media_kind_for_filename(&src) == MediaKind::Video {
                        self.push_media_tag(&src, MediaKind::Video, &attributes);
                    }
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
        if is_nasal_overlay(attributes) {
            self.apply_nasal_overlay(attributes);
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

    fn apply_nasal_overlay(&mut self, attributes: &[(String, String)]) {
        let mut effect = Style::default();
        if let Some(css) = attribute(attributes, "style") {
            apply_css(&mut effect, css);
        }
        if let Some(color) = attribute(attributes, "color").and_then(parse_color) {
            effect.color = Some(color);
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
        let mut last_text = None;
        for (offset, inline) in self.current[start..].iter_mut().enumerate() {
            if let Inline::Text { text, .. } = inline {
                if !text.is_empty() {
                    last_text = Some(start + offset);
                }
            }
        }
        if let Some(index) = last_text {
            if let Some(style) = inline_style_mut(&mut self.current[index]) {
                style.pitch.nasal = true;
                if effect.color.is_some() {
                    style.pitch.nasal_color = effect.color;
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
        if let Some(pre) = self.pre.as_mut() {
            pre.push_str(&decode_entities(text));
            return;
        }
        if let Some(body) = self.hint_body.as_mut() {
            body.html.push_str(text);
            return;
        }
        if let Some(label) = self.hint_label.as_mut() {
            label.label.push_str(&decode_entities(text));
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
            style.cloze_active = true;
        }
        for part in split_math(text) {
            match part {
                MathPart::Text(segment) => {
                    push_text_with_furigana(&mut self.current, &segment, style.clone())
                }
                MathPart::Math(src) => self.push_math(&src, style.clone()),
            }
        }
    }

    fn push_math(&mut self, src: &str, style: Style) {
        for (text, script) in render_math_runs(src) {
            if text.is_empty() {
                continue;
            }
            let mut run_style = style.clone();
            if script != Script::Normal {
                run_style.script = script;
                match script {
                    Script::Sub => run_style.sub = true,
                    Script::Sup => run_style.sup = true,
                    Script::Normal => {}
                }
            }
            push_text_plain(&mut self.current, &text, run_style);
        }
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
        if self.in_table_cell() {
            // Block boundaries inside a table cell become line breaks; the
            // cell is flushed into a `Cell` on `</td>`.
            if !self.current.is_empty() && !matches!(self.current.last(), Some(Inline::Break)) {
                self.current.push(Inline::Break);
            }
            return;
        }
        while matches!(self.current.first(), Some(Inline::Break)) {
            self.current.remove(0);
        }
        while matches!(self.current.last(), Some(Inline::Break)) {
            self.current.pop();
        }
        normalize_nasal_markers(&mut self.current);
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
            self.push_block(Block::ListItem { marker, content });
        } else if self.block_kind == BlockKind::Section {
            self.push_block(Block::Section(content));
        } else {
            self.push_block(Block::Paragraph(content));
        }
        self.block_kind = BlockKind::Paragraph;
    }

    fn finish(&mut self) {
        self.finish_ruby();
        if let Some(text) = self.pre.take() {
            if self.in_table_cell() {
                let mut style = self.style.clone();
                style.mono = true;
                push_text_with_furigana(&mut self.current, &text, style);
            } else {
                self.flush();
                self.push_block(Block::Code { text });
            }
        }
        if let Some(cap) = self.hint_label.take() {
            self.finish_hint_label(cap);
        }
        if let Some(body) = self.hint_body.take() {
            self.finish_hint_body(body);
        }
        while !self.table_stack.is_empty() {
            self.flush_cell();
            self.end_row();
            if let Some(builder) = self.table_stack.pop() {
                if !builder.rows.is_empty() {
                    self.flush();
                    self.push_block(Block::Table { rows: builder.rows });
                }
            }
        }
        self.flush();
        while let Some(mut outer) = self.quote_stack.pop() {
            let inner = std::mem::take(&mut self.blocks);
            if !inner.is_empty() {
                outer.push(Block::Quote(inner));
            }
            self.blocks = outer;
        }
    }

    fn finish_hint_label(&mut self, cap: HintLabel) {
        let label = cap.label.trim().to_owned();
        let label = if label.is_empty() {
            format!("Hint {}", cap.id + 1)
        } else {
            label
        };
        self.current.push(Inline::HintToggle { id: cap.id, label });
        self.pending_hint = Some(cap.id);
    }

    fn finish_hint_body(&mut self, body: HintBody) {
        let content = parse_html(&body.html);
        self.flush();
        self.push_block(Block::Hint {
            id: body.id,
            content,
        });
    }

    /// Close the open cell, moving `current` into the current row.
    fn flush_cell(&mut self) {
        if !self.in_table_cell() {
            return;
        }
        let mut content = std::mem::take(&mut self.current);
        while matches!(content.first(), Some(Inline::Break)) {
            content.remove(0);
        }
        while matches!(content.last(), Some(Inline::Break)) {
            content.pop();
        }
        content.retain(|inline| match inline {
            Inline::Text { text, .. } => !text.is_empty(),
            _ => true,
        });
        let draft = self
            .table_stack
            .last_mut()
            .and_then(|builder| builder.in_cell.take());
        if let Some(draft) = draft {
            if let Some(builder) = self.table_stack.last_mut() {
                builder.current_row.push(Cell {
                    header: draft.header,
                    colspan: draft.colspan.max(1),
                    content,
                });
            }
        }
        self.current_marker = None;
        self.block_kind = BlockKind::Paragraph;
    }

    fn end_row(&mut self) {
        if let Some(builder) = self.table_stack.last_mut() {
            if !builder.current_row.is_empty() {
                let row = std::mem::take(&mut builder.current_row);
                builder.rows.push(row);
            }
        }
    }

    fn close_table(&mut self) {
        self.flush_cell();
        self.end_row();
        if let Some(builder) = self.table_stack.pop() {
            if !builder.rows.is_empty() {
                self.flush();
                self.push_block(Block::Table { rows: builder.rows });
            }
        }
    }

    fn handle_table_tag(&mut self, name: &str, closing: bool, attributes: &[(String, String)]) {
        if let Some(builder) = self.table_stack.last_mut() {
            if builder.nested > 0 {
                if name == "table" {
                    if closing {
                        builder.nested -= 1;
                    } else {
                        builder.nested += 1;
                    }
                }
                return;
            }
            if name == "table" && !closing {
                builder.nested += 1;
                return;
            }
        } else if name != "table" || closing {
            return;
        }
        match name {
            "table" => {
                if closing {
                    self.close_table();
                } else {
                    self.flush();
                    self.table_stack.push(TableBuilder::default());
                }
            }
            "tr" => {
                if self.table_stack.is_empty() {
                    return;
                }
                self.flush_cell();
                self.end_row();
            }
            "th" | "td" => {
                if self.table_stack.is_empty() {
                    return;
                }
                if closing {
                    self.flush_cell();
                } else {
                    self.flush_cell();
                    let colspan = attributes
                        .iter()
                        .find(|(key, _)| key == "colspan")
                        .and_then(|(_, value)| value.trim().parse::<u8>().ok())
                        .unwrap_or(1)
                        .clamp(1, 12);
                    if let Some(builder) = self.table_stack.last_mut() {
                        builder.in_cell = Some(CellDraft {
                            header: name == "th",
                            colspan,
                        });
                    }
                }
            }
            _ => {}
        }
    }

    /// Push animated/video media for a `<video>`/`<source>`/`<img>` tag.
    fn push_media_tag(&mut self, src: &str, kind: MediaKind, attributes: &[(String, String)]) {
        let decoded = decode_entities(src);
        let name = decoded
            .rsplit(|ch| ch == '/' || ch == '\\')
            .next()
            .unwrap_or(decoded.as_str())
            .trim()
            .trim_matches('"')
            .trim_matches('\'');
        if name.is_empty() {
            return;
        }
        let animated = is_animated_image(name);
        let autoplay = has_attr(attributes, "autoplay") || animated;
        let loop_ = has_attr(attributes, "loop") || animated;
        self.current.push(Inline::Media {
            path: name.to_owned(),
            kind,
            autoplay,
            loop_,
        });
    }
}

fn has_attr(attributes: &[(String, String)], name: &str) -> bool {
    attributes.iter().any(|(key, _)| key == name)
}

fn is_animated_image(name: &str) -> bool {
    let ext = name.rsplit('.').next().unwrap_or("").to_ascii_lowercase();
    matches!(ext.as_str(), "gif" | "webp")
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
        "sub" => {
            style.sub = true;
            style.script = Script::Sub;
        }
        "sup" => {
            style.sup = true;
            style.script = Script::Sup;
        }
        "code" => style.mono = true,
        "mark" => style.highlight = Some([255, 235, 120]),
        _ => {}
    }
    if tag == "span" {
        let mut active = false;
        let mut inactive = false;
        for (_, value) in attributes.iter().filter(|(name, _)| name == "class") {
            for class in value.split_whitespace() {
                if class.eq_ignore_ascii_case("cloze-inactive") {
                    inactive = true;
                } else if class.eq_ignore_ascii_case("cloze") {
                    active = true;
                } else if class == "typeGood" {
                    style.diff = Some(DiffClass::Good);
                } else if class == "typeBad" {
                    style.diff = Some(DiffClass::Bad);
                } else if class == "typeMissed" {
                    style.diff = Some(DiffClass::Missed);
                }
            }
        }
        // Newer renderers also mark clozes with data attributes.
        if attributes
            .iter()
            .any(|(name, _)| name == "data-cloze" || name == "data-ordinal")
        {
            active = true;
        }
        if inactive {
            style.cloze_inactive = true;
            style.cloze = false;
            style.cloze_active = false;
        } else if active {
            style.cloze = true;
            style.cloze_active = true;
        }
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
    } else if matches!(
        value,
        "nasal"
            | "bidakuon"
            | "handakuten"
            | "pronunciation-nasal"
            | "nasal-diacritic"
            | "pronunciation-nasal-diacritic"
    ) {
        style.pitch.nasal = true;
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
        Inline::Marker(_)
        | Inline::Break
        | Inline::TypeInput
        | Inline::HintToggle { .. }
        | Inline::Media { .. } => None,
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

fn is_nasal_overlay(attributes: &[(String, String)]) -> bool {
    attribute(attributes, "class")
        .is_some_and(|value| {
            value.split_whitespace().any(|class| {
                matches!(
                    class,
                    "pronunciation-nasal-diacritic" | "nasal-diacritic" | "nasal-circle"
                )
            })
        })
}

/// Math and LaTeX handling.
///
/// Card text may embed MathJax groups (`\(…\)`, `\[…\]`, `$…$`, `$$…$$`).
/// They are swapped for readable Unicode approximations before layout:
/// Greek letters and common operators are substituted, `\frac{a}{b}`
/// becomes `(a)/(b)`, `^{x}`/`_{x}` use Unicode super/subscripts when every
/// character has one (otherwise the run keeps a [`Script`] style), and
/// `\ce{H2O}` (mhchem) subscripts digit runs after letters. `\mathrm`,
/// `\text`, `\left` and `\right` are stripped; unknown `\cmd` stays as-is.

/// A text segment split out of [`split_math`]: either plain text or the inner
/// content of a `\(…\)`, `\[…\]`, `$…$` or `$$…$$` group.
enum MathPart {
    Text(String),
    Math(String),
}

/// Split card text into plain and math segments. An opener without a closer
/// stays plain text.
fn split_math(text: &str) -> Vec<MathPart> {
    let mut parts = Vec::new();
    let mut current = String::new();
    let mut index = 0;
    while index < text.len() {
        let rest = &text[index..];
        if rest.starts_with("\\(") || rest.starts_with("\\[") {
            let close = if rest.starts_with("\\(") {
                "\\)"
            } else {
                "\\]"
            };
            if let Some(end) = rest[2..].find(close) {
                if !current.is_empty() {
                    parts.push(MathPart::Text(std::mem::take(&mut current)));
                }
                parts.push(MathPart::Math(rest[2..2 + end].to_owned()));
                index += 2 + end + 2;
                continue;
            }
        } else if rest.starts_with("$$") {
            if let Some(end) = rest[2..].find("$$") {
                if !current.is_empty() {
                    parts.push(MathPart::Text(std::mem::take(&mut current)));
                }
                parts.push(MathPart::Math(rest[2..2 + end].to_owned()));
                index += 2 + end + 2;
                continue;
            }
        } else if rest.starts_with("\\$") {
            current.push('$');
            index += 2;
            continue;
        } else if rest.starts_with('$') {
            if let Some(end) = rest[1..].find('$') {
                // A `$$` closer belongs to display math, not to this group.
                if end > 0 && !rest[1..1 + end].contains("$$") {
                    if !current.is_empty() {
                        parts.push(MathPart::Text(std::mem::take(&mut current)));
                    }
                    parts.push(MathPart::Math(rest[1..1 + end].to_owned()));
                    index += 1 + end + 1;
                    continue;
                }
            }
        }
        let ch = rest.chars().next().expect("non-empty rest");
        current.push(ch);
        index += ch.len_utf8();
    }
    if !current.is_empty() {
        parts.push(MathPart::Text(current));
    }
    parts
}

/// Render a math group to readable text, keeping runs that need super/sub
/// script styling separate. [`render_math`] is the plain-string projection of
/// this function.
fn render_math_runs(src: &str) -> Vec<(String, Script)> {
    let mut runs: Vec<(String, Script)> = vec![(String::new(), Script::Normal)];
    let mut chars = src.chars().peekable();
    while let Some(ch) = chars.next() {
        match ch {
            '\\' => {
                if let Some(&next) = chars.peek() {
                    if matches!(
                        next,
                        '$' | '{' | '}' | '#' | '&' | '_' | '%' | '|' | '<' | '>'
                    ) {
                        chars.next();
                        push_math_text(&mut runs, &next.to_string());
                        continue;
                    }
                    if next == '\\' {
                        chars.next();
                        push_math_text(&mut runs, " ");
                        continue;
                    }
                }
                let mut name = String::new();
                while let Some(&c) = chars.peek() {
                    if c.is_ascii_alphabetic() {
                        name.push(c);
                        chars.next();
                    } else {
                        break;
                    }
                }
                if name.is_empty() {
                    match chars.next() {
                        Some(',') | Some(';') | Some(':') | Some(' ') => {
                            push_math_text(&mut runs, " ")
                        }
                        Some('!') => {}
                        Some(other) => push_math_text(&mut runs, &other.to_string()),
                        None => push_math_text(&mut runs, "\\"),
                    }
                    continue;
                }
                match name.as_str() {
                    "frac" => {
                        let over = read_brace_group(&mut chars);
                        let under = read_brace_group(&mut chars);
                        push_math_text(&mut runs, &format!("({over})/({under})"));
                    }
                    "sqrt" => {
                        if chars.peek() == Some(&'{') {
                            let inner = read_brace_group(&mut chars);
                            push_math_text(&mut runs, &format!("√({inner})"));
                        } else if let Some(next) = chars.next() {
                            push_math_text(&mut runs, "√");
                            push_math_text(&mut runs, &next.to_string());
                        } else {
                            push_math_text(&mut runs, "√");
                        }
                    }
                    "ce" => {
                        let inner = read_brace_group(&mut chars);
                        push_math_text(&mut runs, &convert_mhchem(&inner));
                    }
                    "text" | "mathrm" | "mathbf" | "mathit" | "mathsf" | "mathtt" | "textrm" => {
                        let inner = read_brace_group(&mut chars);
                        push_math_text(&mut runs, &inner);
                    }
                    "left" | "right" => {}
                    "quad" | "qquad" => push_math_text(&mut runs, " "),
                    _ => {
                        if let Some(symbol) = latex_symbol(&name) {
                            push_math_text(&mut runs, &symbol.to_string());
                        } else {
                            push_math_text(&mut runs, "\\");
                            push_math_text(&mut runs, &name);
                        }
                    }
                }
            }
            '^' | '_' => {
                let script = if ch == '^' { Script::Sup } else { Script::Sub };
                let atom = if chars.peek() == Some(&'{') {
                    chars.next();
                    read_brace_inner(&mut chars)
                } else if let Some(next) = chars.next() {
                    next.to_string()
                } else {
                    continue;
                };
                if !atom.is_empty() && atom.chars().all(|c| shift_char(c, script).is_some()) {
                    let mapped: String =
                        atom.chars().filter_map(|c| shift_char(c, script)).collect();
                    push_math_text(&mut runs, &mapped);
                } else if !atom.is_empty() {
                    runs.push((atom, script));
                    runs.push((String::new(), Script::Normal));
                }
            }
            '{' => {
                let inner = read_brace_inner(&mut chars);
                for (text, script) in render_math_runs(&inner) {
                    if script == Script::Normal {
                        push_math_text(&mut runs, &text);
                    } else if !text.is_empty() {
                        runs.push((text, script));
                        runs.push((String::new(), Script::Normal));
                    }
                }
            }
            '}' => {}
            '-' => push_math_text(&mut runs, "−"),
            '*' => push_math_text(&mut runs, "×"),
            _ => push_math_text(&mut runs, &ch.to_string()),
        }
    }
    runs.retain(|(text, _)| !text.is_empty());
    runs
}

fn push_math_text(runs: &mut Vec<(String, Script)>, text: &str) {
    if text.is_empty() {
        return;
    }
    if let Some((current, Script::Normal)) = runs.last_mut() {
        current.push_str(text);
    } else {
        runs.push((text.to_owned(), Script::Normal));
    }
}

/// Read a `{…}` group, consuming the opening brace when present. Nested
/// braces are honoured.
fn read_brace_group(chars: &mut std::iter::Peekable<std::str::Chars>) -> String {
    if chars.peek() == Some(&'{') {
        chars.next();
        read_brace_inner(chars)
    } else if let Some(ch) = chars.next() {
        ch.to_string()
    } else {
        String::new()
    }
}

fn read_brace_inner(chars: &mut std::iter::Peekable<std::str::Chars>) -> String {
    let mut inner = String::new();
    let mut depth = 0usize;
    while let Some(ch) = chars.next() {
        match ch {
            '{' => {
                depth += 1;
                inner.push(ch);
            }
            '}' => {
                if depth == 0 {
                    break;
                }
                depth -= 1;
                inner.push(ch);
            }
            _ => inner.push(ch),
        }
    }
    inner
}

/// Map a LaTeX command to a readable Unicode character.
fn latex_symbol(name: &str) -> Option<char> {
    Some(match name {
        "alpha" => 'α',
        "beta" => 'β',
        "gamma" => 'γ',
        "delta" => 'δ',
        "epsilon" => 'ε',
        "varepsilon" => 'ε',
        "zeta" => 'ζ',
        "eta" => 'η',
        "theta" => 'θ',
        "vartheta" => 'θ',
        "iota" => 'ι',
        "kappa" => 'κ',
        "lambda" => 'λ',
        "mu" => 'μ',
        "nu" => 'ν',
        "xi" => 'ξ',
        "pi" => 'π',
        "varpi" => 'π',
        "rho" => 'ρ',
        "varrho" => 'ρ',
        "sigma" => 'σ',
        "varsigma" => 'ς',
        "tau" => 'τ',
        "upsilon" => 'υ',
        "phi" => 'φ',
        "varphi" => 'φ',
        "chi" => 'χ',
        "psi" => 'ψ',
        "omega" => 'ω',
        "Gamma" => 'Γ',
        "Delta" => 'Δ',
        "Theta" => 'Θ',
        "Lambda" => 'Λ',
        "Xi" => 'Ξ',
        "Pi" => 'Π',
        "Sigma" => 'Σ',
        "Upsilon" => 'Υ',
        "Phi" => 'Φ',
        "Psi" => 'Ψ',
        "Omega" => 'Ω',
        "times" => '×',
        "cdot" => '·',
        "pm" => '±',
        "mp" => '∓',
        "leq" | "le" => '≤',
        "geq" | "ge" => '≥',
        "neq" | "ne" => '≠',
        "approx" => '≈',
        "equiv" => '≡',
        "infty" | "inf" => '∞',
        "sum" => '∑',
        "prod" => '∏',
        "int" => '∫',
        "oint" => '∮',
        "partial" => '∂',
        "nabla" => '∇',
        "to" | "rightarrow" => '→',
        "leftarrow" => '←',
        "uparrow" => '↑',
        "downarrow" => '↓',
        "leftrightarrow" => '↔',
        "rightleftharpoons" => '⇋',
        "Rightarrow" => '⇒',
        "Leftarrow" => '⇐',
        "Leftrightarrow" => '⇔',
        "degree" => '°',
        "prime" => '′',
        "ldots" => '…',
        "cdots" => '⋯',
        "forall" => '∀',
        "exists" => '∃',
        "in" => '∈',
        "notin" => '∉',
        "ni" => '∋',
        "cap" => '∩',
        "cup" => '∪',
        "subset" => '⊂',
        "supset" => '⊃',
        "subseteq" => '⊆',
        "supseteq" => '⊇',
        "oplus" => '⊕',
        "otimes" => '⊗',
        "perp" => '⊥',
        "angle" => '∠',
        "wedge" | "land" => '∧',
        "vee" | "lor" => '∨',
        "neg" | "lnot" => '¬',
        "ast" => '∗',
        "bullet" => '∙',
        "div" => '÷',
        "propto" => '∝',
        "odot" => '⊙',
        _ => return None,
    })
}

/// Map a character to its Unicode super/subscript form, if one exists. Only
/// the characters named by the card spec (digits, `+−=()`, `n`, `i`) qualify;
/// anything else falls back to [`Script`] styling.
fn shift_char(ch: char, script: Script) -> Option<char> {
    Some(match script {
        Script::Normal => return None,
        Script::Sup => match ch {
            '0' => '⁰',
            '1' => '¹',
            '2' => '²',
            '3' => '³',
            '4' => '⁴',
            '5' => '⁵',
            '6' => '⁶',
            '7' => '⁷',
            '8' => '⁸',
            '9' => '⁹',
            '+' => '⁺',
            '-' | '−' => '⁻',
            '=' => '⁼',
            '(' => '⁽',
            ')' => '⁾',
            'n' => 'ⁿ',
            'i' => 'ⁱ',
            _ => return None,
        },
        Script::Sub => match ch {
            '0' => '₀',
            '1' => '₁',
            '2' => '₂',
            '3' => '₃',
            '4' => '₄',
            '5' => '₅',
            '6' => '₆',
            '7' => '₇',
            '8' => '₈',
            '9' => '₉',
            '+' => '₊',
            '-' | '−' => '₋',
            '=' => '₌',
            '(' => '₍',
            ')' => '₎',
            'n' => 'ₙ',
            'i' => 'ᵢ',
            _ => return None,
        },
    })
}

/// Convert mhchem `\ce{…}` content: digits following a letter (or an ongoing
/// digit run, for multi-digit counts) become subscripts.
fn convert_mhchem(src: &str) -> String {
    let mut output = String::new();
    let mut prev_letter = false;
    let mut sub_run = false;
    for ch in src.chars() {
        if ch.is_ascii_digit() && (prev_letter || sub_run) {
            output.push(shift_char(ch, Script::Sub).unwrap_or(ch));
            sub_run = true;
            prev_letter = false;
        } else {
            output.push(ch);
            prev_letter = ch.is_alphabetic();
            sub_run = false;
        }
    }
    output
}

/// Render a LaTeX math group to readable text. Super/subscripts whose
/// characters all have Unicode forms are substituted inline; other markup is
/// dropped or passed through by [`render_math_runs`].
pub fn render_math(src: &str) -> String {
    render_math_runs(src)
        .into_iter()
        .map(|(text, _)| text)
        .collect()
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

fn normalize_nasal_markers(current: &mut Vec<Inline>) {
    let mut output = Vec::with_capacity(current.len());
    for inline in current.drain(..) {
        match inline {
            Inline::Text { text, style } => append_text_with_nasal(&mut output, &text, style),
            other => output.push(other),
        }
    }
    *current = output;
}

fn append_text_with_nasal(output: &mut Vec<Inline>, text: &str, style: Style) {
    let mut segment_start = 0usize;
    for (index, ch) in text.char_indices() {
        if index < segment_start || !is_nasal_marker_char(ch) {
            continue;
        }
        let previous_in_segment = text[segment_start..index].chars().next_back();
        let previous_is_kana = match previous_in_segment {
            Some(previous) => is_kana_char(previous),
            None => last_text_ends_with_kana(output),
        };
        if !previous_is_kana {
            continue;
        }
        if ch == '\u{25CB}' && !circle_is_nasal(text, index, &style) {
            continue;
        }
        if previous_in_segment.is_some_and(is_kana_char) {
            let mut start = mora_start(text, index);
            if start < segment_start {
                start = segment_start;
            }
            if start < index {
                push_text_plain(output, &text[segment_start..start], style.clone());
                let mut mora_style = style.clone();
                apply_nasal_marker_style(&mut mora_style, &style);
                push_text_plain(output, &text[start..index], mora_style);
                segment_start = index + ch.len_utf8();
            }
        } else if attach_nasal_to_last(output, &style) {
            segment_start = index + ch.len_utf8();
        }
    }
    if segment_start < text.len() {
        push_text_plain(output, &text[segment_start..], style);
    }
}

fn attach_nasal_to_last(output: &mut Vec<Inline>, marker_style: &Style) -> bool {
    let Some(index) = output.iter().rposition(
        |inline| matches!(inline, Inline::Text { text, .. } if !text.is_empty()),
    ) else {
        return false;
    };
    if !output[index + 1..].iter().all(
        |inline| matches!(inline, Inline::Text { text, .. } if text.is_empty()),
    ) {
        return false;
    }
    let mut tail = output.split_off(index);
    let Inline::Text { text, style } = tail.remove(0) else {
        return false;
    };
    if !text.chars().next_back().is_some_and(is_kana_char) {
        output.push(Inline::Text { text, style });
        output.extend(tail);
        return false;
    }
    let start = mora_start(&text, text.len());
    if start > 0 {
        output.push(Inline::Text {
            text: text[..start].to_owned(),
            style: style.clone(),
        });
    }
    let mut mora_style = style;
    apply_nasal_marker_style(&mut mora_style, marker_style);
    output.push(Inline::Text {
        text: text[start..].to_owned(),
        style: mora_style,
    });
    output.extend(tail);
    true
}

fn apply_nasal_marker_style(mora_style: &mut Style, marker_style: &Style) {
    mora_style.pitch.nasal = true;
    if marker_style.color.is_some() {
        mora_style.pitch.nasal_color = marker_style.color;
    }
    if marker_style.pitch.downstep {
        mora_style.pitch.downstep = true;
    }
    if marker_style.pitch.upstep {
        mora_style.pitch.upstep = true;
    }
}

fn last_text_ends_with_kana(output: &[Inline]) -> bool {
    output
        .iter()
        .rev()
        .find_map(|inline| match inline {
            Inline::Text { text, .. } if !text.is_empty() => {
                Some(text.chars().next_back().is_some_and(is_kana_char))
            }
            Inline::Text { .. } => None,
            _ => Some(false),
        })
        .unwrap_or(false)
}

fn is_nasal_marker_char(ch: char) -> bool {
    matches!(
        ch,
        '\u{309A}' | '\u{309C}' | '\u{FF9F}' | '\u{00B0}' | '\u{02DA}' | '\u{25CB}'
    )
}

fn circle_is_nasal(text: &str, index: usize, style: &Style) -> bool {
    if style.pitch != PitchStyle::default() {
        return true;
    }
    let after = &text[index + '\u{25CB}'.len_utf8()..];
    after
        .chars()
        .next()
        .is_some_and(|next| is_kana_char(next) || next == '[')
}

fn is_kana_char(ch: char) -> bool {
    matches!(ch as u32, 0x3041..=0x3096 | 0x30a1..=0x30fa)
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

fn mora_start(text: &str, end: usize) -> usize {
    let mut start = match text[..end].char_indices().next_back() {
        Some((index, _)) => index,
        None => return end,
    };
    loop {
        let ch = text[start..].chars().next().unwrap_or(' ');
        if !is_small_kana(ch) {
            break;
        }
        match text[..start].char_indices().next_back() {
            Some((previous, previous_ch)) if is_kana_char(previous_ch) => start = previous,
            _ => break,
        }
    }
    start
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
            if is_kanji(ch)
                || matches!(ch, '\u{3005}' | '\u{3006}' | '\u{3007}' | '\u{30fc}')
                || is_nasal_marker_char(ch)
            {
                start = index;
            } else {
                break;
            }
        }
    } else if last.is_alphanumeric() {
        for (index, ch) in text[..last_index].char_indices().rev() {
            if ch.is_alphanumeric()
                || matches!(ch, '\'' | '’' | '-' | '_')
                || is_nasal_marker_char(ch)
            {
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

pub(crate) fn decode_entities(text: &str) -> String {
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
        "sum" => '∑',
        "prod" => '∏',
        "infin" => '∞',
        "le" => '≤',
        "ge" => '≥',
        "ne" => '≠',
        "approx" => '≈',
        "equiv" => '≡',
        "prime" => '′',
        "Prime" => '″',
        "nabla" => '∇',
        "part" => '∂',
        "forall" => '∀',
        "exist" => '∃',
        "empty" => '∅',
        "isin" => '∈',
        "notin" => '∉',
        "ni" => '∋',
        "cap" => '∩',
        "cup" => '∪',
        "sub" => '⊂',
        "sup" => '⊃',
        "sube" => '⊆',
        "supe" => '⊇',
        "oplus" => '⊕',
        "otimes" => '⊗',
        "perp" => '⊥',
        "lang" => '⟨',
        "rang" => '⟩',
        "lArr" => '⇐',
        "uArr" => '⇑',
        "rArr" => '⇒',
        "dArr" => '⇓',
        "hArr" => '⇔',
        "alpha" => 'α',
        "beta" => 'β',
        "gamma" => 'γ',
        "delta" => 'δ',
        "epsilon" => 'ε',
        "zeta" => 'ζ',
        "eta" => 'η',
        "theta" => 'θ',
        "iota" => 'ι',
        "kappa" => 'κ',
        "lambda" => 'λ',
        "mu" => 'μ',
        "nu" => 'ν',
        "xi" => 'ξ',
        "omicron" => 'ο',
        "pi" => 'π',
        "rho" => 'ρ',
        "sigmaf" => 'ς',
        "sigma" => 'σ',
        "tau" => 'τ',
        "upsilon" => 'υ',
        "phi" => 'φ',
        "chi" => 'χ',
        "psi" => 'ψ',
        "omega" => 'ω',
        "Alpha" => 'Α',
        "Beta" => 'Β',
        "Gamma" => 'Γ',
        "Delta" => 'Δ',
        "Epsilon" => 'Ε',
        "Zeta" => 'Ζ',
        "Eta" => 'Η',
        "Theta" => 'Θ',
        "Iota" => 'Ι',
        "Kappa" => 'Κ',
        "Lambda" => 'Λ',
        "Mu" => 'Μ',
        "Nu" => 'Ν',
        "Xi" => 'Ξ',
        "Omicron" => 'Ο',
        "Pi" => 'Π',
        "Rho" => 'Ρ',
        "Sigma" => 'Σ',
        "Tau" => 'Τ',
        "Upsilon" => 'Υ',
        "Phi" => 'Φ',
        "Chi" => 'Χ',
        "Psi" => 'Ψ',
        "Omega" => 'Ω',
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
        fn runs(
            html: &str,
        ) -> Vec<(String, bool, bool, bool, Option<[u8; 3]>, Option<[u8; 3]>)> {
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
                        style.pitch.nasal,
                        style.color,
                        style.pitch.nasal_color,
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
                ("ワ".into(), false, false, false, None, None),
                ("タシ".into(), true, false, false, None, None),
            ]
        );

        assert_eq!(
            runs(
                r#"ア<span style="display:inline-block;position:relative;padding-right:0.1em;margin-right:0.1em;"><span style="display:inline;">ナ</span><span style="border-color:currentColor;display:block;user-select:none;pointer-events:none;position:absolute;top:0.1em;left:0;right:0;height:0;border-top-width:0.1em;border-top-style:solid;right:-0.1em;height:0.4em;border-right-width:0.1em;border-right-style:solid;"></span></span>タ"#
            ),
            vec![
                ("ア".into(), false, false, false, None, None),
                ("ナ".into(), true, true, false, None, None),
                ("タ".into(), false, false, false, None, None),
            ]
        );

        assert_eq!(
            runs(
                r#"<span style="color: royalblue;">ス</span><span style="display:inline-block;position:relative;padding-right:0.1em;margin-right:0.1em;"><span style="display:inline;">キ</span><span style="border-color:currentColor;display:block;user-select:none;pointer-events:none;position:absolute;top:0.1em;left:0;right:0;height:0;border-top-width:0.1em;border-top-style:solid;right:-0.1em;height:0.4em;border-right-width:0.1em;border-right-style:solid;"></span></span>"#
            ),
            vec![
                (
                    "ス".into(),
                    false,
                    false,
                    false,
                    Some([65, 105, 225]),
                    None
                ),
                ("キ".into(), true, true, false, None, None),
            ]
        );

        assert_eq!(
            runs(
                r#"ニ<span style="display:inline-block;position:relative;"><span style="display:inline;">ホンコ<span style="color: red;">°</span></span><span style="border-color:currentColor;display:block;user-select:none;pointer-events:none;position:absolute;top:0.1em;left:0;right:0;height:0;border-top-width:0.1em;border-top-style:solid;"></span></span>"#
            ),
            vec![
                ("ニ".into(), false, false, false, None, None),
                ("ホン".into(), true, false, false, None, None),
                (
                    "コ".into(),
                    true,
                    false,
                    true,
                    None,
                    Some([220, 40, 30])
                ),
            ]
        );

        assert_eq!(
            runs(r#"ベ<span style="text-decoration:overline;">ンキョー</span>"#),
            vec![
                ("ベ".into(), false, false, false, None, None),
                ("ンキョー".into(), true, false, false, None, None),
            ]
        );
    }

    #[test]
    fn nasal_markers_attach_to_the_preceding_mora() {
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
                        style.pitch.nasal,
                        style.pitch.nasal_color,
                    )),
                    _ => None,
                })
                .collect()
        }

        assert_eq!(
            runs("ハケ°シイ"),
            vec![
                ("ハ".into(), false, false, None),
                ("ケ".into(), false, true, None),
                ("シイ".into(), false, false, None),
            ]
        );
        assert_eq!(
            runs("カ\u{309A}"),
            vec![("カ".into(), false, true, None)]
        );
        assert_eq!(runs("ケ゜"), vec![("ケ".into(), false, true, None)]);
        assert_eq!(
            runs(
                r#"<span style="display:inline-block;position:relative;"><span style="display:inline;">ケ○</span><span style="border-color:currentColor;display:block;user-select:none;pointer-events:none;position:absolute;top:0.1em;left:0;right:0;height:0;border-top-width:0.1em;border-top-style:solid;"></span></span>"#
            ),
            vec![("ケ".into(), true, true, None)]
        );
        assert_eq!(
            runs("<span class=\"nasal\">ガ</span>"),
            vec![("ガ".into(), false, true, None)]
        );
    }

    #[test]
    fn nasal_marker_in_pitch_word_stays_in_the_base_for_layout() {
        let blocks = parse_html("ハケ°シイ[2]");
        let Block::Paragraph(content) = &blocks[0] else {
            panic!("expected paragraph");
        };
        assert!(content.iter().any(|inline| matches!(
            inline,
            Inline::PitchWord { base, accent, .. } if base == "ハケ°シイ" && *accent == 2
        )));
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

    #[test]
    fn tables_are_parsed_with_headers_and_spans() {
        let blocks = parse_html(
            "<table><thead><tr><th>Name</th><th colspan=2>Value</th></tr></thead>\
             <tbody><tr><td>a</td><td>b</td><td>c</td></tr></tbody></table>",
        );
        assert_eq!(blocks.len(), 1);
        let Block::Table { rows } = &blocks[0] else {
            panic!("expected table, got {blocks:?}");
        };
        assert_eq!(rows.len(), 2);
        assert!(rows[0][0].header);
        assert_eq!(rows[0][1].colspan, 2);
        assert!(!rows[1][0].header);
        let first: String = rows[1][0]
            .content
            .iter()
            .map(|inline| match inline {
                Inline::Text { text, .. } => text.as_str(),
                _ => "",
            })
            .collect();
        assert_eq!(first, "a");
    }

    #[test]
    fn nested_tables_flatten_into_text() {
        let blocks = parse_html(
            "<table><tr><td>outer<table><tr><td>inner</td></tr></table>done</td></tr></table>",
        );
        let Block::Table { rows } = &blocks[0] else {
            panic!("expected table");
        };
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].len(), 1);
        let text: String = rows[0][0]
            .content
            .iter()
            .map(|inline| match inline {
                Inline::Text { text, .. } => text.as_str(),
                _ => "",
            })
            .collect();
        assert!(text.contains("outer"));
        assert!(text.contains("inner"));
        assert!(text.contains("done"));
    }

    #[test]
    fn hints_become_toggles_with_hidden_bodies() {
        let blocks = parse_html(
            "Q <a class=hint href=\"#\" onclick=\"return false;\">Field</a>\
             <div id=\"hintabc123\" class=hint style=\"display: none\">Answer <b>bold</b></div> after",
        );
        let toggle: Option<(u16, String)> = blocks.iter().find_map(|block| match block {
            Block::Paragraph(content) | Block::Section(content) => {
                content.iter().find_map(|inline| match inline {
                    Inline::HintToggle { id, label } => Some((*id, label.clone())),
                    _ => None,
                })
            }
            _ => None,
        });
        assert_eq!(
            toggle.as_ref().map(|(id, label)| (*id, label.as_str())),
            Some((0, "Field"))
        );
        let body = blocks.iter().find_map(|block| match block {
            Block::Hint { id, content } => Some((*id, content)),
            _ => None,
        });
        let (id, content) = body.expect("hint body");
        assert_eq!(id, 0);
        let text: String = content
            .iter()
            .flat_map(|block| match block {
                Block::Paragraph(inlines) | Block::Section(inlines) => inlines.as_slice(),
                _ => &[],
            })
            .map(|inline| match inline {
                Inline::Text { text, .. } => text.as_str(),
                _ => "",
            })
            .collect();
        assert!(text.contains("Answer"));
        assert!(text.contains("bold"));
    }

    #[test]
    fn type_answer_diff_spans_set_diff_styles() {
        let blocks = parse_html(
            "<code id=typeans><span class=typeGood>abc</span>\
             <span class=typeBad>x</span><span class=typeMissed>y</span></code>",
        );
        let inlines: Vec<&Inline> = blocks
            .iter()
            .flat_map(|block| match block {
                Block::Paragraph(content) | Block::Section(content) => content.iter(),
                _ => [].iter(),
            })
            .collect();
        let diff_of = |text: &str| -> DiffClass {
            inlines
                .iter()
                .find_map(|inline| match inline {
                    Inline::Text { text: value, style } if value == text => style.diff,
                    _ => None,
                })
                .unwrap_or_else(|| panic!("missing {text}"))
        };
        assert_eq!(diff_of("abc"), DiffClass::Good);
        assert_eq!(diff_of("x"), DiffClass::Bad);
        assert_eq!(diff_of("y"), DiffClass::Missed);
        assert!(inlines
            .iter()
            .all(|inline| !matches!(inline, Inline::Text { style, .. } if !style.mono)));
    }

    #[test]
    fn cloze_active_and_inactive_are_distinguished() {
        let html =
            "<span class=\"cloze\">active</span> <span class=\"cloze-inactive\">muted</span>";
        let blocks = parse_html(html);
        let content: Vec<&Inline> = blocks
            .iter()
            .flat_map(|block| match block {
                Block::Paragraph(inlines) | Block::Section(inlines) => inlines.iter(),
                _ => [].iter(),
            })
            .collect();
        let style_of = |text: &str| -> Style {
            content
                .iter()
                .find_map(|inline| match inline {
                    Inline::Text { text: value, style } if value == text => Some(style.clone()),
                    _ => None,
                })
                .unwrap_or_else(|| panic!("missing {text}"))
        };
        let active = style_of("active");
        assert!(active.cloze && active.cloze_active && !active.cloze_inactive);
        let muted = style_of("muted");
        assert!(muted.cloze_inactive && !muted.cloze && !muted.cloze_active);

        // Newer data-attribute markup counts as active.
        let blocks = parse_html("<span data-cloze=\"x\" data-ordinal=\"1\">new</span>");
        let (Block::Paragraph(content) | Block::Section(content)) = &blocks[0] else {
            panic!("expected paragraph");
        };
        assert!(matches!(
            &content[0],
            Inline::Text { style, .. } if style.cloze_active
        ));

        // Marked answer HTML keeps active markers through nesting.
        let marked =
            mark_clozes("<span class=\"cloze-inactive\">[<span class=\"cloze\">a</span>]</span>")
                .unwrap();
        assert_eq!(marked.matches(CLOZE_START).count(), 1);
        let blocks = parse_html(&marked);
        let found = blocks.iter().flat_map(|block| match block {
            Block::Paragraph(inlines) | Block::Section(inlines) => inlines.iter(),
            _ => [].iter(),
        });
        assert!(found.clone().any(|inline| matches!(
            inline,
            Inline::Text { text, style, .. } if text == "a" && style.cloze_active
        )));
    }

    #[test]
    fn sub_sup_code_and_pre_are_captured() {
        let blocks = parse_html("H<sub>2</sub> x<sup>n</sup> <code>mono</code>");
        let (Block::Paragraph(content) | Block::Section(content)) = &blocks[0] else {
            panic!("expected paragraph");
        };
        assert!(content.iter().any(
            |inline| matches!(inline, Inline::Text { text, style, .. } if text == "2" && style.script == Script::Sub)
        ));
        assert!(content.iter().any(
            |inline| matches!(inline, Inline::Text { text, style, .. } if text == "n" && style.script == Script::Sup)
        ));
        assert!(content.iter().any(
            |inline| matches!(inline, Inline::Text { text, style, .. } if text == "mono" && style.mono)
        ));

        let blocks = parse_html("<pre>  keep\n\tspace  </pre>");
        let Block::Code { text } = &blocks[0] else {
            panic!("expected code, got {blocks:?}");
        };
        assert_eq!(text, "  keep\n\tspace  ");
    }

    #[test]
    fn blockquotes_nest_and_type_input_parses() {
        let blocks = parse_html("<blockquote>one<blockquote>two</blockquote></blockquote>");
        let Block::Quote(outer) = &blocks[0] else {
            panic!("expected quote, got {blocks:?}");
        };
        assert!(outer.iter().any(|block| matches!(block, Block::Quote(_))));

        let blocks = parse_html("a<input type=text id=typeans> b");
        assert!(blocks
            .iter()
            .flat_map(|block| match block {
                Block::Paragraph(inlines) | Block::Section(inlines) => inlines.iter(),
                _ => [].iter(),
            })
            .any(|inline| matches!(inline, Inline::TypeInput)));
    }

    #[test]
    fn video_source_and_animated_images_become_media() {
        let blocks = parse_html("<video src=\"clip.mp4\" autoplay loop></video>");
        let media = blocks
            .iter()
            .find_map(|block| match block {
                Block::Paragraph(inlines) | Block::Section(inlines) => {
                    inlines.iter().find_map(|inline| match inline {
                        Inline::Media {
                            path,
                            kind,
                            autoplay,
                            loop_,
                        } => Some((path.clone(), *kind, *autoplay, *loop_)),
                        _ => None,
                    })
                }
                _ => None,
            })
            .expect("media");
        assert_eq!(media.0, "clip.mp4");
        assert_eq!(media.1, MediaKind::Video);
        assert!(media.2 && media.3);

        let blocks = parse_html("<img src=\"anim.gif\">");
        assert!(blocks
            .iter()
            .flat_map(|block| match block {
                Block::Paragraph(inlines) | Block::Section(inlines) => inlines.iter(),
                _ => [].iter(),
            })
            .any(|inline| matches!(
                inline,
                Inline::Media {
                    kind: MediaKind::Video,
                    autoplay: true,
                    loop_: true,
                    ..
                }
            )));

        // Still images stay out of the media path.
        let blocks = parse_html("<img src=\"photo.jpg\">");
        assert!(!blocks
            .iter()
            .flat_map(|block| match block {
                Block::Paragraph(inlines) | Block::Section(inlines) => inlines.iter(),
                _ => [].iter(),
            })
            .any(|inline| matches!(inline, Inline::Media { .. })));
    }

    #[test]
    fn math_groups_become_readable_text() {
        assert_eq!(render_math("\\alpha + \\times"), "α + ×");
        assert_eq!(render_math("\\frac{a}{b}"), "(a)/(b)");
        assert_eq!(render_math("x^2"), "x²");
        assert_eq!(render_math("H_2O"), "H₂O");
        assert_eq!(render_math("\\ce{H2O}"), "H₂O");
        assert_eq!(render_math("\\mathrm{AB}"), "AB");
        assert_eq!(render_math("\\unknowncmd"), "\\unknowncmd");
        assert_eq!(render_math("\\sum_{i}"), "∑ᵢ");

        let blocks = parse_html("Solve \\(x^2 + \\alpha\\) now");
        let (Block::Paragraph(content) | Block::Section(content)) = &blocks[0] else {
            panic!("expected paragraph");
        };
        let text: String = content
            .iter()
            .map(|inline| match inline {
                Inline::Text { text, .. } => text.as_str(),
                _ => "",
            })
            .collect();
        assert!(text.contains("x²"), "{text}");
        assert!(text.contains('α'), "{text}");

        // Runs without a Unicode form keep a Script style.
        let runs = render_math_runs("x^{ab}");
        assert!(runs
            .iter()
            .any(|(text, script)| text == "ab" && *script == Script::Sup));

        // Math entities decode as well.
        let blocks = parse_html("&sum; &alpha;");
        let (Block::Paragraph(content) | Block::Section(content)) = &blocks[0] else {
            panic!("expected paragraph");
        };
        let text: String = content
            .iter()
            .map(|inline| match inline {
                Inline::Text { text, .. } => text.as_str(),
                _ => "",
            })
            .collect();
        assert!(text.contains('∑') && text.contains('α'), "{text}");
    }
}
