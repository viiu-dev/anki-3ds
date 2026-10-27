// Copyright: Anki 3DS contributors
// License: GNU AGPL, version 3 or later; http://www.gnu.org/licenses/agpl.html

mod assets;
mod auth_store;
mod backend;
mod clock;
mod i18n;
mod review_text;
mod sync;
mod time_correction;
mod ui;
use std::collections::HashSet;
use std::fs;
use std::io::Write;
use std::time::Duration;
use std::time::Instant;

use anki_proto::decks::DeckTreeNode;
use backend::Backend;
use backend::DeckOptions;
use backend::NoteDraft;
use backend::OfferedCard;
use ctru::prelude::*;
use i18n::tr;
use ui::*;

const LOG: &str = "sdmc:/3ds/anki-3ds/prototype-events.txt";
#[cfg(feature = "emulator")]
const TEST_SCRIPT: &str = "sdmc:/3ds/anki-3ds/test-script.txt";
const ROW_Y: f32 = 43.0;
const ROW_H: f32 = 24.0;
const ROWS: usize = 6;

fn event(text: &str) {
    if let Ok(mut log) = fs::OpenOptions::new().create(true).append(true).open(LOG) {
        let _ = writeln!(log, "{text}");
        let _ = log.sync_all();
    }
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
    ids: Vec<i64>,
    selected: usize,
    preview: (String, String),
    titles: Vec<String>,
    page: usize,
}
impl Browser {
    fn load(backend: &mut Backend, query: String) -> backend::Result<Self> {
        let ids = backend.search(&query)?;
        let mut browser = Self {
            query,
            ids,
            selected: 0,
            preview: Default::default(),
            titles: Vec::new(),
            page: usize::MAX,
        };
        browser.refresh(backend)?;
        Ok(browser)
    }
    fn refresh(&mut self, backend: &mut Backend) -> backend::Result<()> {
        if self.ids.is_empty() {
            self.preview = ("Sin resultados".into(), String::new());
            return Ok(());
        }
        self.selected = self.selected.min(self.ids.len() - 1);
        self.preview = backend.preview(self.ids[self.selected])?;
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
    Info {
        title: String,
        text: String,
        previous: Box<Screen>,
    },
    Error(String),
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
    Help,
    Stats,
    Sync,
    Login,
    Logout,
    Clock,
    Language,
    Undo,
    Exit,
}

fn user_error(error: &(dyn std::error::Error + 'static)) -> String {
    if let Some(error) = error.downcast_ref::<anki::error::AnkiError>() {
        error.message(&anki::prelude::I18n::new(&[i18n::language()]))
    } else {
        error_text(error)
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
                            COLOR_COUNT_NEW
                        } else {
                            COLOR_TEXT_DARK
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
                COLOR_TEXT_MUTED,
                &format!("{} / {}  UP/DOWN", start + 1, max + 1),
            );
        }
        max
    }
}
fn header(width: f32, title: &str) {
    Ui::draw_rect(0.0, 0.0, width, 42.0, rgba(239, 244, 249, 255));
    Ui::draw_line(0.0, 42.0, width, 42.0, 1.0, COLOR_PANEL_BORDER);
    Ui::fit_text(
        width / 2.0,
        12.0,
        0.60,
        width - 24.0,
        COLOR_TEXT_DARK,
        title,
    );
}
fn footer(text: &str) {
    Ui::draw_rect(0.0, 224.0, BOTTOM_WIDTH, 16.0, rgba(231, 237, 243, 255));
    Ui::fit_text(160.0, 226.0, 0.40, 304.0, COLOR_TEXT_MUTED, text);
}
fn counts(new: u32, learn: u32, review: u32, y: f32) {
    for (x, label, value, color) in [
        (75.0, tr("New"), new, COLOR_COUNT_NEW),
        (200.0, tr("Learning"), learn, COLOR_COUNT_LEARN),
        (325.0, tr("Review"), review, COLOR_COUNT_DUE),
    ] {
        Ui::draw_text_centered(x, y, 0.48, color, &value.to_string());
        Ui::draw_text_centered(x, y + 17.0, 0.43, COLOR_TEXT_MUTED, label);
    }
}
fn counts_compact(new: u32, learn: u32, review: u32, y: f32) {
    for (x, label, value, color) in [
        (70.0, tr("New"), new, COLOR_COUNT_NEW),
        (160.0, tr("Learning"), learn, COLOR_COUNT_LEARN),
        (250.0, tr("Review"), review, COLOR_COUNT_DUE),
    ] {
        Ui::draw_text_centered(x, y, 0.54, color, &value.to_string());
        Ui::draw_text_centered(x, y + 17.0, 0.37, COLOR_TEXT_MUTED, label);
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
    Ui::draw_rect(
        0.0,
        y,
        320.0,
        24.0,
        if selected {
            rgba(220, 236, 248, 255)
        } else {
            COLOR_PANEL_WHITE
        },
    );
    Ui::draw_line(0.0, y + 24.0, 320.0, y + 24.0, 1.0, COLOR_PANEL_BORDER);
    Ui::draw_text(
        10.0,
        y + 4.0,
        0.52,
        COLOR_TEXT_DARK,
        &Ui::ellipsize(label, 0.52, 282.0),
    );
}
const MENU: [&str; 14] = [
    "Add note",
    "Edit note",
    "Browse",
    "Help",
    "Statistics",
    "Undo",
    "Deck options",
    "Sync",
    "Log in",
    "Log out",
    "Update clock",
    "Language",
    "Font scale",
    "Exit",
];
const FONT_SCALE_MENU_INDEX: usize = 12;

fn main() {
    let apt = Apt::new().expect("APT initialization");
    let mut hid = Hid::new().expect("HID initialization");
    let _gfx = Gfx::new().expect("graphics initialization");
    if !Ui::init() {
        panic!("Failed to initialize Citro2D/Citro3D UI engine");
    }
    use tracing_subscriber::prelude::*;
    tracing_subscriber::registry()
        .with(
            tracing_subscriber::fmt::layer()
                .without_time()
                .with_ansi(false)
                .with_writer(|| {
                    fs::OpenOptions::new()
                        .create(true)
                        .append(true)
                        .open(LOG)
                        .expect("sync log")
                })
                .with_filter(
                    tracing_subscriber::filter::Targets::new()
                        .with_target("anki::sync::collection::normal", tracing::Level::DEBUG)
                        .with_target("anki_3ds_sync_transport", tracing::Level::DEBUG),
                ),
        )
        .init();
    i18n::init();
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
    let mut exit_pending = false;
    let mut clock_job: Option<clock::Job> = None;
    let mut next_clock_check =
        Instant::now() + Duration::from_secs(if clock::needs_refresh() { 0 } else { 1800 });
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
    let mut root = DeckTreeNode::default();
    let mut screen = if let Some(error) = startup_error {
        Screen::Error(error)
    } else {
        match backend.as_mut() {
            Some(b) => match b.deck_tree() {
                Ok(tree) => {
                    root = tree;
                    Screen::Decks
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
    let mut last_review_card: Option<(i64, bool)> = None;
    let mut top_layout = TextLayout::default();
    let mut top_card = RichTextLayout::default();
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
    let trace_touch = fs::metadata("sdmc:/3ds/anki-3ds/trace-touch").is_ok();

    while apt.main_loop() {
        if let Some(result) = clock_job.as_mut().and_then(clock::Job::poll) {
            clock_job = None;
            match result {
                Ok(offset) => {
                    if let Some(backend) = backend.as_mut() {
                        if let Err(error) = backend.set_clock_offset(offset) {
                            event(&format!("CLOCK_OFFSET_ERROR {error}"));
                        }
                    }
                }
                Err(error) => event(&format!("CLOCK_AUTO_ERROR {error}")),
            }
        }
        let connected = clock::connected();
        if connected && !clock_connected && clock::needs_refresh() {
            next_clock_check = Instant::now();
        }
        clock_connected = connected;
        if clock_job.is_none()
            && job.is_none()
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
            backend = done.backend;
            auth = done.auth;
            screen = match done.result {
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
                    Screen::Info {
                        title: tr("Synced").into(),
                        text: tr("Collection synced.").into(),
                        previous: Box::new(Screen::Decks),
                    }
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
        hid.scan_input();
        let down = hid.keys_down();
        let held = hid.keys_held();
        if matches!(screen, Screen::Question { .. } | Screen::Answer { .. }) {
            let factor = i18n::font_scale_factor();
            let mut delta = 0.0f32;
            if held.intersects(KeyPad::DPAD_DOWN | KeyPad::CSTICK_DOWN) {
                delta += 5.5 * factor;
            }
            if held.intersects(KeyPad::DPAD_UP | KeyPad::CSTICK_UP) {
                delta -= 5.5 * factor;
            }
            let (_, pad_y) = hid.circlepad_position();
            if pad_y.abs() > 24 {
                delta -= (f32::from(pad_y) / 156.0) * 9.0 * factor;
            } else if held.contains(KeyPad::CPAD_DOWN) {
                delta += 5.5 * factor;
            } else if held.contains(KeyPad::CPAD_UP) {
                delta -= 5.5 * factor;
            }
            if held.contains(KeyPad::ZL) {
                delta -= 9.0 * factor;
            }
            if held.contains(KeyPad::ZR) {
                delta += 9.0 * factor;
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
        let mut action = if down.contains(KeyPad::START) {
            Action::Exit
        } else if down.contains(KeyPad::SELECT) {
            Action::Menu
        } else if down.contains(KeyPad::B) {
            Action::Back
        } else if matches!(screen, Screen::Editor { .. } | Screen::Options { .. })
            && down.contains(KeyPad::X)
        {
            Action::Save
        } else if matches!(screen, Screen::Browser(_)) && down.contains(KeyPad::X) {
            Action::Search
        } else if matches!(screen, Screen::Question { .. }) && down.contains(KeyPad::X) {
            Action::Edit
        } else if matches!(screen, Screen::Answer { .. }) && down.contains(KeyPad::Y) {
            Action::Rate(0)
        } else if matches!(screen, Screen::Answer { .. }) && down.contains(KeyPad::X) {
            Action::Rate(1)
        } else if matches!(screen, Screen::Answer { .. }) && down.contains(KeyPad::R) {
            Action::Rate(3)
        } else if down.contains(KeyPad::A) {
            Action::Primary
        } else if matches!(screen, Screen::Question { .. } | Screen::Answer { .. }) {
            // Review scrolling is pixel-based and handled above from the held
            // key state. Do not also emit line-stepping actions here.
            Action::None
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
        if down.contains(KeyPad::TOUCH) {
            let (x, y) = hid.touch_position();
            if trace_touch {
                event(&format!("TOUCH x={x} y={y}"));
            }
            let x = x as f32;
            let y = y as f32;
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
                    0 => Action::None,
                    1 => Action::Add,
                    2 => Action::Edit,
                    _ => Action::Browse,
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
                Screen::Question { .. }
                    if (128.0..200.0).contains(&y) && (40.0..280.0).contains(&x) =>
                {
                    Action::Reveal
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
                Screen::Browser(_) | Screen::Editor { .. } | Screen::Menu { .. }
                    if x >= 304.0 && (43.0..187.0).contains(&y) =>
                {
                    if y < 115.0 {
                        Action::Up
                    } else {
                        Action::Down
                    }
                }
                Screen::Browser(_) if y < 42.0 => Action::Search,
                Screen::Browser(b) if (43.0..187.0).contains(&y) => {
                    let index = b.page * ROWS + ((y - 43.0) / 24.0) as usize;
                    if index < b.ids.len() {
                        b.selected = index;
                        Action::Primary
                    } else {
                        Action::None
                    }
                }
                Screen::Browser(_) if (194.0..222.0).contains(&y) => {
                    if x < 160.0 {
                        Action::Edit
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
                Screen::Options { field, .. } if (43.0..91.0).contains(&y) => {
                    *field = ((y - 43.0) / 24.0) as usize;
                    Action::Field
                }
                Screen::Menu { selected, .. } if (43.0..187.0).contains(&y) => {
                    *selected = *selected / ROWS * ROWS + ((y - 43.0) / 24.0) as usize;
                    *selected = (*selected).min(MENU.len() - 1);
                    Action::Primary
                }
                Screen::Info { .. } if (194.0..222.0).contains(&y) => Action::Back,
                Screen::Empty { .. } if (110.0..144.0).contains(&y) => Action::Back,
                _ => Action::None,
            };
            if y >= 224.0 && !matches!(screen, Screen::Question { .. } | Screen::Answer { .. }) {
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
        if matches!(action, Action::Exit) {
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
                if *selected == FONT_SCALE_MENU_INDEX {
                    let direction = match action {
                        Action::Primary | Action::Right => 1,
                        Action::Left => -1,
                        _ => 0,
                    };
                    if direction != 0 {
                        let percent = i18n::adjust_font_scale(direction)?;
                        event(&format!("FONT_SCALE {percent}%"));
                        review_scroll = 0.0;
                        return Ok(());
                    }
                }
                if matches!(action, Action::Primary) {
                    action = [
                        Action::Add,
                        Action::Edit,
                        Action::Browse,
                        Action::Help,
                        Action::Stats,
                        Action::Undo,
                        Action::Options,
                        Action::Sync,
                        Action::Login,
                        Action::Logout,
                        Action::Clock,
                        Action::Language,
                        Action::None,
                        Action::Exit,
                    ][*selected]
                        .clone();
                    let old = std::mem::replace(&mut screen, Screen::Decks);
                    if let Screen::Menu { previous, .. } = old {
                        screen = *previous;
                    }
                }
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
                Action::Help | Action::Stats => {
                    let (title, text) = match action {
                        Action::Help => (tr("Help"), tr("Decks: UP/DOWN selects. A studies. LEFT/RIGHT expands or collapses.\nReview: A reveals. Y Again; X Hard; A Good; R Easy.\nScroll the top screen with Circle Pad, D-pad, C-stick or ZL/ZR.\nSELECT opens Menu. B returns. START closes and exits.\nMenu > Font scale changes card text size.\nAdd, Edit and Browse manage notes. Save confirms changes; Cancel discards them.").into()),
                        Action::Stats => (tr("Statistics"), backend.as_mut().ok_or("collection unavailable")?.statistics()?),
                        _ => unreachable!(),
                    };
                    let previous = std::mem::replace(&mut screen, Screen::Decks);
                    screen = Screen::Info {
                        title: title.into(),
                        text,
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
                    if let Screen::Browser(b) = &screen {
                        if let Some(query) = Ui::keyboard("Buscar tarjetas", &b.query)
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
                Action::Options => {
                    screen = Screen::Options {
                        options: backend
                            .as_mut()
                            .ok_or("collection unavailable")?
                            .options(did)?,
                        field: 0,
                    };
                    card_scroll = 0;
                }
                Action::Undo => {
                    backend.as_mut().ok_or("collection unavailable")?.undo()?;
                    event("UNDO_OK");
                    screen = Screen::Decks;
                }
                Action::Back => {
                    let old = std::mem::replace(&mut screen, Screen::Decks);
                    screen = match old {
                        Screen::Info { previous, .. } | Screen::Menu { previous, .. } => *previous,
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
                            screen = match backend
                                .as_mut()
                                .ok_or("collection unavailable")?
                                .select(did)?
                            {
                                Some(card) => Screen::Question { card, deck_name },
                                None => Screen::Empty { deck_name },
                            };
                            card_scroll = 0;
                        }
                        Screen::Question { .. } => {
                            let old = std::mem::replace(&mut screen, Screen::Decks);
                            if let Screen::Question { card, deck_name } = old {
                                screen = Screen::Answer { card, deck_name };
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
                                let id = b.answer(card, rating)?;
                                event(&format!("REVIEW card={id} rating={rating}"));
                                screen = match b.next()? {
                                    Some(card) => Screen::Question { card, deck_name },
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
                            let initial = if *field == 0 {
                                options.new
                            } else {
                                options.reviews
                            };
                            if let Some(value) =
                                Ui::keyboard(tr("Daily limit (0-9999)"), &initial.to_string())
                                    .map_err(|e| -> Box<dyn std::error::Error> { e.into() })?
                            {
                                let value: u32 = value.parse()?;
                                if value > 9999 {
                                    return Err(tr("Maximum limit: 9999").into());
                                }
                                if *field == 0 {
                                    options.new = value
                                } else {
                                    options.reviews = value
                                };
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
                        let step = 19.0 * i18n::font_scale_factor();
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
                            Screen::Options { field, .. } => {
                                *field = next(*field, 2);
                            }
                            Screen::Menu { selected, .. } => {
                                *selected = next(*selected, MENU.len());
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
                Action::None | Action::Menu | Action::Exit => {}
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

        if matches!(screen, Screen::Empty { .. })
            && last_empty_poll.elapsed() >= Duration::from_secs(1)
        {
            last_empty_poll = Instant::now();
            if let Screen::Empty { deck_name } = &screen {
                let name = deck_name.clone();
                if let Some(b) = backend.as_mut() {
                    match b.next() {
                        Ok(Some(card)) => {
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
        if review_card != last_review_card {
            last_review_card = review_card;
            review_scroll = 0.0;
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
            Screen::Info { .. } => "INFO",
            Screen::Error(_) => "ERROR",
        };
        if state != last_state {
            if state == "DECKS" {
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

        Ui::frame_begin();
        Ui::target_top(COLOR_BG_TOP);
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
                            COLOR_COUNT_NEW
                        } else {
                            COLOR_PANEL_BORDER
                        },
                    );
                }
            }
            Screen::SyncChoice {
                upload_ok,
                download_ok,
                download,
                confirmed,
            } => {
                header(400.0, tr("Full sync"));
                let selected = if *download {
                    tr("Download (replaces local)")
                } else {
                    tr("Upload (replaces remote)")
                };
                let instruction = if *confirmed {
                    tr("Press A to confirm replacement.")
                } else {
                    tr("UP/DOWN or touch selects. A continues. B cancels.")
                };
                let text=i18n::format("Full sync replaces one collection.\nUpload allowed: {0}\nDownload allowed: {1}\nSelected: {2}\n{3}",&[
                    tr(if *upload_ok {"Yes"} else {"No"}),tr(if *download_ok {"Yes"} else {"No"}),
                    selected,
                    instruction,
                ]);
                top_layout.draw(&text, 360.0, 200.0, 60.0, 8, 0);
            }
            Screen::Decks => {
                Ui::draw_text_centered(200.0, 32.0, 0.65, COLOR_TEXT_DARK, tr("Decks"));
                Ui::draw_line(20.0, 60.0, 380.0, 60.0, 1.0, COLOR_PANEL_BORDER);
                if let Some(deck) = decks.get(selected) {
                    Ui::fit_text(200.0, 86.0, 0.68, 360.0, COLOR_TEXT_DARK, &deck.leaf_name);
                    Ui::fit_text(200.0, 116.0, 0.44, 360.0, COLOR_TEXT_MUTED, &deck.name);
                    counts(deck.new_count, deck.learn_count, deck.review_count, 153.0);
                    Ui::draw_text_centered(
                        200.0,
                        210.0,
                        0.43,
                        COLOR_TEXT_MUTED,
                        tr("A Study    SELECT Menu"),
                    );
                } else {
                    Ui::draw_text_centered(200.0, 100.0, 0.60, COLOR_TEXT_DARK, tr("No decks"));
                }
            }
            Screen::Question { card, .. } | Screen::Answer { card, .. } => {
                let html = if matches!(screen, Screen::Answer { .. }) {
                    card.cloze_answer_html
                        .as_deref()
                        .unwrap_or(&card.answer_html)
                } else {
                    &card.question_html
                };
                top_card.set_text(html, CARD_WIDTH, i18n::font_scale_factor());
                review_max_scroll = top_card.max_scroll();
                review_scroll = review_scroll.min(review_max_scroll);
                top_card.draw(review_scroll);
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
                Ui::draw_text_centered(200.0, 30.0, 0.60, COLOR_TEXT_DARK, tr("Preview"));
                let text = format!("{}\n\n{}", b.preview.0, b.preview.1);
                top_max = top_layout.draw(&text, 360.0, 200.0, 62.0, 7, card_scroll);
            }
            Screen::Editor { draft, field, .. } => {
                let (label, value) = if *field < draft.labels.len() {
                    (&draft.labels[*field], draft.note.fields()[*field].clone())
                } else {
                    (&String::from(tr("Tags")), draft.note.tags.join(" "))
                };
                Ui::fit_text(200.0, 30.0, 0.60, 360.0, COLOR_TEXT_DARK, label);
                top_max = top_layout.draw(&value, 360.0, 200.0, 64.0, 7, card_scroll);
            }
            Screen::Options { options, .. } => {
                Ui::draw_text_centered(200.0, 35.0, 0.60, COLOR_TEXT_DARK, tr("Deck options"));
                top_max = top_layout.draw(tr("Daily limits for this deck.\nThe shared preset stays unchanged.\nSave applies changes. Cancel keeps previous limits."), 360.0, 200.0, 70.0, 7, 0);
                Ui::draw_text_centered(
                    200.0,
                    217.0,
                    0.42,
                    COLOR_TEXT_MUTED,
                    if options.snapshot.fsrs {
                        tr("FSRS enabled")
                    } else {
                        tr("FSRS disabled")
                    },
                );
            }
            Screen::Menu { .. } => {
                Ui::draw_text_centered(200.0, 70.0, 0.65, COLOR_TEXT_DARK, tr("Menu"));
                Ui::draw_text_centered(
                    200.0,
                    112.0,
                    0.48,
                    COLOR_TEXT_MUTED,
                    tr("Select an action. B returns."),
                );
            }
            Screen::Info { title, text, .. } => {
                Ui::fit_text(200.0, 30.0, 0.60, 360.0, COLOR_TEXT_DARK, title);
                top_max = top_layout.draw(text, 360.0, 200.0, 64.0, 7, card_scroll);
            }
            Screen::Empty { deck_name } => {
                Ui::draw_text_centered(
                    200.0,
                    65.0,
                    0.58,
                    COLOR_TEXT_DARK,
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
            Screen::Error(err) => {
                Ui::draw_text_centered(200.0, 30.0, 0.60, COLOR_COUNT_LEARN, tr("Error"));
                top_max = top_layout.draw(err, 360.0, 200.0, 62.0, 7, card_scroll);
            }
        }
        Ui::target_bottom(COLOR_BG_BOTTOM);
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
                for (i, label) in [tr("Decks"), tr("Add"), tr("Edit"), tr("Browse")]
                    .iter()
                    .enumerate()
                {
                    Ui::draw_nav_tab(i as f32 * 80.0, 0.0, 80.0, 42.0, label, i as i32, i == 0);
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
                button(168.0, 194.0, 140.0, tr("Sync"), None, 6);
            }
            Screen::Question { card, deck_name } => {
                Ui::draw_text(
                    8.0,
                    3.0,
                    0.44,
                    COLOR_TEXT_MUTED,
                    &Ui::ellipsize(deck_name, 0.44, 205.0),
                );
                Ui::draw_status_compact(314.0, 3.0, 0.42);
                counts_compact(card.new_count, card.learn_count, card.review_count, 48.0);
                Ui::draw_button(40.0, 128.0, 240.0, 52.0, tr("Show answer"), None, 0, false);
                Ui::draw_text_centered(
                    160.0,
                    187.0,
                    0.40,
                    COLOR_TEXT_MUTED,
                    tr("Tap or press [A]"),
                );
            }
            Screen::Answer { card, deck_name } => {
                Ui::draw_text(
                    8.0,
                    3.0,
                    0.44,
                    COLOR_TEXT_MUTED,
                    &Ui::ellipsize(deck_name, 0.44, 205.0),
                );
                Ui::draw_status_compact(314.0, 3.0, 0.42);
                counts_compact(card.new_count, card.learn_count, card.review_count, 48.0);
                for (i, label) in [tr("Again"), tr("Hard"), tr("Good"), tr("Easy")]
                    .iter()
                    .enumerate()
                {
                    let interval = card.intervals.get(i).map(String::as_str).unwrap_or("?");
                    let hint = format!("{interval} [{}]", ["Y", "X", "A", "R"][i]);
                    let hint = Ui::ellipsize(&hint, 0.42, 68.0);
                    button(
                        8.0 + i as f32 * 76.0,
                        190.0,
                        72.0,
                        label,
                        Some(&hint),
                        i as i32 + 2,
                    );
                }
            }
            Screen::Browser(b) => {
                header(
                    320.0,
                    &format!(
                        "{}: {}",
                        tr("Search"),
                        if b.query.is_empty() {
                            tr("All")
                        } else {
                            &b.query
                        }
                    ),
                );
                for (i, title) in b.titles.iter().enumerate() {
                    row(
                        43.0 + i as f32 * 24.0,
                        title,
                        b.page * ROWS + i == b.selected,
                    );
                }
                Ui::draw_scrollbar(
                    304.0,
                    43.0,
                    16.0,
                    144.0,
                    b.ids.len(),
                    ROWS,
                    b.page.saturating_mul(ROWS),
                );
                button(12.0, 194.0, 140.0, tr("Edit note"), None, 0);
                button(168.0, 194.0, 140.0, tr("Search"), None, 0);
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
                header(320.0, tr("Daily limits"));
                row(
                    43.0,
                    &format!("{}: {}", tr("New"), options.new),
                    *field == 0,
                );
                row(
                    67.0,
                    &format!("{}: {}", tr("Review"), options.reviews),
                    *field == 1,
                );
                button(12.0, 194.0, 140.0, tr("Save [X]"), None, 0);
                button(168.0, 194.0, 140.0, tr("Cancel [B]"), None, 6);
            }
            Screen::Menu { selected, .. } => {
                header(320.0, tr("Menu"));
                for (i, label) in MENU
                    .iter()
                    .enumerate()
                    .skip(selected / ROWS * ROWS)
                    .take(ROWS)
                {
                    let label = if i == FONT_SCALE_MENU_INDEX {
                        i18n::format("Font scale: {0}%", &[&i18n::font_scale().to_string()])
                    } else {
                        tr(label).to_owned()
                    };
                    row(43.0 + (i % ROWS) as f32 * 24.0, &label, i == *selected);
                }
                Ui::draw_scrollbar(
                    304.0,
                    43.0,
                    16.0,
                    144.0,
                    MENU.len(),
                    ROWS,
                    selected / ROWS * ROWS,
                );
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
                Ui::draw_text_centered(160.0, 90.0, 0.50, COLOR_TEXT_DARK, tr("Retry [A]"));
                Ui::draw_text_centered(160.0, 120.0, 0.50, COLOR_TEXT_DARK, tr("START: Exit"));
            }
        }
        if !matches!(
            screen,
            Screen::Question { .. } | Screen::Answer { .. } | Screen::SyncChoice { .. }
        ) {
            footer(if clock::is_known() {
                tr("Menu [SELECT]                      Options")
            } else {
                tr("Clock will update when connected")
            });
        }
        Ui::frame_end();
    }
    if let Some(active) = job.take() {
        if let Some(done) = active.finish() {
            backend = done.backend;
        }
    }
    drop(clock_job);
    Ui::fini();
    if let Some(backend) = backend {
        match backend.close() {
            Ok(()) => event("CLOSE_OK"),
            Err(e) => event(&format!("CLOSE_ERROR {}", error_text(e.as_ref()))),
        }
    }
}
