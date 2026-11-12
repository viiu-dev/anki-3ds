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
    pub color: Option<[u8; 3]>,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct PillStyle {
    pub bg: [u8; 3],
    pub bg_alpha: u8,
    pub border: Option<[u8; 3]>,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct BoxStyle {
    pub bg: Option<[u8; 3]>,
    pub bg_alpha: u8,
    pub border_left: Option<[u8; 3]>,
    pub border_all: Option<[u8; 3]>,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Script {
    #[default]
    Normal,
    Sub,
    Sup,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DiffClass {
    Good,
    Bad,
    Missed,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Style {
    pub bold: bool,
    pub italic: bool,
    pub underline: bool,
    pub sub: bool,
    pub sup: bool,
    pub cloze: bool,
    pub cloze_active: bool,
    pub cloze_inactive: bool,
    pub mono: bool,
    pub script: Script,
    pub diff: Option<DiffClass>,
    pub color: Option<[u8; 3]>,
    pub highlight: Option<[u8; 3]>,
    pub scale: Option<u8>,
    pub muted: bool,
    pub pill: Option<PillStyle>,
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
        self.muted |= other.muted;
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
        if other.scale.is_some() {
            self.scale = other.scale;
        }
        if other.pill.is_some() {
            self.pill = other.pill;
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
        if other.pitch.color.is_some() {
            self.pitch.color = other.pitch.color;
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
    TypeInput,
    HintToggle {
        id: u16,
        label: String,
    },
    Media {
        path: String,
        kind: MediaKind,
        autoplay: bool,
        loop_: bool,
    },
    Image {
        src: String,
        max_w: Option<f32>,
        max_h: Option<f32>,
    },
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum MediaKind {
    #[default]
    Image,
    Audio,
    Video,
}

pub fn is_video_filename(filename: &str) -> bool {
    let ext = filename
        .rsplit('.')
        .next()
        .unwrap_or("")
        .to_ascii_lowercase();
    matches!(ext.as_str(), "mp4" | "webm" | "avi" | "mov" | "mkv" | "ogv")
}

pub fn media_kind_for_filename(filename: &str) -> MediaKind {
    if is_video_filename(filename) {
        return MediaKind::Video;
    }
    let ext = filename
        .rsplit('.')
        .next()
        .unwrap_or("")
        .to_ascii_lowercase();
    match ext.as_str() {
        "gif" | "webp" => MediaKind::Video,
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
    Code {
        text: String,
    },
    Hint {
        id: u16,
        content: Vec<Block>,
    },
    Quote(Vec<Block>),
    Callout {
        style: BoxStyle,
        content: Vec<Block>,
    },
}

use std::collections::HashMap;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum PseudoElement {
    None,
    Before,
    After,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Combinator {
    Descendant,
    Child,
    AdjacentSibling,
    GeneralSibling,
}

#[derive(Clone, Debug)]
enum AttrOp {
    Exists,
    Equals(String),
    ContainsWord(String),
    Prefix(String),
    Suffix(String),
    Substring(String),
}

#[derive(Clone, Debug)]
struct AttrSelector {
    name: String,
    op: AttrOp,
}

#[derive(Clone, Debug)]
enum PseudoClass {
    FirstChild,
    LastChild,
    OnlyChild,
    NthChild(i32, i32),
    Not(Box<CompoundSelector>),
}

#[derive(Clone, Debug, Default)]
struct CompoundSelector {
    tag: Option<String>,
    id: Option<String>,
    classes: Vec<String>,
    attrs: Vec<AttrSelector>,
    pseudos: Vec<PseudoClass>,
}

#[derive(Clone, Debug)]
struct SelectorStep {
    comb: Option<Combinator>,
    compound: CompoundSelector,
}

#[derive(Clone, Debug)]
struct Selector {
    /// Stored right-to-left: `steps[0]` matches the target element,
    /// `steps[i].comb` describes the link from `steps[i-1]` back to
    /// `steps[i]`.
    steps: Vec<SelectorStep>,
    pseudo: PseudoElement,
    specificity: (u16, u16, u16),
}

#[derive(Clone, Debug)]
struct CssDecl {
    prop: String,
    value: String,
}

#[derive(Clone, Debug)]
struct CssRule {
    selector: Selector,
    decls: Vec<CssDecl>,
    order: u32,
}

#[derive(Clone, Debug, Default)]
struct Stylesheet {
    rules: Vec<CssRule>,
    vars: HashMap<String, String>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum CssDisplay {
    Block,
    Inline,
    InlineBlock,
    Flex,
    InlineFlex,
    None,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
enum BorderSide {
    #[default]
    Unset,
    None,
    Set(Option<[u8; 3]>),
}

#[derive(Clone, Debug, Default)]
struct ComputedCss {
    display: Option<CssDisplay>,
    flex_column: bool,
    visibility_hidden: Option<bool>,
    opacity_zero: bool,
    opacity: Option<f32>,
    color: Option<[u8; 3]>,
    color_alpha: u8,
    bg: Option<([u8; 3], u8)>,
    bold: Option<bool>,
    italic: Option<bool>,
    underline: Option<bool>,
    overline: Option<bool>,
    font_size_pct: Option<(u16, bool)>,
    font_size_zero: bool,
    pos_abs: Option<bool>,
    border_all: BorderSide,
    border_top: BorderSide,
    border_right: BorderSide,
    border_bottom: BorderSide,
    border_left: BorderSide,
    has_radius: bool,
    has_padding: bool,
    list_style_none: Option<bool>,
    width_px: Option<f32>,
    height_px: Option<f32>,
    max_width_px: Option<f32>,
    max_height_px: Option<f32>,
    overflow_hidden: bool,
    content_literal: Option<String>,
}

#[derive(Clone, Debug)]
struct ElementInfo {
    tag: String,
    id: Option<String>,
    classes: Vec<String>,
    attrs: Vec<(String, String)>,
    nth_child: usize,
    prev_siblings: Vec<SiblingInfo>,
}

#[derive(Clone, Debug)]
struct SiblingInfo {
    tag: String,
    id: Option<String>,
    classes: Vec<String>,
    attrs: Vec<(String, String)>,
}

impl Stylesheet {
    fn from_css_and_html(css: &str, html: &str, dark_mode: bool) -> Self {
        let mut combined = String::from(css);
        collect_inline_style_tags(html, &mut combined);
        Self::parse(&combined, dark_mode)
    }

    fn parse(css: &str, dark_mode: bool) -> Self {
        let stripped = strip_css_comments(css);
        let mut sheet = Stylesheet::default();
        let mut order = 0u32;
        parse_css_block(&stripped, dark_mode, &mut sheet.rules, &mut order);
        sheet.extract_root_vars(dark_mode);
        sheet
    }

    fn extract_root_vars(&mut self, dark_mode: bool) {
        let root_ctx = ElementInfo {
            tag: "html".into(),
            id: None,
            classes: if dark_mode {
                vec!["card".into(), "nightMode".into(), "night_mode".into()]
            } else {
                vec!["card".into()]
            },
            attrs: vec![(
                "data-theme".into(),
                if dark_mode { "dark" } else { "light" }.into(),
            )],
            nth_child: 1,
            prev_siblings: Vec::new(),
        };
        let mut matches: Vec<&CssRule> = self
            .rules
            .iter()
            .filter(|rule| {
                rule.selector.pseudo == PseudoElement::None
                    && rule.decls.iter().any(|d| d.prop.starts_with("--"))
                    && selector_matches_root_only(&rule.selector, &root_ctx, dark_mode)
            })
            .collect();
        matches.sort_by_key(|rule| (rule.selector.specificity, rule.order));
        for rule in matches {
            for decl in &rule.decls {
                if decl.prop.starts_with("--") {
                    self.vars.insert(decl.prop.clone(), decl.value.clone());
                }
            }
        }
    }

    fn compute(
        &self,
        ancestors: &[ElementFrame],
        elem: &ElementInfo,
        inline_style: Option<&str>,
        var_scope: &mut Vec<(String, String)>,
        parent_color: Option<[u8; 3]>,
        dark_mode: bool,
    ) -> (ComputedCss, Option<ComputedCss>, Option<ComputedCss>) {
        let mut matched: Vec<(&CssRule, PseudoElement)> = Vec::new();
        for rule in &self.rules {
            if selector_matches(&rule.selector, ancestors, elem, dark_mode) {
                matched.push((rule, rule.selector.pseudo));
            }
        }
        matched.sort_by_key(|(rule, _)| (rule.selector.specificity, rule.order));

        for (rule, pseudo) in &matched {
            if *pseudo == PseudoElement::None {
                for decl in &rule.decls {
                    if decl.prop.starts_with("--") {
                        var_scope.push((decl.prop.clone(), decl.value.clone()));
                    }
                }
            }
        }
        let inline_decls = inline_style.map(parse_declarations).unwrap_or_default();
        for decl in &inline_decls {
            if decl.prop.starts_with("--") {
                var_scope.push((decl.prop.clone(), decl.value.clone()));
            }
        }

        let mut base = ComputedCss::default();
        let mut before: Option<ComputedCss> = None;
        let mut after: Option<ComputedCss> = None;
        for (rule, pseudo) in matched {
            let target = match pseudo {
                PseudoElement::None => &mut base,
                PseudoElement::Before => before.get_or_insert_with(ComputedCss::default),
                PseudoElement::After => after.get_or_insert_with(ComputedCss::default),
            };
            for decl in &rule.decls {
                if !decl.prop.starts_with("--") {
                    apply_css_decl(
                        target,
                        &decl.prop,
                        &decl.value,
                        &self.vars,
                        var_scope,
                        parent_color,
                    );
                }
            }
        }
        for decl in &inline_decls {
            if !decl.prop.starts_with("--") {
                apply_css_decl(
                    &mut base,
                    &decl.prop,
                    &decl.value,
                    &self.vars,
                    var_scope,
                    parent_color,
                );
            }
        }
        (base, before, after)
    }
}

fn collect_inline_style_tags(html: &str, out: &mut String) {
    let lower = html.to_ascii_lowercase();
    let mut pos = 0usize;
    while let Some(rel) = lower[pos..].find('<') {
        let start = pos + rel;
        let rest = &lower[start..];
        if rest.starts_with("<!--") {
            pos = rest
                .find("-->")
                .map(|i| start + i + 3)
                .unwrap_or(html.len());
            continue;
        }
        if rest.starts_with("<script")
            && rest[7..]
                .chars()
                .next()
                .is_some_and(|c| c.is_whitespace() || c == '>')
        {
            let Some(end) = tag_end(&html[start..]) else {
                break;
            };
            if !html[start..start + end].trim_end().ends_with("/>") {
                if let Some(close) = find_closing_rawtext(&lower[start + end..], "script") {
                    pos = start + end + close;
                    continue;
                }
            }
            pos = start + end;
            continue;
        }
        if rest.starts_with("<style")
            && rest[6..]
                .chars()
                .next()
                .is_some_and(|c| c.is_whitespace() || c == '>')
        {
            let Some(end) = tag_end(&html[start..]) else {
                break;
            };
            let raw_tag = &html[start..start + end];
            if !raw_tag.trim_end().ends_with("/>") {
                if let Some(close_rel) =
                    find_closing_rawtext_content(&lower[start + end..], "style")
                {
                    let inner = &html[start + end..start + end + close_rel.0];
                    if !raw_tag.to_ascii_lowercase().contains("disabled") {
                        out.push('\n');
                        out.push_str(inner);
                    }
                    pos = start + end + close_rel.1;
                    continue;
                }
            }
            pos = start + end;
            continue;
        }
        pos = start + 1;
    }
}

fn find_closing_rawtext(lower_rest: &str, tag: &str) -> Option<usize> {
    find_closing_rawtext_content(lower_rest, tag).map(|(_, end)| end)
}

fn find_closing_rawtext_content(lower_rest: &str, tag: &str) -> Option<(usize, usize)> {
    let needle = format!("</{tag}");
    let mut offset = 0usize;
    while let Some(rel) = lower_rest[offset..].find(&needle) {
        let start = offset + rel;
        let after = &lower_rest[start + needle.len()..];
        let next = after.chars().next();
        if next.is_none_or(|ch| ch == '>' || ch.is_whitespace() || ch == '/') {
            let close_gt = after.find('>').map(|i| start + needle.len() + i + 1);
            return Some((start, close_gt.unwrap_or(lower_rest.len())));
        }
        offset = start + needle.len();
    }
    None
}

fn strip_css_comments(css: &str) -> String {
    let mut out = String::with_capacity(css.len());
    let bytes = css.as_bytes();
    let mut i = 0usize;
    let mut quote: Option<u8> = None;
    while i < bytes.len() {
        if let Some(q) = quote {
            out.push(bytes[i] as char);
            if bytes[i] == b'\\' && i + 1 < bytes.len() {
                i += 1;
                out.push(bytes[i] as char);
            } else if bytes[i] == q {
                quote = None;
            }
            i += 1;
            continue;
        }
        if bytes[i] == b'/' && i + 1 < bytes.len() && bytes[i + 1] == b'*' {
            i += 2;
            while i + 1 < bytes.len() && !(bytes[i] == b'*' && bytes[i + 1] == b'/') {
                i += 1;
            }
            i = (i + 2).min(bytes.len());
            out.push(' ');
            continue;
        }
        if bytes[i] == b'"' || bytes[i] == b'\'' {
            quote = Some(bytes[i]);
        }
        let ch = css[i..].chars().next().expect("valid utf8");
        out.push(ch);
        i += ch.len_utf8();
    }
    out
}

fn find_matching_brace(text: &str, open_brace: usize) -> usize {
    let bytes = text.as_bytes();
    let mut depth = 0usize;
    let mut i = open_brace;
    let mut quote: Option<u8> = None;
    while i < bytes.len() {
        if let Some(q) = quote {
            if bytes[i] == b'\\' && i + 1 < bytes.len() {
                i += 2;
                continue;
            }
            if bytes[i] == q {
                quote = None;
            }
            i += 1;
            continue;
        }
        match bytes[i] {
            b'"' | b'\'' => quote = Some(bytes[i]),
            b'{' => depth += 1,
            b'}' => {
                depth = depth.saturating_sub(1);
                if depth == 0 {
                    return i;
                }
            }
            _ => {}
        }
        i += 1;
    }
    bytes.len()
}

fn find_outside_parens(text: &str, target: &[u8]) -> Option<usize> {
    let bytes = text.as_bytes();
    let mut paren = 0usize;
    let mut bracket = 0usize;
    let mut quote: Option<u8> = None;
    let mut i = 0usize;
    while i < bytes.len() {
        if let Some(q) = quote {
            if bytes[i] == b'\\' && i + 1 < bytes.len() {
                i += 2;
                continue;
            }
            if bytes[i] == q {
                quote = None;
            }
            i += 1;
            continue;
        }
        match bytes[i] {
            b'"' | b'\'' => quote = Some(bytes[i]),
            b'(' => paren += 1,
            b')' => paren = paren.saturating_sub(1),
            b'[' => bracket += 1,
            b']' => bracket = bracket.saturating_sub(1),
            b if paren == 0 && bracket == 0 && target.contains(&b) => return Some(i),
            _ => {}
        }
        i += 1;
    }
    None
}

fn parse_css_block(css: &str, dark_mode: bool, rules: &mut Vec<CssRule>, order: &mut u32) {
    let mut pos = 0usize;
    while pos < css.len() {
        let rest = css[pos..].trim_start();
        let trimmed_skip = css[pos..].len() - rest.len();
        pos += trimmed_skip;
        if pos >= css.len() {
            break;
        }
        if rest.starts_with('@') {
            let Some(delim) = find_outside_parens(rest, &[b'{', b';']) else {
                break;
            };
            if rest.as_bytes()[delim] == b';' {
                pos += delim + 1;
                continue;
            }
            let header = rest[..delim].trim();
            let close = find_matching_brace(rest, delim);
            let inner = if close > delim + 1 {
                &rest[delim + 1..close]
            } else {
                ""
            };
            let lower_header = header.to_ascii_lowercase();
            if let Some(cond) = lower_header.strip_prefix("@media") {
                if eval_media_query(cond, dark_mode) {
                    parse_css_block(inner, dark_mode, rules, order);
                }
            } else if lower_header.starts_with("@supports") || lower_header.starts_with("@layer") {
                parse_css_block(inner, dark_mode, rules, order);
            }
            pos += (close + 1).min(rest.len());
            continue;
        }
        let Some(open) = find_outside_parens(rest, &[b'{', b';']) else {
            break;
        };
        if rest.as_bytes()[open] == b';' {
            pos += open + 1;
            continue;
        }
        let selector_text = rest[..open].trim();
        let close = find_matching_brace(rest, open);
        let body = if close > open + 1 {
            &rest[open + 1..close]
        } else {
            ""
        };
        if !selector_text.is_empty() && !body.trim().is_empty() {
            let decls = parse_declarations(body);
            if !decls.is_empty() {
                for sel_str in split_top_level(selector_text, b',') {
                    if let Some(selector) = parse_selector(sel_str) {
                        *order += 1;
                        rules.push(CssRule {
                            selector,
                            decls: decls.clone(),
                            order: *order,
                        });
                    }
                }
            }
        }
        pos += (close + 1).min(rest.len());
    }
}

fn eval_media_query(query: &str, dark_mode: bool) -> bool {
    let query = query.trim();
    if query.is_empty() || query == "all" || query == "screen" {
        return true;
    }
    for branch in query.split(',') {
        let branch = branch.trim();
        if branch.is_empty() {
            continue;
        }
        if eval_media_branch(branch, dark_mode) {
            return true;
        }
    }
    false
}

fn eval_media_branch(branch: &str, dark_mode: bool) -> bool {
    if branch.contains("print") && !branch.starts_with("not ") {
        return false;
    }
    let mut ok = true;
    let mut rest = branch;
    while let Some(open) = rest.find('(') {
        let Some(close) = rest[open + 1..].find(')') else {
            break;
        };
        let feat = rest[open + 1..open + 1 + close].trim();
        if let Some((key, val)) = feat.split_once(':') {
            let key = key.trim();
            let val = val.trim();
            match key {
                "prefers-color-scheme" => {
                    if val == "dark" && !dark_mode {
                        ok = false;
                    } else if val == "light" && dark_mode {
                        ok = false;
                    }
                }
                "min-width" => {
                    if let Some(px) = parse_px_len(val) {
                        if 400.0 < px {
                            ok = false;
                        }
                    }
                }
                "max-width" => {
                    if let Some(px) = parse_px_len(val) {
                        if 400.0 > px {
                            ok = false;
                        }
                    }
                }
                "hover" => {
                    if val == "hover" {
                        ok = false;
                    }
                }
                "pointer" => {
                    if val == "fine" {
                        ok = false;
                    }
                }
                _ => {}
            }
        }
        rest = &rest[open + 1 + close + 1..];
    }
    ok
}

fn split_top_level(text: &str, sep: u8) -> Vec<&str> {
    let mut parts = Vec::new();
    let bytes = text.as_bytes();
    let mut start = 0usize;
    let mut paren = 0usize;
    let mut bracket = 0usize;
    let mut quote: Option<u8> = None;
    let mut i = 0usize;
    while i < bytes.len() {
        if let Some(q) = quote {
            if bytes[i] == b'\\' && i + 1 < bytes.len() {
                i += 2;
                continue;
            }
            if bytes[i] == q {
                quote = None;
            }
            i += 1;
            continue;
        }
        match bytes[i] {
            b'"' | b'\'' => quote = Some(bytes[i]),
            b'(' => paren += 1,
            b')' => paren = paren.saturating_sub(1),
            b'[' => bracket += 1,
            b']' => bracket = bracket.saturating_sub(1),
            b if b == sep && paren == 0 && bracket == 0 => {
                parts.push(&text[start..i]);
                start = i + 1;
            }
            _ => {}
        }
        i += 1;
    }
    parts.push(&text[start..]);
    parts
}

fn parse_declarations(body: &str) -> Vec<CssDecl> {
    let mut decls = Vec::new();
    for raw in split_top_level(body, b';') {
        let Some((key, val)) = raw.split_once(':') else {
            continue;
        };
        let prop = key.trim();
        if prop.is_empty() {
            continue;
        }
        let prop = if prop.starts_with("--") {
            prop.to_owned()
        } else {
            prop.to_ascii_lowercase()
        };
        let val = val
            .trim()
            .trim_end_matches("!important")
            .trim_end_matches("! important")
            .trim();
        if !val.is_empty() {
            decls.push(CssDecl {
                prop,
                value: val.to_owned(),
            });
        }
    }
    decls
}

fn parse_selector(raw: &str) -> Option<Selector> {
    let raw = raw.trim();
    if raw.is_empty() {
        return None;
    }
    let mut tokens: Vec<(Option<Combinator>, CompoundSelector)> = Vec::new();
    let mut rest = raw;
    let mut pending_comb: Option<Combinator> = None;
    let mut pseudo = PseudoElement::None;

    while !rest.is_empty() {
        let trimmed = rest.trim_start();
        if trimmed.len() < rest.len() && !tokens.is_empty() && pending_comb.is_none() {
            pending_comb = Some(Combinator::Descendant);
        }
        rest = trimmed;
        if rest.is_empty() {
            break;
        }
        if let Some(first) = rest.chars().next() {
            if matches!(first, '>' | '+' | '~') {
                pending_comb = Some(match first {
                    '>' => Combinator::Child,
                    '+' => Combinator::AdjacentSibling,
                    _ => Combinator::GeneralSibling,
                });
                rest = rest[1..].trim_start();
                continue;
            }
        }
        let (compound, next_rest, parsed_pseudo, interactive) = parse_compound_selector(rest)?;
        if interactive || parsed_pseudo == Some(PseudoElement::None) && false {
            return None;
        }
        if interactive {
            return None;
        }
        if let Some(p) = parsed_pseudo {
            pseudo = p;
        }
        tokens.push((pending_comb.take(), compound));
        rest = next_rest;
    }

    if tokens.is_empty() {
        return None;
    }
    if tokens.iter().any(|(_, c)| {
        c.id.is_none()
            && c.tag.is_none()
            && c.classes.is_empty()
            && c.attrs.is_empty()
            && c.pseudos.is_empty()
    }) && tokens.len() > 1
    {
        return None;
    }

    let mut specificity = (0u16, 0u16, 0u16);
    for (_, c) in &tokens {
        let spec = compound_specificity(c);
        specificity.0 = specificity.0.saturating_add(spec.0);
        specificity.1 = specificity.1.saturating_add(spec.1);
        specificity.2 = specificity.2.saturating_add(spec.2);
    }

    let mut steps = Vec::with_capacity(tokens.len());
    for idx in (0..tokens.len()).rev() {
        let comb = if idx + 1 < tokens.len() {
            tokens[idx + 1].0
        } else {
            None
        };
        steps.push(SelectorStep {
            comb,
            compound: tokens[idx].1.clone(),
        });
    }
    Some(Selector {
        steps,
        pseudo,
        specificity,
    })
}

fn compound_specificity(c: &CompoundSelector) -> (u16, u16, u16) {
    let a = if c.id.is_some() { 1 } else { 0 };
    let mut b = (c.classes.len() + c.attrs.len()) as u16;
    for p in &c.pseudos {
        match p {
            PseudoClass::Not(inner) => {
                let s = compound_specificity(inner);
                b = b.saturating_add(s.0 + s.1 + s.2);
            }
            _ => b = b.saturating_add(1),
        }
    }
    let c_tag = if c.tag.is_some() { 1 } else { 0 };
    (a, b, c_tag)
}

fn parse_compound_selector<'a>(
    mut rest: &'a str,
) -> Option<(CompoundSelector, &'a str, Option<PseudoElement>, bool)> {
    let mut sel = CompoundSelector::default();
    let mut pseudo = None;
    let mut parsed_anything = false;

    if let Some(after_star) = rest.strip_prefix('*') {
        rest = after_star;
        parsed_anything = true;
    } else if rest
        .chars()
        .next()
        .is_some_and(|ch| ch.is_ascii_alphabetic() || ch == '_' || ch == '-')
    {
        let end = rest
            .find(|ch: char| !(ch.is_ascii_alphanumeric() || ch == '-' || ch == '_'))
            .unwrap_or(rest.len());
        sel.tag = Some(rest[..end].to_ascii_lowercase());
        rest = &rest[end..];
        parsed_anything = true;
    }

    while !rest.is_empty() {
        let first = rest.chars().next()?;
        if first.is_whitespace() || matches!(first, '>' | '+' | '~') {
            break;
        }
        parsed_anything = true;
        match first {
            '.' => {
                rest = &rest[1..];
                let (ident, next) = read_css_ident(rest);
                if ident.is_empty() {
                    return None;
                }
                sel.classes.push(ident);
                rest = next;
            }
            '#' => {
                rest = &rest[1..];
                let (ident, next) = read_css_ident(rest);
                if ident.is_empty() {
                    return None;
                }
                sel.id = Some(ident);
                rest = next;
            }
            '[' => {
                let close = rest.find(']')?;
                let inner = rest[1..close].trim();
                rest = &rest[close + 1..];
                if let Some(attr) = parse_attr_selector(inner) {
                    sel.attrs.push(attr);
                } else {
                    return None;
                }
            }
            ':' => {
                let is_double = rest.starts_with("::");
                rest = if is_double { &rest[2..] } else { &rest[1..] };
                let end = rest
                    .find(|ch: char| {
                        ch.is_whitespace()
                            || matches!(ch, '.' | '#' | '[' | ':' | '>' | '+' | '~' | '(')
                    })
                    .unwrap_or(rest.len());
                let name = rest[..end].to_ascii_lowercase();
                rest = &rest[end..];
                let mut arg: Option<&str> = None;
                if rest.starts_with('(') {
                    let close = find_matching_paren(rest, 0);
                    arg = Some(&rest[1..close]);
                    rest = &rest[(close + 1).min(rest.len())..];
                }
                if is_double || matches!(name.as_str(), "before" | "after") {
                    match name.as_str() {
                        "before" => pseudo = Some(PseudoElement::Before),
                        "after" => pseudo = Some(PseudoElement::After),
                        _ => return None,
                    }
                    continue;
                }
                match name.as_str() {
                    "hover" | "focus" | "focus-within" | "focus-visible" | "active" | "target"
                    | "visited" => {
                        return Some((sel, rest, pseudo, true));
                    }
                    "root" => {
                        if sel.tag.is_none() {
                            sel.tag = Some("html".into());
                        }
                    }
                    "first-child" | "first-of-type" => sel.pseudos.push(PseudoClass::FirstChild),
                    "last-child" | "last-of-type" => sel.pseudos.push(PseudoClass::LastChild),
                    "only-child" => sel.pseudos.push(PseudoClass::OnlyChild),
                    "nth-child" => {
                        if let Some((a, b)) = arg.and_then(parse_nth) {
                            sel.pseudos.push(PseudoClass::NthChild(a, b));
                        }
                    }
                    "not" | "is" | "where" => {
                        if let Some(inner) = arg {
                            if name == "not" {
                                if let Some((inner_sel, _, _, interactive)) =
                                    parse_compound_selector(inner.trim())
                                {
                                    if !interactive {
                                        sel.pseudos.push(PseudoClass::Not(Box::new(inner_sel)));
                                    }
                                }
                            } else if let Some((inner_sel, _, _, interactive)) =
                                parse_compound_selector(inner.trim())
                            {
                                if !interactive {
                                    if sel.tag.is_none() {
                                        sel.tag = inner_sel.tag;
                                    }
                                    sel.classes.extend(inner_sel.classes);
                                    sel.attrs.extend(inner_sel.attrs);
                                }
                            }
                        }
                    }
                    "lang" | "empty" => {}
                    _ => {}
                }
            }
            _ => return None,
        }
    }

    parsed_anything.then_some((sel, rest, pseudo, false))
}

fn find_matching_paren(text: &str, open: usize) -> usize {
    let bytes = text.as_bytes();
    let mut depth = 0usize;
    let mut i = open;
    while i < bytes.len() {
        match bytes[i] {
            b'(' => depth += 1,
            b')' => {
                depth = depth.saturating_sub(1);
                if depth == 0 {
                    return i;
                }
            }
            _ => {}
        }
        i += 1;
    }
    bytes.len()
}

fn read_css_ident(text: &str) -> (String, &str) {
    let mut out = String::new();
    let mut bytes_used = 0usize;
    let mut chars = text.char_indices().peekable();
    while let Some((idx, ch)) = chars.next() {
        if ch == '\\' {
            if let Some((next_idx, next_ch)) = chars.next() {
                out.push(next_ch);
                bytes_used = next_idx + next_ch.len_utf8();
                continue;
            }
        }
        if ch.is_ascii_alphanumeric() || matches!(ch, '-' | '_') || !ch.is_ascii() {
            out.push(ch);
            bytes_used = idx + ch.len_utf8();
        } else {
            break;
        }
    }
    (out, &text[bytes_used..])
}

fn parse_attr_selector(inner: &str) -> Option<AttrSelector> {
    let ops = ["~=", "^=", "$=", "*=", "="];
    for op_str in ops {
        if let Some((left, right)) = inner.split_once(op_str) {
            let name = left.trim().to_ascii_lowercase();
            let val = right
                .trim()
                .trim_end_matches(" i")
                .trim_end_matches(" s")
                .trim()
                .trim_matches(|c| c == '"' || c == '\'')
                .to_owned();
            let op = match op_str {
                "=" => AttrOp::Equals(val),
                "~=" => AttrOp::ContainsWord(val),
                "^=" => AttrOp::Prefix(val),
                "$=" => AttrOp::Suffix(val),
                _ => AttrOp::Substring(val),
            };
            return Some(AttrSelector { name, op });
        }
    }
    let name = inner.trim().to_ascii_lowercase();
    (!name.is_empty()).then_some(AttrSelector {
        name,
        op: AttrOp::Exists,
    })
}

fn parse_nth(expr: &str) -> Option<(i32, i32)> {
    let expr = expr.trim().to_ascii_lowercase().replace(' ', "");
    if expr == "odd" {
        return Some((2, 1));
    }
    if expr == "even" {
        return Some((2, 0));
    }
    if let Some((a_part, b_part)) = expr.split_once('n') {
        let a = match a_part {
            "" | "+" => 1,
            "-" => -1,
            _ => a_part.parse::<i32>().ok()?,
        };
        let b = if b_part.is_empty() {
            0
        } else {
            b_part.parse::<i32>().ok()?
        };
        Some((a, b))
    } else {
        expr.parse::<i32>().ok().map(|n| (0, n))
    }
}

fn compound_matches(
    sel: &CompoundSelector,
    tag: &str,
    id: Option<&str>,
    classes: &[String],
    attrs: &[(String, String)],
    nth_child: usize,
) -> bool {
    if let Some(req_tag) = &sel.tag {
        if !tag.eq_ignore_ascii_case(req_tag) {
            return false;
        }
    }
    if let Some(req_id) = &sel.id {
        if id != Some(req_id.as_str()) {
            return false;
        }
    }
    for req_cls in &sel.classes {
        if !classes.iter().any(|c| c == req_cls) {
            return false;
        }
    }
    for req_attr in &sel.attrs {
        let found = attrs
            .iter()
            .find(|(k, _)| k.eq_ignore_ascii_case(&req_attr.name));
        let Some((_, val)) = found else {
            return false;
        };
        let matched = match &req_attr.op {
            AttrOp::Exists => true,
            AttrOp::Equals(target) => val == target,
            AttrOp::ContainsWord(word) => val.split_whitespace().any(|w| w == word),
            AttrOp::Prefix(p) => !p.is_empty() && val.starts_with(p),
            AttrOp::Suffix(s) => !s.is_empty() && val.ends_with(s),
            AttrOp::Substring(sub) => !sub.is_empty() && val.contains(sub),
        };
        if !matched {
            return false;
        }
    }
    for pseudo in &sel.pseudos {
        match pseudo {
            PseudoClass::FirstChild => {
                if nth_child != 1 {
                    return false;
                }
            }
            PseudoClass::LastChild | PseudoClass::OnlyChild => {
                if matches!(pseudo, PseudoClass::OnlyChild) && nth_child != 1 {
                    return false;
                }
            }
            PseudoClass::NthChild(a, b) => {
                let idx = nth_child as i32;
                let ok = if *a == 0 {
                    idx == *b
                } else {
                    (idx - *b) % *a == 0 && (idx - *b) / *a >= 0
                };
                if !ok {
                    return false;
                }
            }
            PseudoClass::Not(inner) => {
                if compound_matches(inner, tag, id, classes, attrs, nth_child) {
                    return false;
                }
            }
        }
    }
    true
}

fn implicit_root_matches(sel: &CompoundSelector, dark_mode: bool) -> bool {
    if let Some(tag) = &sel.tag {
        if !matches!(tag.as_str(), "html" | "body" | ":root") {
            return false;
        }
    }
    if sel.id.is_some() {
        return false;
    }
    for cls in &sel.classes {
        let ok = match cls.as_str() {
            "card" => true,
            "nightMode" | "night_mode" | "night-mode" | "dark" => dark_mode,
            _ => false,
        };
        if !ok {
            return false;
        }
    }
    for attr in &sel.attrs {
        let dark_val = if dark_mode { "dark" } else { "light" };
        if !(attr.name.contains("theme")
            && matches!(&attr.op, AttrOp::Equals(v) if v.eq_ignore_ascii_case(dark_val)))
        {
            return false;
        }
    }
    true
}

fn selector_matches_root_only(sel: &Selector, root: &ElementInfo, dark_mode: bool) -> bool {
    if sel.steps.len() == 1 {
        let c = &sel.steps[0].compound;
        return compound_matches(c, &root.tag, None, &root.classes, &root.attrs, 1)
            || implicit_root_matches(c, dark_mode);
    }
    if sel.steps.len() == 2 {
        return implicit_root_matches(&sel.steps[1].compound, dark_mode)
            && implicit_root_matches(&sel.steps[0].compound, dark_mode);
    }
    false
}

fn selector_matches(
    sel: &Selector,
    ancestors: &[ElementFrame],
    elem: &ElementInfo,
    dark_mode: bool,
) -> bool {
    let Some(first) = sel.steps.first() else {
        return false;
    };
    if !compound_matches(
        &first.compound,
        &elem.tag,
        elem.id.as_deref(),
        &elem.classes,
        &elem.attrs,
        elem.nth_child,
    ) {
        return false;
    }
    if sel.steps.len() == 1 {
        return true;
    }
    match_chain(
        &sel.steps,
        1,
        ancestors,
        elem.prev_siblings.as_slice(),
        dark_mode,
    )
}

fn match_chain(
    steps: &[SelectorStep],
    step_idx: usize,
    ancestors: &[ElementFrame],
    prev_siblings: &[SiblingInfo],
    dark_mode: bool,
) -> bool {
    if step_idx >= steps.len() {
        return true;
    }
    let step = &steps[step_idx];
    let comb = step.comb.unwrap_or(Combinator::Descendant);
    match comb {
        Combinator::Child => {
            if let Some((parent, grand)) = ancestors.split_last() {
                if compound_matches(
                    &step.compound,
                    &parent.info.tag,
                    parent.info.id.as_deref(),
                    &parent.info.classes,
                    &parent.info.attrs,
                    parent.info.nth_child,
                ) {
                    return match_chain(
                        steps,
                        step_idx + 1,
                        grand,
                        &parent.info.prev_siblings,
                        dark_mode,
                    );
                }
            } else if step_idx + 1 == steps.len()
                && implicit_root_matches(&step.compound, dark_mode)
            {
                return true;
            }
            false
        }
        Combinator::Descendant => {
            for pos in (0..ancestors.len()).rev() {
                let anc = &ancestors[pos];
                if compound_matches(
                    &step.compound,
                    &anc.info.tag,
                    anc.info.id.as_deref(),
                    &anc.info.classes,
                    &anc.info.attrs,
                    anc.info.nth_child,
                ) && match_chain(
                    steps,
                    step_idx + 1,
                    &ancestors[..pos],
                    &anc.info.prev_siblings,
                    dark_mode,
                ) {
                    return true;
                }
            }
            step_idx + 1 == steps.len() && implicit_root_matches(&step.compound, dark_mode)
        }
        Combinator::AdjacentSibling => {
            if let Some((prev, rest_sibs)) = prev_siblings.split_last() {
                if compound_matches(
                    &step.compound,
                    &prev.tag,
                    prev.id.as_deref(),
                    &prev.classes,
                    &prev.attrs,
                    1,
                ) {
                    return match_chain(steps, step_idx + 1, ancestors, rest_sibs, dark_mode);
                }
            }
            false
        }
        Combinator::GeneralSibling => {
            for pos in (0..prev_siblings.len()).rev() {
                let prev = &prev_siblings[pos];
                if compound_matches(
                    &step.compound,
                    &prev.tag,
                    prev.id.as_deref(),
                    &prev.classes,
                    &prev.attrs,
                    1,
                ) && match_chain(
                    steps,
                    step_idx + 1,
                    ancestors,
                    &prev_siblings[..pos],
                    dark_mode,
                ) {
                    return true;
                }
            }
            false
        }
    }
}

fn resolve_vars(
    value: &str,
    root_vars: &HashMap<String, String>,
    scoped_vars: &[(String, String)],
    depth: u8,
) -> String {
    if depth >= 8 || !value.contains("var(") {
        return value.to_owned();
    }
    let mut out = String::with_capacity(value.len());
    let mut rest = value;
    while let Some(idx) = rest.find("var(") {
        out.push_str(&rest[..idx]);
        let after = &rest[idx + 4..];
        let close = find_matching_paren(&rest[idx + 3..], 0);
        let inside = &after[..close.saturating_sub(1).min(after.len())];
        let (name, fallback) = match find_outside_parens(inside, &[b',']) {
            Some(comma) => (inside[..comma].trim(), Some(inside[comma + 1..].trim())),
            None => (inside.trim(), None),
        };
        let resolved = scoped_vars
            .iter()
            .rev()
            .find(|(k, _)| k == name)
            .map(|(_, v)| v.as_str())
            .or_else(|| root_vars.get(name).map(|v| v.as_str()))
            .or(fallback);
        if let Some(val) = resolved {
            out.push_str(&resolve_vars(val, root_vars, scoped_vars, depth + 1));
        }
        rest = &rest[idx + 3 + (close + 1).min(rest[idx + 3..].len())..];
    }
    out.push_str(rest);
    out
}

fn apply_css_decl(
    css: &mut ComputedCss,
    prop: &str,
    raw_value: &str,
    root_vars: &HashMap<String, String>,
    scoped_vars: &[(String, String)],
    current_color: Option<[u8; 3]>,
) {
    let resolved = resolve_vars(raw_value, root_vars, scoped_vars, 0);
    let value = resolved.trim();
    let lower = value.to_ascii_lowercase();
    match prop {
        "display" => {
            css.display = match lower.as_str() {
                "none" => Some(CssDisplay::None),
                "inline" => Some(CssDisplay::Inline),
                "inline-block" => Some(CssDisplay::InlineBlock),
                "inline-flex" | "inline-grid" => Some(CssDisplay::InlineFlex),
                "flex" | "grid" => Some(CssDisplay::Flex),
                "block" | "flow-root" | "list-item" | "table" | "table-row" | "table-cell" => {
                    Some(CssDisplay::Block)
                }
                _ => css.display,
            };
        }
        "flex-direction" => {
            css.flex_column = lower.starts_with("column");
        }
        "visibility" => {
            if matches!(lower.as_str(), "hidden" | "collapse") {
                css.visibility_hidden = Some(true);
            } else if lower == "visible" {
                css.visibility_hidden = Some(false);
            }
        }
        "opacity" => {
            if let Ok(op) = lower.parse::<f32>() {
                css.opacity = Some(op);
                css.opacity_zero = op <= 0.02;
            }
        }
        "position" => {
            css.pos_abs = Some(matches!(lower.as_str(), "absolute" | "fixed"));
        }
        "overflow" | "overflow-y" | "overflow-x" => {
            if lower.contains("hidden") || lower.contains("clip") {
                css.overflow_hidden = true;
            }
        }
        "width" => css.width_px = parse_px_len(&lower),
        "height" => css.height_px = parse_px_len(&lower),
        "max-width" => css.max_width_px = parse_px_len(&lower),
        "max-height" => css.max_height_px = parse_px_len(&lower),
        "color" => {
            if let Some((rgb, alpha)) = parse_color_alpha(value, current_color) {
                css.color = Some(rgb);
                css.color_alpha = alpha;
            }
        }
        "background" | "background-color" => {
            if matches!(lower.as_str(), "none" | "transparent") {
                css.bg = Some(([0, 0, 0], 0));
            } else if let Some(c) = parse_background_color(value, current_color) {
                css.bg = Some(c);
            }
        }
        "font-weight" => {
            if lower == "bold" || lower == "bolder" || lower.parse::<u32>().is_ok_and(|w| w >= 600)
            {
                css.bold = Some(true);
            } else if lower == "normal"
                || lower == "lighter"
                || lower.parse::<u32>().is_ok_and(|w| w < 600)
            {
                css.bold = Some(false);
            }
        }
        "font-style" => {
            if matches!(lower.as_str(), "italic" | "oblique") {
                css.italic = Some(true);
            } else if lower == "normal" {
                css.italic = Some(false);
            }
        }
        "text-decoration" | "text-decoration-line" => {
            if lower == "none" {
                css.underline = Some(false);
                css.overline = Some(false);
            } else {
                if lower.contains("underline") {
                    css.underline = Some(true);
                }
                if lower.contains("overline") {
                    css.overline = Some(true);
                }
            }
        }
        "font-size" => {
            if let Some((pct, is_rel)) = parse_font_size_pct(&lower) {
                css.font_size_pct = Some((pct, is_rel));
            }
            if parse_px_len(&lower).is_some_and(|px| px <= 0.5) {
                css.font_size_zero = true;
            }
        }
        "list-style" | "list-style-type" => {
            if lower.contains("none") {
                css.list_style_none = Some(true);
            } else if !lower.is_empty() && !matches!(lower.as_str(), "inherit" | "initial") {
                css.list_style_none = Some(false);
            }
        }
        "border" => {
            let side = parse_border_side(value, current_color);
            css.border_all = side;
            css.border_top = side;
            css.border_right = side;
            css.border_bottom = side;
            css.border_left = side;
        }
        "border-top" | "border-block-start" => {
            css.border_top = parse_border_side(value, current_color);
        }
        "border-top-width" | "border-top-style" => {
            if border_value_is_none(&lower) {
                css.border_top = BorderSide::None;
            } else if css.border_top == BorderSide::Unset {
                css.border_top = BorderSide::Set(None);
            }
        }
        "border-top-color" => {
            if let Some((rgb, a)) = parse_color_alpha(value, current_color) {
                if a > 0 && css.border_top != BorderSide::None {
                    css.border_top = BorderSide::Set(Some(rgb));
                }
            }
        }
        "border-right" | "border-inline-end" => {
            css.border_right = parse_border_side(value, current_color);
        }
        "border-right-width" | "border-right-style" => {
            if border_value_is_none(&lower) {
                css.border_right = BorderSide::None;
            } else if css.border_right == BorderSide::Unset {
                css.border_right = BorderSide::Set(None);
            }
        }
        "border-bottom" | "border-block-end" => {
            css.border_bottom = parse_border_side(value, current_color);
        }
        "border-bottom-width" | "border-bottom-style" => {
            if border_value_is_none(&lower) {
                css.border_bottom = BorderSide::None;
            } else if css.border_bottom == BorderSide::Unset {
                css.border_bottom = BorderSide::Set(None);
            }
        }
        "border-left" | "border-inline-start" => {
            css.border_left = parse_border_side(value, current_color);
        }
        "border-left-width" | "border-left-style" => {
            if border_value_is_none(&lower) {
                css.border_left = BorderSide::None;
            } else if css.border_left == BorderSide::Unset {
                css.border_left = BorderSide::Set(None);
            }
        }
        "border-left-color" | "border-inline-start-color" => {
            if let Some((rgb, a)) = parse_color_alpha(value, current_color) {
                if a > 0 && css.border_left != BorderSide::None {
                    css.border_left = BorderSide::Set(Some(rgb));
                }
            }
        }
        "border-color" => {
            if let Some((rgb, a)) = parse_color_alpha(value, current_color) {
                if a > 0 {
                    if let BorderSide::Set(ref mut c) = css.border_all {
                        *c = Some(rgb);
                    }
                    if let BorderSide::Set(ref mut c) = css.border_left {
                        *c = Some(rgb);
                    }
                    if let BorderSide::Set(ref mut c) = css.border_top {
                        *c = Some(rgb);
                    }
                }
            }
        }
        "border-radius" => {
            css.has_radius = !matches!(lower.as_str(), "0" | "0px" | "0%" | "none");
        }
        "padding" | "padding-left" | "padding-right" | "padding-top" | "padding-bottom"
        | "padding-inline" | "padding-block" => {
            css.has_padding = lower
                .split_whitespace()
                .any(|tok| parse_px_len(tok).is_some_and(|v| v > 0.5));
        }
        "content" => {
            css.content_literal = parse_css_content_string(value);
        }
        _ => {}
    }
}

fn border_value_is_none(lower: &str) -> bool {
    matches!(
        lower.trim(),
        "none" | "hidden" | "0" | "0px" | "0em" | "0rem"
    )
}

fn parse_border_side(value: &str, current_color: Option<[u8; 3]>) -> BorderSide {
    let lower = value.to_ascii_lowercase();
    if border_value_is_none(&lower) {
        return BorderSide::None;
    }
    let parts: Vec<&str> = value.split_whitespace().collect();
    if parts.iter().any(|p| {
        matches!(
            p.to_ascii_lowercase().as_str(),
            "none" | "hidden" | "0" | "0px"
        )
    }) && parts.len() <= 2
    {
        return BorderSide::None;
    }
    let mut color = None;
    for part in parts {
        if let Some((rgb, a)) = parse_color_alpha(part, current_color) {
            if a == 0 {
                return BorderSide::None;
            }
            color = Some(rgb);
            break;
        }
    }
    if color.is_none() {
        color = parse_color_alpha(value, current_color)
            .filter(|(_, a)| *a > 0)
            .map(|(rgb, _)| rgb);
    }
    BorderSide::Set(color)
}

fn parse_background_color(value: &str, current_color: Option<[u8; 3]>) -> Option<([u8; 3], u8)> {
    if let Some((rgb, alpha)) = parse_color_alpha(value, current_color) {
        return Some((rgb, alpha));
    }
    for part in split_top_level(value, b' ') {
        if let Some((rgb, alpha)) = parse_color_alpha(part.trim(), current_color) {
            return Some((rgb, alpha));
        }
    }
    if let Some(open) = value.find('(') {
        if let Some(close) = value.rfind(')') {
            for arg in split_top_level(&value[open + 1..close], b',') {
                for tok in arg.split_whitespace() {
                    if let Some((rgb, alpha)) = parse_color_alpha(tok, current_color) {
                        if alpha > 0 {
                            return Some((rgb, alpha));
                        }
                    }
                }
            }
        }
    }
    None
}

fn parse_css_content_string(value: &str) -> Option<String> {
    let trimmed = value.trim();
    if matches!(trimmed.to_ascii_lowercase().as_str(), "none" | "normal") {
        return None;
    }
    let quote = trimmed.chars().next().filter(|&c| c == '"' || c == '\'')?;
    if !trimmed.ends_with(quote) || trimmed.len() < 2 {
        return None;
    }
    let inner = &trimmed[1..trimmed.len() - 1];
    if inner.is_empty()
        || inner
            .chars()
            .all(|ch| ('\u{e000}'..='\u{f8ff}').contains(&ch) || ch.is_whitespace())
    {
        return None;
    }
    Some(inner.to_owned())
}

fn parse_px_len(value: &str) -> Option<f32> {
    let v = value.trim().to_ascii_lowercase();
    if v == "0" {
        return Some(0.0);
    }
    if let Some(n) = v.strip_suffix("px") {
        return n.trim().parse::<f32>().ok();
    }
    if let Some(n) = v.strip_suffix("rem").or_else(|| v.strip_suffix("em")) {
        return n.trim().parse::<f32>().ok().map(|x| x * 16.0);
    }
    if let Some(n) = v.strip_suffix("pt") {
        return n.trim().parse::<f32>().ok().map(|x| x * 1.333);
    }
    v.parse::<f32>().ok()
}

fn parse_font_size_pct(value: &str) -> Option<(u16, bool)> {
    Some(match value.trim() {
        "xx-small" => (65, false),
        "x-small" => (72, false),
        "small" | "smaller" => (82, value.trim() == "smaller"),
        "medium" => (100, false),
        "large" | "larger" => (122, value.trim() == "larger"),
        "x-large" => (135, false),
        "xx-large" => (150, false),
        other => {
            if let Some(n) = other.strip_suffix("rem") {
                return n
                    .trim()
                    .parse::<f32>()
                    .ok()
                    .map(|x| ((x * 100.0).round() as u16, false));
            }
            if let Some(n) = other.strip_suffix("em") {
                return n
                    .trim()
                    .parse::<f32>()
                    .ok()
                    .map(|x| ((x * 100.0).round() as u16, true));
            }
            if let Some(n) = other.strip_suffix('%') {
                return n
                    .trim()
                    .parse::<f32>()
                    .ok()
                    .map(|x| (x.round() as u16, true));
            }
            if let Some(n) = other.strip_suffix("px") {
                return n
                    .trim()
                    .parse::<f32>()
                    .ok()
                    .map(|px| (((px / 16.0) * 100.0).round() as u16, false));
            }
            return None;
        }
    })
}

fn parse_color_alpha(value: &str, current_color: Option<[u8; 3]>) -> Option<([u8; 3], u8)> {
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
            3 => Some(([digits[0] * 17, digits[1] * 17, digits[2] * 17], 255)),
            4 => Some((
                [digits[0] * 17, digits[1] * 17, digits[2] * 17],
                digits[3] * 17,
            )),
            6 => Some((
                [
                    digits[0] * 16 + digits[1],
                    digits[2] * 16 + digits[3],
                    digits[4] * 16 + digits[5],
                ],
                255,
            )),
            8 => Some((
                [
                    digits[0] * 16 + digits[1],
                    digits[2] * 16 + digits[3],
                    digits[4] * 16 + digits[5],
                ],
                digits[6] * 16 + digits[7],
            )),
            _ => None,
        };
    }
    let lower = value.to_ascii_lowercase();
    if lower == "transparent" {
        return Some(([0, 0, 0], 0));
    }
    if lower == "currentcolor" {
        return current_color.map(|c| (c, 255));
    }
    if let Some(inner) = lower
        .strip_prefix("rgba(")
        .or_else(|| lower.strip_prefix("rgb("))
        .and_then(|rest| rest.strip_suffix(')'))
    {
        return parse_rgb_args(inner);
    }
    if let Some(inner) = lower
        .strip_prefix("hsla(")
        .or_else(|| lower.strip_prefix("hsl("))
        .and_then(|rest| rest.strip_suffix(')'))
    {
        return parse_hsl_args(inner);
    }
    let rgb = match lower.as_str() {
        "black" => [0, 0, 0],
        "white" => [255, 255, 255],
        "red" => [220, 40, 30],
        "crimson" => [220, 20, 60],
        "firebrick" => [178, 34, 34],
        "darkred" => [139, 0, 0],
        "green" => [30, 140, 50],
        "limegreen" => [50, 205, 50],
        "forestgreen" => [34, 139, 34],
        "seagreen" => [46, 139, 87],
        "blue" => [30, 80, 210],
        "royalblue" => [65, 105, 225],
        "dodgerblue" => [30, 144, 255],
        "deepskyblue" => [0, 191, 255],
        "skyblue" => [135, 206, 235],
        "steelblue" => [70, 130, 180],
        "yellow" => [255, 235, 60],
        "orange" => [235, 140, 30],
        "orangered" => [255, 69, 0],
        "tomato" => [255, 99, 71],
        "coral" => [255, 127, 80],
        "salmon" => [250, 128, 114],
        "purple" => [130, 60, 170],
        "rebeccapurple" => [102, 51, 153],
        "indigo" => [75, 0, 130],
        "violet" => [200, 120, 220],
        "gray" | "grey" => [120, 120, 120],
        "darkgray" | "darkgrey" => [90, 90, 90],
        "dimgray" | "dimgrey" => [105, 105, 105],
        "lightgray" | "lightgrey" => [205, 205, 205],
        "slategray" | "slategrey" => [112, 128, 144],
        "brown" => [130, 85, 45],
        "pink" => [235, 130, 170],
        "cyan" | "aqua" => [30, 180, 190],
        "turquoise" => [64, 224, 208],
        "magenta" | "fuchsia" => [200, 50, 170],
        "lime" => [70, 210, 70],
        "navy" => [20, 35, 110],
        "teal" => [20, 120, 120],
        "maroon" => [120, 25, 35],
        "olive" => [120, 120, 30],
        "silver" => [190, 190, 190],
        "gold" | "goldenrod" => [210, 170, 40],
        _ => return None,
    };
    Some((rgb, 255))
}

fn parse_alpha_component(raw: &str) -> Option<u8> {
    let raw = raw.trim();
    if let Some(pct) = raw.strip_suffix('%') {
        let p = pct.trim().parse::<f32>().ok()?;
        return Some(((p / 100.0).clamp(0.0, 1.0) * 255.0).round() as u8);
    }
    let f = raw.parse::<f32>().ok()?;
    Some((f.clamp(0.0, 1.0) * 255.0).round() as u8)
}

fn parse_rgb_args(inner: &str) -> Option<([u8; 3], u8)> {
    let (main, slash_alpha) = match inner.split_once('/') {
        Some((l, r)) => (l, Some(r.trim())),
        None => (inner, None),
    };
    let tokens: Vec<&str> = if main.contains(',') {
        main.split(',').map(str::trim).collect()
    } else {
        main.split_whitespace().collect()
    };
    if tokens.len() < 3 {
        return None;
    }
    let parse_chan = |s: &str| -> Option<u8> {
        if let Some(p) = s.strip_suffix('%') {
            let v = p.trim().parse::<f32>().ok()?;
            Some(((v / 100.0).clamp(0.0, 1.0) * 255.0).round() as u8)
        } else {
            let v = s.parse::<f32>().ok()?;
            Some(v.clamp(0.0, 255.0).round() as u8)
        }
    };
    let rgb = [
        parse_chan(tokens[0])?,
        parse_chan(tokens[1])?,
        parse_chan(tokens[2])?,
    ];
    let alpha = slash_alpha
        .or_else(|| tokens.get(3).copied())
        .and_then(parse_alpha_component)
        .unwrap_or(255);
    Some((rgb, alpha))
}

fn parse_hsl_args(inner: &str) -> Option<([u8; 3], u8)> {
    let (main, slash_alpha) = match inner.split_once('/') {
        Some((l, r)) => (l, Some(r.trim())),
        None => (inner, None),
    };
    let tokens: Vec<&str> = if main.contains(',') {
        main.split(',').map(str::trim).collect()
    } else {
        main.split_whitespace().collect()
    };
    if tokens.len() < 3 {
        return None;
    }
    let h_str = tokens[0]
        .trim_end_matches("deg")
        .trim_end_matches("rad")
        .trim_end_matches("turn");
    let h_raw = h_str.parse::<f32>().ok()?;
    let h = if tokens[0].ends_with("rad") {
        (h_raw * 180.0 / std::f32::consts::PI).rem_euclid(360.0)
    } else if tokens[0].ends_with("turn") {
        (h_raw * 360.0).rem_euclid(360.0)
    } else {
        h_raw.rem_euclid(360.0)
    };
    let s = (tokens[1].trim_end_matches('%').parse::<f32>().ok()? / 100.0).clamp(0.0, 1.0);
    let l = (tokens[2].trim_end_matches('%').parse::<f32>().ok()? / 100.0).clamp(0.0, 1.0);
    let c = (1.0 - (2.0 * l - 1.0).abs()) * s;
    let x = c * (1.0 - ((h / 60.0) % 2.0 - 1.0).abs());
    let m = l - c / 2.0;
    let (r1, g1, b1) = match h as u32 {
        0..=59 => (c, x, 0.0),
        60..=119 => (x, c, 0.0),
        120..=179 => (0.0, c, x),
        180..=239 => (0.0, x, c),
        240..=299 => (x, 0.0, c),
        _ => (c, 0.0, x),
    };
    let rgb = [
        ((r1 + m) * 255.0).round().clamp(0.0, 255.0) as u8,
        ((g1 + m) * 255.0).round().clamp(0.0, 255.0) as u8,
        ((b1 + m) * 255.0).round().clamp(0.0, 255.0) as u8,
    ];
    let alpha = slash_alpha
        .and_then(parse_alpha_component)
        .or_else(|| tokens.get(3).copied().and_then(parse_alpha_component))
        .unwrap_or(255);
    Some((rgb, alpha))
}

#[derive(Clone, Debug)]
struct ListState {
    ordered: bool,
    next: usize,
    none: bool,
    inline_row: bool,
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
    tag: String,
    depth: usize,
    html: String,
}

#[derive(Clone, Debug)]
enum ContainerKind {
    Quote,
    Callout(BoxStyle),
}

#[derive(Clone, Debug)]
struct ContainerFrame {
    blocks: Vec<Block>,
    lists: Vec<ListState>,
    kind: ContainerKind,
}

#[derive(Clone, Debug)]
struct ElementFrame {
    info: ElementInfo,
    hidden: bool,
    is_block: bool,
    is_list: bool,
    is_list_item: bool,
    pushed_container: bool,
    is_details_closed: bool,
    in_summary: bool,
    vars_len: usize,
    prev_style: Style,
    prev_block_kind: BlockKind,
    span_group: bool,
    after_pseudo: Option<ComputedCss>,
    after_style: Style,
    child_count: usize,
    prev_siblings: Vec<SiblingInfo>,
}

struct Parser<'a> {
    sheet: &'a Stylesheet,
    dark_mode: bool,
    blocks: Vec<Block>,
    current: Vec<Inline>,
    current_marker: Option<String>,
    block_kind: BlockKind,
    style: Style,
    element_stack: Vec<ElementFrame>,
    var_scope: Vec<(String, String)>,
    span_groups: Vec<SpanGroup>,
    cloze_depth: usize,
    lists: Vec<ListState>,
    ruby: Option<RubyState>,
    svg: Option<SvgCollector>,
    skip: Option<(String, usize)>,
    table_stack: Vec<TableBuilder>,
    pre: Option<String>,
    container_stack: Vec<ContainerFrame>,
    hint_label: Option<HintLabel>,
    hint_body: Option<HintBody>,
    pending_hint: Option<u16>,
    hints_seen: u16,
    in_video: bool,
    root_child_count: usize,
    root_prev_siblings: Vec<SiblingInfo>,
}

#[derive(Clone, Debug)]
struct SvgCollector {
    tag_attrs: Vec<(String, String)>,
    css: ComputedCss,
    ancestors_icon_hint: bool,
    body: String,
    depth: usize,
}

impl<'a> Parser<'a> {
    fn new(sheet: &'a Stylesheet, dark_mode: bool) -> Self {
        Self {
            sheet,
            dark_mode,
            blocks: Vec::new(),
            current: Vec::new(),
            current_marker: None,
            block_kind: BlockKind::Paragraph,
            style: Style::default(),
            element_stack: Vec::new(),
            var_scope: Vec::new(),
            span_groups: Vec::new(),
            cloze_depth: 0,
            lists: Vec::new(),
            ruby: None,
            svg: None,
            skip: None,
            table_stack: Vec::new(),
            pre: None,
            container_stack: Vec::new(),
            hint_label: None,
            hint_body: None,
            pending_hint: None,
            hints_seen: 0,
            in_video: false,
            root_child_count: 0,
            root_prev_siblings: Vec::new(),
        }
    }

    fn push_block(&mut self, block: Block) {
        self.blocks.push(block);
    }

    fn in_table_cell(&self) -> bool {
        self.table_stack
            .last()
            .is_some_and(|builder| builder.in_cell.is_some())
    }

    fn active_block_kind(&self) -> BlockKind {
        for frame in self.element_stack.iter().rev() {
            if frame.info.tag == "p" {
                return BlockKind::Paragraph;
            }
            if frame.is_block && !frame.is_list_item {
                return BlockKind::Section;
            }
        }
        BlockKind::Paragraph
    }
}

/// Parse the rendered HTML used by Anki cards into simple display blocks.
#[allow(dead_code)]
pub fn parse_html(html: &str) -> Vec<Block> {
    parse_html_with_css(html, "", false)
}

pub fn parse_html_with_css(html: &str, css: &str, dark_mode: bool) -> Vec<Block> {
    let sheet = Stylesheet::from_css_and_html(css, html, dark_mode);
    parse_blocks_with_sheet(html, &sheet, dark_mode)
}

fn parse_blocks_with_sheet(html: &str, sheet: &Stylesheet, dark_mode: bool) -> Vec<Block> {
    let mut parser = Parser::new(sheet, dark_mode);
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
        if !rest[1..]
            .chars()
            .next()
            .is_some_and(|c| c.is_ascii_alphabetic() || matches!(c, '/' | '!' | '?'))
        {
            parser.handle_text("<");
            position = start + 1;
            continue;
        }
        let Some(end) = tag_end(rest) else {
            parser.handle_text(rest);
            position = html.len();
            break;
        };
        let raw_tag = &rest[..end];
        if !raw_tag.trim_end().ends_with("/>")
            && (tag_name_is(raw_tag, "script") || tag_name_is(raw_tag, "style"))
            && !raw_tag
                .trim_start_matches('<')
                .trim_start()
                .starts_with('/')
        {
            let tag = if tag_name_is(raw_tag, "script") {
                "script"
            } else {
                "style"
            };
            let after = &html[start + end..];
            let lower_after = after.to_ascii_lowercase();
            position = if let Some(close) = find_closing_rawtext(&lower_after, tag) {
                start + end + close
            } else {
                html.len()
            };
            continue;
        }
        parser.handle_tag(raw_tag);
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

fn is_void_tag(name: &str) -> bool {
    matches!(
        name,
        "area"
            | "base"
            | "br"
            | "col"
            | "embed"
            | "hr"
            | "img"
            | "input"
            | "link"
            | "meta"
            | "param"
            | "source"
            | "track"
            | "wbr"
    )
}

fn is_default_block_tag(name: &str) -> bool {
    matches!(
        name,
        "p" | "div"
            | "section"
            | "article"
            | "header"
            | "footer"
            | "main"
            | "nav"
            | "address"
            | "aside"
            | "figure"
            | "figcaption"
            | "h1"
            | "h2"
            | "h3"
            | "h4"
            | "h5"
            | "h6"
            | "details"
            | "summary"
            | "dl"
            | "dt"
            | "dd"
            | "blockquote"
            | "ul"
            | "ol"
            | "li"
            | "pre"
            | "table"
            | "hr"
    )
}

impl<'a> Parser<'a> {
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
        let self_closing = inner.ends_with('/')
            || (!closing && {
                let ne = inner
                    .find(|ch: char| ch.is_whitespace())
                    .unwrap_or(inner.len());
                is_void_tag(&inner[..ne].to_ascii_lowercase())
            });
        let raw_clean = if inner.ends_with('/') {
            inner[..inner.len() - 1].trim_end()
        } else {
            inner
        };
        let name_end = raw_clean
            .find(|ch: char| ch.is_whitespace())
            .unwrap_or(raw_clean.len());
        let name = raw_clean[..name_end].to_ascii_lowercase();
        if name.is_empty() {
            return;
        }

        if self.pre.is_some() && !(closing && name == "pre") {
            return;
        }
        if let Some(ref body) = self.hint_body {
            let finished = closing && name == body.tag && body.depth == 0;
            if finished {
                let body = self.hint_body.take().expect("hint body present");
                self.finish_hint_body(body);
                return;
            }
            if let Some(body) = self.hint_body.as_mut() {
                if name == body.tag {
                    if closing {
                        body.depth = body.depth.saturating_sub(1);
                    } else if !self_closing {
                        body.depth += 1;
                    }
                }
                body.html.push_str(raw);
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

        if let Some(mut svg) = self.svg.take() {
            if closing && name == "svg" {
                if svg.depth == 0 {
                    if let Some(points) = parse_svg_pitch_graph(&svg) {
                        self.current
                            .push(Inline::Marker(PitchMarker::Graph(points)));
                    }
                    return;
                }
                svg.depth -= 1;
                svg.body.push_str(raw);
                self.svg = Some(svg);
                return;
            }
            if !closing && name == "svg" && !self_closing {
                svg.depth += 1;
            }
            svg.body.push_str(raw);
            self.svg = Some(svg);
            return;
        }

        if let Some((skip_tag, depth)) = self.skip.as_mut() {
            if name == *skip_tag {
                if closing {
                    if *depth == 0 {
                        self.skip = None;
                    } else {
                        *depth -= 1;
                    }
                } else if !self_closing {
                    *depth += 1;
                }
            }
            return;
        }
        if !closing && matches!(name.as_str(), "head" | "title" | "template" | "noscript") {
            if !self_closing {
                self.skip = Some((name, 0));
            }
            return;
        }

        if self.ruby.is_some() {
            match name.as_str() {
                "ruby" if closing => {
                    self.finish_ruby();
                    return;
                }
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
                    return;
                }
                "rp" => {
                    if let Some(ruby) = self.ruby.as_mut() {
                        ruby.in_rp = !closing;
                    }
                    return;
                }
                _ if !is_default_block_tag(&name) => return,
                _ => {
                    self.finish_ruby();
                }
            }
        }

        if !closing && name == "ruby" {
            self.ruby = Some(RubyState {
                base: String::new(),
                reading: String::new(),
                chunks: Vec::new(),
                in_rt: false,
                in_rp: false,
                style: self.style.clone(),
            });
            return;
        }

        if closing {
            self.close_element(&name);
            return;
        }

        let attributes = parse_attributes(raw_clean, name_end);
        self.open_element(&name, attributes, self_closing);
    }

    fn open_element(&mut self, name: &str, attributes: Vec<(String, String)>, self_closing: bool) {
        let parent_hidden = self.element_stack.last().is_some_and(|frame| {
            frame.hidden || (frame.is_details_closed && name != "summary" && !frame.in_summary)
        });
        if parent_hidden {
            if !self_closing {
                self.push_hidden_frame(name, attributes);
            }
            return;
        }

        if name == "svg" {
            let elem = self.build_element_info(name, &attributes);
            let mut scope_mark = self.var_scope.len();
            let _ = &mut scope_mark;
            let (css, _, _) = self.sheet.compute(
                &self.element_stack,
                &elem,
                attribute(&attributes, "style"),
                &mut self.var_scope,
                self.style.color,
                self.dark_mode,
            );
            self.var_scope.truncate(scope_mark);
            if css.display == Some(CssDisplay::None)
                || css.visibility_hidden == Some(true)
                || css.opacity_zero
                || has_attr(&attributes, "hidden")
            {
                if !self_closing {
                    self.push_hidden_frame(name, attributes);
                }
                return;
            }
            if !self_closing {
                let ancestors_icon_hint =
                    element_or_ancestors_look_like_icon(&attributes, &self.element_stack);
                self.svg = Some(SvgCollector {
                    tag_attrs: attributes,
                    css,
                    ancestors_icon_hint,
                    body: String::new(),
                    depth: 0,
                });
            }
            return;
        }

        if name == "a"
            && attribute(&attributes, "class")
                .is_some_and(|v| v.split_whitespace().any(|c| c.eq_ignore_ascii_case("hint")))
        {
            let id = self.hints_seen;
            self.hints_seen = self.hints_seen.saturating_add(1);
            self.hint_label = Some(HintLabel {
                id,
                label: String::new(),
            });
            return;
        }

        if name == "div" && self.hint_body.is_none() {
            if let Some(id) = self.pending_hint.filter(|_| !self.in_table_cell()) {
                let is_hint_div = attribute(&attributes, "id")
                    .is_some_and(|v| v.starts_with("hint"))
                    || attribute(&attributes, "class").is_some_and(|v| {
                        v.split_whitespace().any(|c| c.eq_ignore_ascii_case("hint"))
                    });
                if is_hint_div {
                    self.pending_hint = None;
                    self.hint_body = Some(HintBody {
                        id,
                        tag: name.to_owned(),
                        depth: 0,
                        html: String::new(),
                    });
                    return;
                }
            }
        }

        let elem = self.build_element_info(name, &attributes);
        let vars_len = self.var_scope.len();
        let (css, before_css, after_css) = self.sheet.compute(
            &self.element_stack,
            &elem,
            attribute(&attributes, "style"),
            &mut self.var_scope,
            self.style.color,
            self.dark_mode,
        );

        let attr_hidden =
            has_attr(&attributes, "hidden") || (name == "dialog" && !has_attr(&attributes, "open"));
        let css_hidden = css.display == Some(CssDisplay::None)
            || css.visibility_hidden == Some(true)
            || css.opacity_zero
            || (css.display != Some(CssDisplay::Inline)
                && css.pos_abs != Some(true)
                && css.overflow_hidden
                && (css.max_height_px.is_some_and(|h| h <= 0.5)
                    || css.height_px.is_some_and(|h| h <= 0.5)));

        if css_hidden || (attr_hidden && css.display.is_none()) {
            self.var_scope.truncate(vars_len);
            if !self_closing {
                self.push_hidden_frame(name, attributes);
            }
            return;
        }

        if matches!(name, "table" | "tr" | "th" | "td") {
            self.var_scope.truncate(vars_len);
            self.handle_table_tag(name, false, &attributes);
            if !self_closing {
                self.push_passthrough_frame(elem);
            }
            return;
        }
        if matches!(
            name,
            "thead" | "tbody" | "tfoot" | "caption" | "colgroup" | "col"
        ) {
            self.var_scope.truncate(vars_len);
            if !self_closing {
                self.push_passthrough_frame(elem);
            }
            return;
        }
        if name == "br" {
            self.var_scope.truncate(vars_len);
            self.current.push(Inline::Break);
            return;
        }
        if name == "hr" {
            self.var_scope.truncate(vars_len);
            self.flush();
            self.push_block(Block::Rule);
            return;
        }
        if name == "input" {
            self.var_scope.truncate(vars_len);
            if attribute(&attributes, "id").is_some_and(|id| id.eq_ignore_ascii_case("typeans")) {
                self.current.push(Inline::TypeInput);
            }
            return;
        }
        if name == "video" {
            self.var_scope.truncate(vars_len);
            if let Some(src) = attribute(&attributes, "src").filter(|s| !s.trim().is_empty()) {
                self.push_media_tag(src, MediaKind::Video, &attributes);
            }
            if !self_closing {
                self.in_video = true;
                self.push_passthrough_frame(elem);
            }
            return;
        }
        if name == "source" {
            self.var_scope.truncate(vars_len);
            if !self.in_video {
                if let Some(src) = attribute(&attributes, "src").filter(|s| !s.trim().is_empty()) {
                    self.push_media_tag(src, MediaKind::Video, &attributes);
                }
            }
            return;
        }
        if name == "img" {
            self.var_scope.truncate(vars_len);
            let src = attribute(&attributes, "src").unwrap_or("").trim();
            if !src.is_empty() && !src.starts_with("data:") && !src.contains("://") {
                if media_kind_for_filename(src) == MediaKind::Video {
                    self.push_media_tag(src, MediaKind::Video, &attributes);
                } else {
                    let filename = normalize_media_filename(src);
                    if !filename.is_empty() {
                        let max_w = attribute(&attributes, "width")
                            .and_then(parse_px_len)
                            .or(css.width_px)
                            .or(css.max_width_px);
                        let max_h = attribute(&attributes, "height")
                            .and_then(parse_px_len)
                            .or(css.height_px)
                            .or(css.max_height_px);
                        self.current.push(Inline::Image {
                            src: filename,
                            max_w,
                            max_h,
                        });
                    }
                }
            }
            return;
        }
        if name == "pre" {
            self.var_scope.truncate(vars_len);
            self.flush();
            if !self_closing {
                self.pre = Some(String::new());
            }
            return;
        }

        let prev_style = self.style.clone();
        let prev_block_kind = self.block_kind;
        let is_list = matches!(name, "ul" | "ol");
        let is_list_item = name == "li";

        let in_inline_list = self.lists.last().is_some_and(|l| l.inline_row);
        let inline_flex_item = is_list_item
            && (matches!(
                css.display,
                Some(CssDisplay::Inline | CssDisplay::InlineBlock | CssDisplay::InlineFlex)
            ) || in_inline_list);

        let is_block = css.pos_abs != Some(true)
            && match css.display {
                Some(CssDisplay::Inline | CssDisplay::InlineBlock | CssDisplay::InlineFlex) => {
                    false
                }
                Some(CssDisplay::Block | CssDisplay::Flex) => true,
                Some(CssDisplay::None) => false,
                None => is_default_block_tag(name) && !inline_flex_item,
            };

        let is_pitch_overlay_el = css.pos_abs == Some(true) && css_has_pitch_effect(&css);
        let pseudo_pitch_before = before_css.as_ref().is_some_and(css_has_pitch_effect);
        let pseudo_pitch_after = after_css.as_ref().is_some_and(css_has_pitch_effect);
        let is_pitch_wrapper_el =
            css.display == Some(CssDisplay::InlineBlock) && css.pos_abs == Some(false);

        if is_pitch_overlay_el || pseudo_pitch_before {
            let eff_css = if pseudo_pitch_before {
                before_css.as_ref().unwrap_or(&css)
            } else {
                &css
            };
            self.apply_pitch_overlay_css(eff_css);
        }
        if is_nasal_overlay(&attributes) {
            self.apply_nasal_overlay_css(&attributes, &css);
        }

        let mut element_style = self.style.clone();
        let mut delta = Style::default();
        apply_tag_style(name, &attributes, &mut delta);
        apply_computed_css_to_style(&css, &mut element_style, name, &self.style);
        element_style.apply(&delta);

        let box_style = extract_box_style(&css, name, &attributes, &elem);
        let is_quote = name == "blockquote" && box_style.is_none();

        if is_block {
            self.flush();
            if is_list {
                let list_none = css.list_style_none.unwrap_or(false)
                    || attribute(&attributes, "style")
                        .is_some_and(|s| s.to_ascii_lowercase().contains("list-style"));
                let inline_row = (css.display == Some(CssDisplay::Flex) && !css.flex_column)
                    && css.list_style_none == Some(true);
                self.lists.push(ListState {
                    ordered: name == "ol",
                    next: 1,
                    none: list_none || inline_row,
                    inline_row,
                });
            } else if is_list_item {
                if let Some(list) = self.lists.last_mut() {
                    if list.none {
                        self.current_marker = Some(String::new());
                    } else if list.ordered {
                        let m = format!("{}. ", list.next);
                        list.next += 1;
                        self.current_marker = Some(m);
                    } else {
                        self.current_marker = Some("• ".to_owned());
                    }
                } else {
                    self.current_marker = Some("• ".to_owned());
                }
                self.block_kind = BlockKind::ListItem;
            } else if name == "p" {
                self.block_kind = BlockKind::Paragraph;
            } else if name == "dd" {
                self.current_marker = Some(String::new());
                self.block_kind = BlockKind::ListItem;
            } else {
                self.block_kind = BlockKind::Section;
            }
        } else if inline_flex_item
            && !self.current.is_empty()
            && !matches!(self.current.last(), Some(Inline::Text { text, .. }) if text.ends_with(' '))
        {
            push_text_plain(&mut self.current, " ", self.style.clone());
        }

        let mut pushed_container = false;
        if let Some(bstyle) = box_style.filter(|_| is_block && !self.in_table_cell()) {
            let blocks = std::mem::take(&mut self.blocks);
            let lists = std::mem::take(&mut self.lists);
            self.container_stack.push(ContainerFrame {
                blocks,
                lists,
                kind: ContainerKind::Callout(bstyle),
            });
            pushed_container = true;
        } else if is_quote && !self.in_table_cell() {
            let blocks = std::mem::take(&mut self.blocks);
            let lists = std::mem::take(&mut self.lists);
            self.container_stack.push(ContainerFrame {
                blocks,
                lists,
                kind: ContainerKind::Quote,
            });
            pushed_container = true;
        }

        self.style = element_style.clone();

        if let Some(ref bcss) = before_css {
            if let Some(lit) = &bcss.content_literal {
                let mut bstyle = element_style.clone();
                apply_computed_css_to_style(bcss, &mut bstyle, name, &element_style);
                push_text_with_furigana(&mut self.current, lit, bstyle);
            }
        }

        let mut span_group = false;
        if name == "span" || is_pitch_wrapper_el {
            let start = self.current.len();
            self.span_groups.push(SpanGroup {
                wrapper: is_pitch_wrapper_el,
                start,
            });
            span_group = true;
            if is_pitch_wrapper_el {
                self.current.push(Inline::Text {
                    text: String::new(),
                    style: self.style.clone(),
                });
            }
        }

        let is_details_closed = name == "details" && !has_attr(&attributes, "open");
        let in_summary =
            name == "summary" || self.element_stack.last().is_some_and(|f| f.in_summary);

        if pseudo_pitch_after && !css_has_pitch_effect(&css) {
            if let Some(acss) = &after_css {
                if let Some(pitch_color) = pitch_color_from_css(acss) {
                    self.style.pitch.color = Some(pitch_color);
                }
                if acss.overline == Some(true) || matches!(acss.border_top, BorderSide::Set(_)) {
                    self.style.pitch.high = true;
                }
                if matches!(acss.border_right, BorderSide::Set(_)) {
                    self.style.pitch.downstep = true;
                }
            }
        }

        let frame = ElementFrame {
            info: elem,
            hidden: false,
            is_block,
            is_list,
            is_list_item,
            pushed_container,
            is_details_closed,
            in_summary,
            vars_len,
            prev_style,
            prev_block_kind,
            span_group,
            after_pseudo: after_css,
            after_style: element_style,
            child_count: 0,
            prev_siblings: Vec::new(),
        };
        self.element_stack.push(frame);
        if self_closing {
            self.close_element(name);
        }
    }

    fn close_element(&mut self, name: &str) {
        let Some(pos) = self
            .element_stack
            .iter()
            .rposition(|frame| frame.info.tag == name)
        else {
            return;
        };
        while self.element_stack.len() > pos {
            let frame = self.element_stack.pop().expect("len > pos");
            self.finish_frame(frame);
        }
    }

    fn finish_frame(&mut self, frame: ElementFrame) {
        self.record_completed_sibling(&frame.info);
        if frame.hidden {
            self.var_scope.truncate(frame.vars_len);
            return;
        }
        if let Some(ref acss) = frame.after_pseudo {
            if let Some(lit) = &acss.content_literal {
                let mut astyle = frame.after_style.clone();
                apply_computed_css_to_style(acss, &mut astyle, &frame.info.tag, &frame.after_style);
                push_text_with_furigana(&mut self.current, lit, astyle);
            }
        }
        if frame.span_group {
            self.span_groups.pop();
        }
        if frame.is_block {
            self.flush();
            if frame.is_list {
                self.lists.pop();
            }
        }
        if frame.pushed_container {
            if let Some(container) = self.container_stack.pop() {
                let inner = std::mem::take(&mut self.blocks);
                self.blocks = container.blocks;
                self.lists = container.lists;
                if !inner.is_empty() {
                    let wrapped = match container.kind {
                        ContainerKind::Quote => Block::Quote(inner),
                        ContainerKind::Callout(style) => Block::Callout {
                            style,
                            content: inner,
                        },
                    };
                    self.blocks.push(wrapped);
                }
            }
        }
        self.var_scope.truncate(frame.vars_len);
        self.style = frame.prev_style;
        if frame.is_block {
            self.block_kind = self.active_block_kind();
        } else {
            self.block_kind = frame.prev_block_kind;
        }
    }

    fn push_hidden_frame(&mut self, name: &str, attributes: Vec<(String, String)>) {
        let elem = self.build_element_info(name, &attributes);
        self.element_stack.push(ElementFrame {
            info: elem,
            hidden: true,
            is_block: false,
            is_list: false,
            is_list_item: false,
            pushed_container: false,
            is_details_closed: false,
            in_summary: false,
            vars_len: self.var_scope.len(),
            prev_style: self.style.clone(),
            prev_block_kind: self.block_kind,
            span_group: false,
            after_pseudo: None,
            after_style: Style::default(),
            child_count: 0,
            prev_siblings: Vec::new(),
        });
    }

    fn push_passthrough_frame(&mut self, elem: ElementInfo) {
        self.element_stack.push(ElementFrame {
            info: elem,
            hidden: false,
            is_block: false,
            is_list: false,
            is_list_item: false,
            pushed_container: false,
            is_details_closed: false,
            in_summary: false,
            vars_len: self.var_scope.len(),
            prev_style: self.style.clone(),
            prev_block_kind: self.block_kind,
            span_group: false,
            after_pseudo: None,
            after_style: Style::default(),
            child_count: 0,
            prev_siblings: Vec::new(),
        });
    }

    fn build_element_info(&mut self, name: &str, attributes: &[(String, String)]) -> ElementInfo {
        let (nth_child, prev_siblings) = if let Some(parent) = self.element_stack.last_mut() {
            parent.child_count += 1;
            (parent.child_count, parent.prev_siblings.clone())
        } else {
            self.root_child_count += 1;
            (self.root_child_count, self.root_prev_siblings.clone())
        };
        let id = attribute(attributes, "id").map(str::to_owned);
        let classes = attribute(attributes, "class")
            .map(|c| c.split_whitespace().map(str::to_owned).collect())
            .unwrap_or_default();
        ElementInfo {
            tag: name.to_owned(),
            id,
            classes,
            attrs: attributes.to_vec(),
            nth_child,
            prev_siblings,
        }
    }

    fn record_completed_sibling(&mut self, info: &ElementInfo) {
        let sib = SiblingInfo {
            tag: info.tag.clone(),
            id: info.id.clone(),
            classes: info.classes.clone(),
            attrs: info.attrs.clone(),
        };
        let target = if let Some(parent) = self.element_stack.last_mut() {
            &mut parent.prev_siblings
        } else {
            &mut self.root_prev_siblings
        };
        if target.len() >= 16 {
            target.remove(0);
        }
        target.push(sib);
    }

    fn apply_pitch_overlay_css(&mut self, css: &ComputedCss) {
        let has_bottom = matches!(css.border_bottom, BorderSide::Set(_))
            || matches!(css.border_all, BorderSide::Set(_));
        let high = css.overline == Some(true)
            || (!has_bottom && matches!(css.border_top, BorderSide::Set(_)));
        let downstep = !has_bottom && matches!(css.border_right, BorderSide::Set(_));
        if !high && !downstep {
            return;
        }
        let pitch_color = pitch_color_from_css(css);
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
                if let Some(c) = pitch_color {
                    if style.pitch.color.is_none() {
                        style.pitch.color = Some(c);
                    }
                }
                last_text = Some(start + offset);
            }
        }
        if downstep {
            if let Some(index) = last_text {
                if let Some(style) = inline_style_mut(&mut self.current[index]) {
                    style.pitch.downstep = true;
                    if let Some(c) = pitch_color {
                        if style.pitch.color.is_none() {
                            style.pitch.color = Some(c);
                        }
                    }
                }
            }
        }
    }

    fn apply_nasal_overlay_css(&mut self, attributes: &[(String, String)], css: &ComputedCss) {
        let nasal_color = css
            .color
            .or_else(|| attribute(attributes, "color").and_then(parse_color));
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
                if nasal_color.is_some() {
                    style.pitch.nasal_color = nasal_color;
                }
            }
        }
    }

    fn handle_text(&mut self, text: &str) {
        if text.is_empty() || self.skip.is_some() {
            return;
        }
        if self
            .element_stack
            .last()
            .is_some_and(|frame| frame.hidden || frame.is_details_closed)
        {
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
            svg.body.push_str(text);
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
        let collapsed = collapse_html_whitespace(text);
        let at_start = self.current.is_empty()
            || matches!(self.current.last(), Some(Inline::Break))
            || self
                .current
                .iter()
                .all(|i| matches!(i, Inline::Text { text, .. } if text.trim().is_empty()));
        let trimmed = if at_start {
            collapsed.trim_start_matches(' ')
        } else {
            collapsed.as_str()
        };
        let last_ends_space = matches!(
            self.current.last(),
            Some(Inline::Text { text, .. }) if text.ends_with(' ')
        );
        let final_text = if last_ends_space {
            trimmed.trim_start_matches(' ')
        } else {
            trimmed
        };
        if final_text.is_empty() {
            return;
        }
        let mut style = self.style.clone();
        if self.cloze_depth > 0 {
            style.cloze = true;
            style.cloze_active = true;
        }
        for part in split_math(final_text) {
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
        let whitespace_only = self.current_marker.is_none()
            && self.current.iter().all(|inline| {
                matches!(
                    inline,
                    Inline::Text { text, style }
                        if text.trim().is_empty()
                            && style.pill.is_none()
                            && style.highlight.is_none()
                            && !style.cloze_active
                )
            });
        if self.current.is_empty() || whitespace_only {
            self.current.clear();
            self.current_marker = None;
            self.block_kind = self.active_block_kind();
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
        self.block_kind = self.active_block_kind();
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
        while let Some(frame) = self.element_stack.pop() {
            self.finish_frame(frame);
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
        while let Some(container) = self.container_stack.pop() {
            let inner = std::mem::take(&mut self.blocks);
            self.blocks = container.blocks;
            if !inner.is_empty() {
                let wrapped = match container.kind {
                    ContainerKind::Quote => Block::Quote(inner),
                    ContainerKind::Callout(style) => Block::Callout {
                        style,
                        content: inner,
                    },
                };
                self.blocks.push(wrapped);
            }
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
        let content = parse_blocks_with_sheet(&body.html, self.sheet, self.dark_mode);
        self.flush();
        self.push_block(Block::Hint {
            id: body.id,
            content,
        });
    }

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
        self.block_kind = self.active_block_kind();
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

    fn push_media_tag(&mut self, src: &str, kind: MediaKind, attributes: &[(String, String)]) {
        let name = normalize_media_filename(src);
        if name.is_empty() {
            return;
        }
        let animated = is_animated_image(&name);
        let autoplay = has_attr(attributes, "autoplay") || animated;
        let loop_ = has_attr(attributes, "loop") || animated;
        self.current.push(Inline::Media {
            path: name,
            kind,
            autoplay,
            loop_,
        });
    }
}

fn normalize_media_filename(src: &str) -> String {
    let decoded = decode_entities(src);
    decoded
        .rsplit(|ch| ch == '/' || ch == '\\')
        .next()
        .unwrap_or(decoded.as_str())
        .trim()
        .trim_matches('"')
        .trim_matches('\'')
        .to_owned()
}

fn has_attr(attributes: &[(String, String)], name: &str) -> bool {
    attributes
        .iter()
        .any(|(key, _)| key.eq_ignore_ascii_case(name))
}

fn is_animated_image(name: &str) -> bool {
    let ext = name.rsplit('.').next().unwrap_or("").to_ascii_lowercase();
    matches!(ext.as_str(), "gif" | "webp")
}

fn css_has_pitch_effect(css: &ComputedCss) -> bool {
    let has_bottom = matches!(css.border_bottom, BorderSide::Set(_))
        || matches!(css.border_all, BorderSide::Set(_));
    css.overline == Some(true)
        || (!has_bottom
            && (matches!(css.border_top, BorderSide::Set(_))
                || matches!(css.border_right, BorderSide::Set(_))))
}

fn pitch_color_from_css(css: &ComputedCss) -> Option<[u8; 3]> {
    if let BorderSide::Set(Some(c)) = css.border_top {
        return Some(c);
    }
    if let BorderSide::Set(Some(c)) = css.border_right {
        return Some(c);
    }
    css.color
}

fn apply_computed_css_to_style(
    css: &ComputedCss,
    style: &mut Style,
    tag: &str,
    parent_style: &Style,
) {
    if let Some(bold) = css.bold {
        style.bold = bold;
    }
    if let Some(italic) = css.italic {
        style.italic = italic;
    }
    if let Some(underline) = css.underline {
        style.underline = underline;
    }
    if let Some(color) = css.color {
        if css.color_alpha >= 16 {
            style.color = Some(color);
            if css.color_alpha < 195 {
                style.muted = true;
            }
        }
    }
    if css.opacity.is_some_and(|o| o > 0.02 && o < 0.85) {
        style.muted = true;
    }
    if let Some((pct, is_rel)) = css.font_size_pct {
        let base = if is_rel {
            parent_style.scale.unwrap_or(100) as u32
        } else {
            100
        };
        let computed = ((base * pct as u32) / 100).clamp(65, 150) as u8;
        if computed != 100 || parent_style.scale.is_some() {
            style.scale = Some(computed);
        }
    }

    let is_inline_el = matches!(
        css.display,
        Some(CssDisplay::Inline | CssDisplay::InlineBlock | CssDisplay::InlineFlex)
    ) || !is_default_block_tag(tag);

    let has_bottom = matches!(css.border_bottom, BorderSide::Set(_))
        || matches!(css.border_all, BorderSide::Set(_));
    let has_left = matches!(css.border_left, BorderSide::Set(_))
        || matches!(css.border_all, BorderSide::Set(_));

    if css.pos_abs != Some(true) {
        if css.overline == Some(true) {
            style.pitch.high = true;
            if let Some(c) = css.color {
                style.pitch.color = Some(c);
            }
        }
        if !has_bottom && !has_left {
            if matches!(css.border_top, BorderSide::Set(_)) {
                style.pitch.high = true;
                if let Some(c) = pitch_color_from_css(css) {
                    style.pitch.color = Some(c);
                }
            }
            if matches!(css.border_right, BorderSide::Set(_)) {
                style.pitch.downstep = true;
                if let Some(c) = pitch_color_from_css(css) {
                    style.pitch.color = Some(c);
                }
            }
        }
    }

    if is_inline_el {
        let border_col = match css.border_all {
            BorderSide::Set(c) => Some(c.unwrap_or([160, 160, 160])),
            _ => None,
        };
        if let Some((bg, alpha)) = css.bg {
            if alpha >= 16 {
                if css.has_radius || css.has_padding || border_col.is_some() {
                    style.pill = Some(PillStyle {
                        bg,
                        bg_alpha: alpha,
                        border: border_col,
                    });
                    style.highlight = None;
                } else {
                    style.highlight = Some(bg);
                }
            } else if alpha == 0 {
                style.highlight = None;
                style.pill = None;
            }
        } else if let Some(bcol) = border_col.filter(|_| css.has_radius || css.has_padding) {
            style.pill = Some(PillStyle {
                bg: [0, 0, 0],
                bg_alpha: 0,
                border: Some(bcol),
            });
        }
    }
}

fn extract_box_style(
    css: &ComputedCss,
    tag: &str,
    attributes: &[(String, String)],
    elem: &ElementInfo,
) -> Option<BoxStyle> {
    if matches!(tag, "li" | "ul" | "ol" | "table" | "tr" | "td" | "th")
        && matches!(css.bg, None)
        && !matches!(css.border_left, BorderSide::Set(_))
    {
        return None;
    }
    let is_card_root_div = elem.classes.iter().any(|c| c == "card")
        && !matches!(css.border_left, BorderSide::Set(_))
        && !matches!(css.border_all, BorderSide::Set(_));
    if is_card_root_div || has_attr(attributes, "hidden") {
        return None;
    }
    let border_left = match css.border_left {
        BorderSide::Set(c) => Some(c.unwrap_or([180, 180, 180])),
        _ => None,
    };
    let border_all = match css.border_all {
        BorderSide::Set(c) => Some(c.unwrap_or([120, 120, 120])),
        _ => None,
    };
    let bg = css.bg.filter(|(_, a)| {
        *a >= 4
            && (css.has_radius
                || css.has_padding
                || border_left.is_some()
                || border_all.is_some()
                || *a < 245)
    });
    if border_left.is_some() || border_all.is_some() || bg.is_some() {
        Some(BoxStyle {
            bg: bg.map(|(rgb, _)| rgb),
            bg_alpha: bg.map(|(_, a)| a).unwrap_or(0),
            border_left,
            border_all,
        })
    } else {
        None
    }
}

fn apply_tag_style(tag: &str, attributes: &[(String, String)], style: &mut Style) {
    match tag {
        "b" | "strong" | "dt" | "summary" => style.bold = true,
        "i" | "em" => style.italic = true,
        "u" | "ins" => style.underline = true,
        "small" => style.scale = Some(82),
        "h1" => {
            style.bold = true;
            style.scale = Some(130);
        }
        "h2" => {
            style.bold = true;
            style.scale = Some(120);
        }
        "h3" => {
            style.bold = true;
            style.scale = Some(110);
        }
        "h4" | "h5" | "h6" => style.bold = true,
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

fn parse_color(value: &str) -> Option<[u8; 3]> {
    parse_color_alpha(value, None)
        .filter(|(_, alpha)| *alpha >= 20)
        .map(|(rgb, _)| rgb)
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
        | Inline::Media { .. }
        | Inline::Image { .. } => None,
    }
}

fn is_nasal_overlay(attributes: &[(String, String)]) -> bool {
    attribute(attributes, "class").is_some_and(|value| {
        value.split_whitespace().any(|class| {
            matches!(
                class,
                "pronunciation-nasal-diacritic" | "nasal-diacritic" | "nasal-circle"
            )
        })
    })
}

fn element_or_ancestors_look_like_icon(
    attributes: &[(String, String)],
    ancestors: &[ElementFrame],
) -> bool {
    let attrs_hint = |attrs: &[(String, String)]| {
        if attribute(attrs, "aria-hidden").is_some_and(|v| v == "true") {
            return true;
        }
        attribute(attrs, "class").is_some_and(|cls| {
            let lower = cls.to_ascii_lowercase();
            [
                "icon", "chevron", "arrow", "caret", "star", "play", "audio", "sound", "speaker",
                "btn", "button", "dropdown", "freq", "menu", "toggle",
            ]
            .iter()
            .any(|k| lower.contains(k))
        })
    };
    attrs_hint(attributes)
        || ancestors.iter().rev().take(3).any(|f| {
            matches!(f.info.tag.as_str(), "button" | "summary") || attrs_hint(&f.info.attrs)
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

fn collapse_html_whitespace(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut prev_space = false;
    for ch in text.chars() {
        if matches!(ch, ' ' | '\n' | '\r' | '\t') {
            if !prev_space {
                out.push(' ');
                prev_space = true;
            }
        } else {
            out.push(ch);
            prev_space = false;
        }
    }
    out
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
    let Some(index) = output
        .iter()
        .rposition(|inline| matches!(inline, Inline::Text { text, .. } if !text.is_empty()))
    else {
        return false;
    };
    if !output[index + 1..]
        .iter()
        .all(|inline| matches!(inline, Inline::Text { text, .. } if text.is_empty()))
    {
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

fn parse_svg_pitch_graph(svg: &SvgCollector) -> Option<Vec<(f32, f32)>> {
    let w_attr = attribute(&svg.tag_attrs, "width").and_then(parse_px_len);
    let h_attr = attribute(&svg.tag_attrs, "height").and_then(parse_px_len);
    let w_px = svg.css.width_px.or(w_attr);
    let h_px = svg.css.height_px.or(h_attr);
    let view_box = attribute(&svg.tag_attrs, "viewbox")
        .or_else(|| attribute(&svg.tag_attrs, "viewBox"))
        .map(svg_numbers)
        .filter(|nums| nums.len() >= 4)
        .map(|nums| (nums[2], nums[3]));

    let aspect_wide = view_box.is_some_and(|(w, h)| h > 0.0 && w >= h * 1.8);
    let square_or_tall_vb = view_box.is_some_and(|(w, h)| h > 0.0 && w < h * 1.8);
    let small_size = matches!((w_px, h_px), (Some(w), Some(h)) if w <= 28.0 && h <= 28.0)
        || (w_px.is_some_and(|w| w <= 22.0) && h_px.is_none())
        || (h_px.is_some_and(|h| h <= 22.0) && w_px.is_none());
    let has_pitch_hint = attribute(&svg.tag_attrs, "class")
        .is_some_and(|c| c.to_ascii_lowercase().contains("pitch"));

    if !has_pitch_hint && (svg.ancestors_icon_hint || (small_size && !aspect_wide)) {
        return None;
    }
    if !has_pitch_hint && square_or_tall_vb {
        return None;
    }
    if !has_pitch_hint
        && attribute(&svg.tag_attrs, "fill").is_some_and(|v| v.eq_ignore_ascii_case("currentcolor"))
        && !aspect_wide
    {
        return None;
    }

    let data = attribute_value(&svg.body, "points").or_else(|| attribute_value(&svg.body, "d"))?;
    let has_bezier = data.contains(['c', 'C', 's', 'S', 'q', 'Q', 'a', 'A']);
    if has_bezier && !has_pitch_hint && !aspect_wide {
        return None;
    }

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
        fn runs(html: &str) -> Vec<(String, bool, bool, bool, Option<[u8; 3]>, Option<[u8; 3]>)> {
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
                ("ス".into(), false, false, false, Some([65, 105, 225]), None),
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
                ("コ".into(), true, false, true, None, Some([220, 40, 30])),
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
        assert_eq!(runs("カ\u{309A}"), vec![("カ".into(), false, true, None)]);
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
    fn media_extensions_distinguish_video_audio_and_stills() {
        for name in ["clip.mp4", "clip.WEBM", "clip.avi", "clip.mkv", "clip.ogv"] {
            assert!(is_video_filename(name), "{name}");
            assert_eq!(media_kind_for_filename(name), MediaKind::Video);
        }
        assert_eq!(media_kind_for_filename("still.webp"), MediaKind::Video);
        assert_eq!(media_kind_for_filename("still.gif"), MediaKind::Video);
        assert_eq!(media_kind_for_filename("sound.mp3"), MediaKind::Audio);
        assert_eq!(media_kind_for_filename("sound.opus"), MediaKind::Audio);
        assert_eq!(media_kind_for_filename("picture.svg"), MediaKind::Image);
        assert_eq!(media_kind_for_filename("picture.png"), MediaKind::Image);
        assert_eq!(media_kind_for_filename("picture.jpg"), MediaKind::Image);
        assert_eq!(media_kind_for_filename("picture.bmp"), MediaKind::Image);
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

    fn collect_text_and_images(blocks: &[Block]) -> (String, Vec<String>) {
        let mut text = String::new();
        let mut imgs = Vec::new();
        for block in blocks {
            match block {
                Block::Paragraph(inlines)
                | Block::Section(inlines)
                | Block::ListItem {
                    content: inlines, ..
                } => {
                    for inline in inlines {
                        match inline {
                            Inline::Text { text: t, .. } => text.push_str(t),
                            Inline::Ruby { base, reading, .. } => {
                                text.push_str(base);
                                text.push('(');
                                text.push_str(reading);
                                text.push(')');
                            }
                            Inline::Image { src, .. } => imgs.push(src.clone()),
                            _ => {}
                        }
                    }
                    text.push('\n');
                }
                Block::Quote(inner) | Block::Callout { content: inner, .. } => {
                    let (t, i) = collect_text_and_images(inner);
                    text.push_str(&t);
                    imgs.extend(i);
                }
                Block::Hint { content: inner, .. } => {
                    let (t, i) = collect_text_and_images(inner);
                    text.push_str(&t);
                    imgs.extend(i);
                }
                Block::Code { text: t } => {
                    text.push_str(t);
                    text.push('\n');
                }
                Block::Rule | Block::Table { .. } => {}
            }
        }
        (text, imgs)
    }

    #[test]
    fn css_hidden_elements_hover_and_decorative_svgs_are_skipped() {
        let css = r#"
            .freq-list { display: none; }
            .freq-dropdown:hover .freq-list { display: block; }
            .ghost-vis { visibility: hidden; }
            .ghost-op { opacity: 0; }
            .badge { font-size: 0.75em; opacity: 0.6; }
        "#;
        let html = r#"
            <div class="freq-dropdown">
                <span class="badge">831</span>
                <svg viewBox="0 0 24 24" width="12" height="12"><path d="M7 10l5 5 5-5z"/></svg>
                <div class="freq-list">831</div>
            </div>
            <div class="ghost-vis">hidden vis</div>
            <div class="ghost-op">hidden op</div>
            <div hidden>hidden attr</div>
            <p>Visible card body</p>
        "#;
        let blocks = parse_html_with_css(html, css, true);
        let (text, _) = collect_text_and_images(&blocks);
        assert_eq!(text.matches("831").count(), 1, "{text}");
        assert!(!text.contains("hidden vis"), "{text}");
        assert!(!text.contains("hidden op"), "{text}");
        assert!(!text.contains("hidden attr"), "{text}");
        assert!(text.contains("Visible card body"), "{text}");
        assert!(!blocks.iter().any(|block| matches!(
            block,
            Block::Section(inlines) | Block::Paragraph(inlines)
                if inlines.iter().any(|i| matches!(i, Inline::Marker(PitchMarker::Graph(_))))
        )));
    }

    #[test]
    fn scripts_with_angle_brackets_and_self_closing_svgs_do_not_truncate() {
        let html = r#"
            <div class="top">
                <ruby>運<rt>はこ</rt></ruby>ぶ
                <svg class="icon" viewBox="0 0 16 16" />
            </div>
            <script>
                for (let i = 0; i < 10; i++) {
                    const s = '<div class="unclosed"><span style="x=1">';
                    if (i > 2 && i < 5) console.log(s);
                }
            </script>
            <div class="pic"><img src="scene.jpg"></div>
            <div class="sentence">とにかく こいつをベッドまで運ぶぞ</div>
        "#;
        let blocks = parse_html_with_css(html, "", true);
        let (text, imgs) = collect_text_and_images(&blocks);
        assert!(text.contains("運(はこ)ぶ"), "{text}");
        assert!(text.contains("とにかく こいつをベッドまで運ぶぞ"), "{text}");
        assert_eq!(imgs, vec!["scene.jpg"]);
    }

    #[test]
    fn stylesheet_pitch_night_mode_variables_and_badge_pill() {
        let css = r#"
            :root { --pitch-c: #1e88e5; }
            .nightMode { --pitch-c: #4fc3f7; }
            .pitch-high { border-top: 2px solid var(--pitch-c); }
            .pitch-drop { border-right: 2px solid var(--pitch-c); }
            .pitch-badge {
                background-color: #ffffff;
                color: #111111;
                border-radius: 4px;
                padding: 1px 4px;
                font-size: 0.75em;
            }
        "#;
        let html = r#"<span style="color: var(--pitch-c)"><span>は</span><span class="pitch-high">こぶ</span></span> <span class="pitch-badge">0</span>"#;
        let blocks = parse_html_with_css(html, css, true);
        let Block::Paragraph(content) = &blocks[0] else {
            panic!("expected paragraph: {blocks:?}");
        };
        assert!(content.iter().any(|inline| matches!(
            inline,
            Inline::Text { text, style }
                if text == "こぶ"
                    && style.pitch.high
                    && style.pitch.color == Some([79, 195, 247])
                    && style.color == Some([79, 195, 247])
        )));
        assert!(content.iter().any(|inline| matches!(
            inline,
            Inline::Text { text, style }
                if text == "0"
                    && style.pill == Some(PillStyle { bg: [255, 255, 255], bg_alpha: 255, border: None })
                    && style.color == Some([17, 17, 17])
                    && style.scale == Some(75)
        )));
    }

    #[test]
    fn structured_dictionary_pills_callouts_details_and_definition_lists() {
        let css = r#"
            .dict-entry { border-left: 2px solid #dcdcdc; background: rgba(255,255,255,0.04); padding: 6px; }
            .tags { display: flex; list-style-type: none; }
            .tag { background-color: #565656; color: #ffffff; border-radius: 4px; padding: 1px 4px; font-size: 0.8em; }
            .tag-red { background-color: #b73b3b; color: #ffffff; border-radius: 4px; padding: 1px 4px; font-size: 0.8em; }
            ol.glossary { list-style-type: none; }
            [data-sc-content="example-sentence"] { border-left: 2px solid #888888; background: rgba(255,255,255,0.06); border-radius: 4px; padding: 4px; }
            [data-sc-content="example-sentence-b"] { font-size: 0.8em; color: rgba(255,255,255,0.7); }
            [data-sc-content="notes"] { border-left: 2px solid #c98a2c; background: rgba(201,138,44,0.12); padding: 4px; }
        "#;
        let html = r#"
            <div class="dict-entry">
                <div>(★, Jitendex.org [2026-10-03])</div>
                <ul class="tags">
                    <li>*</li>
                    <li><span class="tag">5-dan</span></li>
                    <li><span class="tag">transitive</span></li>
                </ul>
                <ol class="glossary">
                    <li>① to carry | to transport
                        <div data-sc-content="example-sentence">
                            荷物を二階に<span style="color:#4caf50">運んで</span>いただけませんか。
                            <div data-sc-content="example-sentence-b">Would you carry my luggage upstairs? [1]</div>
                        </div>
                    </li>
                    <li>② <span class="tag-red">honorific</span> to come | to go
                        <div data-sc-content="notes">
                            <div style="opacity:0.7;font-size:0.8em">Note</div>
                            <div>as お運びになる、お運び下さる、etc.</div>
                        </div>
                    </li>
                </ol>
                <details><summary>Inflections</summary><p>Hidden unless open</p></details>
                <dl><dt>Usage</dt><dd>Common verb</dd></dl>
            </div>
        "#;
        let blocks = parse_html_with_css(html, css, true);
        let Block::Callout { style, content } = &blocks[0] else {
            panic!("expected outer dict Callout, got {blocks:?}");
        };
        assert_eq!(style.border_left, Some([220, 220, 220]));

        let (text, _) = collect_text_and_images(&blocks);
        assert!(text.contains("Jitendex.org"), "{text}");
        assert!(text.contains("* 5-dan transitive"), "{text}");
        assert!(text.contains("to carry"), "{text}");
        assert!(
            !text.contains("1. "),
            "list-style: none should suppress numeric marker: {text}"
        );
        assert!(
            text.contains("Would you carry my luggage upstairs?"),
            "{text}"
        );
        assert!(text.contains("honorific"), "{text}");
        assert!(text.contains("Note"), "{text}");
        assert!(text.contains("お運びになる"), "{text}");
        assert!(text.contains("Inflections"), "{text}");
        assert!(!text.contains("Hidden unless open"), "{text}");
        assert!(text.contains("Usage"), "{text}");
        assert!(text.contains("Common verb"), "{text}");

        let has_nested_note_callout = content.iter().any(|block| match block {
            Block::Callout { style, .. } => style.border_left == Some([201, 138, 44]),
            _ => false,
        });
        assert!(
            has_nested_note_callout,
            "missing amber note callout: {content:#?}"
        );
    }
}
