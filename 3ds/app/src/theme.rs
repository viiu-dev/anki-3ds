// Copyright: Anki 3DS contributors
// License: GNU AGPL, version 3 or later; http://www.gnu.org/licenses/agpl.html

use std::fs;
use std::io;
use std::sync::atomic::AtomicUsize;
use std::sync::atomic::Ordering;

const SETTINGS_PATH: &str = "sdmc:/3ds/anki-3ds/settings.json";

#[inline]
const fn c(r: u8, g: u8, b: u8) -> u32 {
    (r as u32) | ((g as u32) << 8) | ((b as u32) << 16) | (255u32 << 24)
}

#[inline]
const fn ca(r: u8, g: u8, b: u8, a: u8) -> u32 {
    (r as u32) | ((g as u32) << 8) | ((b as u32) << 16) | ((a as u32) << 24)
}

#[repr(C)]
#[derive(Clone, Copy, PartialEq, Eq)]
pub struct Theme {
    pub bg_top: u32,
    pub bg_bottom: u32,
    pub surface: u32,
    pub surface_alt: u32,
    pub surface_pressed: u32,
    pub border: u32,
    pub text: u32,
    pub text_muted: u32,
    pub text_faint: u32,
    pub accent: u32,
    pub on_accent: u32,
    pub new: u32,
    pub learn: u32,
    pub review: u32,
    pub cloze_bg: u32,
    pub pitch: u32,
    pub row_selected: u32,
    pub tab_active_bg: u32,
    pub tab_active_fg: u32,
    pub tab_inactive_bg: u32,
    pub tab_inactive_fg: u32,
    pub scroll_track: u32,
    pub scroll_thumb: u32,
    pub again_bg: u32,
    pub again_fg: u32,
    pub hard_bg: u32,
    pub hard_fg: u32,
    pub good_bg: u32,
    pub good_fg: u32,
    pub easy_bg: u32,
    pub easy_fg: u32,
    pub shadow: u32,
    pub diff_good: u32,
    pub diff_bad: u32,
    pub diff_missed: u32,
}

const LIGHT: Theme = Theme {
    bg_top: c(247, 247, 245),
    bg_bottom: c(238, 241, 243),
    surface: c(255, 255, 255),
    surface_alt: c(243, 244, 246),
    surface_pressed: c(226, 229, 233),
    border: c(226, 227, 224),
    text: c(30, 32, 36),
    text_muted: c(107, 114, 128),
    text_faint: c(156, 163, 175),
    accent: c(37, 99, 235),
    on_accent: c(255, 255, 255),
    new: c(20, 100, 200),
    learn: c(210, 60, 40),
    review: c(36, 130, 56),
    cloze_bg: c(255, 243, 166),
    pitch: c(30, 32, 36),
    row_selected: c(226, 238, 250),
    tab_active_bg: c(219, 234, 254),
    tab_active_fg: c(37, 99, 235),
    tab_inactive_bg: c(243, 244, 246),
    tab_inactive_fg: c(107, 114, 128),
    scroll_track: ca(120, 126, 134, 36),
    scroll_thumb: ca(120, 126, 134, 130),
    again_bg: c(252, 232, 230),
    again_fg: c(197, 40, 41),
    hard_bg: c(254, 243, 224),
    hard_fg: c(176, 110, 0),
    good_bg: c(230, 244, 234),
    good_fg: c(30, 142, 62),
    easy_bg: c(232, 240, 254),
    easy_fg: c(26, 115, 232),
    shadow: ca(0, 0, 0, 26),
    diff_good: c(46, 125, 50),
    diff_bad: c(198, 40, 40),
    diff_missed: c(120, 120, 120),
};

const CREAM: Theme = Theme {
    bg_top: c(250, 246, 237),
    bg_bottom: c(244, 238, 224),
    surface: c(255, 252, 246),
    surface_alt: c(246, 240, 227),
    surface_pressed: c(233, 225, 208),
    border: c(230, 221, 203),
    text: c(58, 50, 38),
    text_muted: c(122, 110, 90),
    text_faint: c(169, 156, 133),
    accent: c(165, 106, 43),
    on_accent: c(255, 255, 255),
    new: c(30, 111, 184),
    learn: c(192, 86, 60),
    review: c(46, 139, 87),
    cloze_bg: c(255, 239, 178),
    pitch: c(58, 50, 38),
    row_selected: c(240, 229, 206),
    tab_active_bg: c(238, 224, 197),
    tab_active_fg: c(165, 106, 43),
    tab_inactive_bg: c(246, 240, 227),
    tab_inactive_fg: c(122, 110, 90),
    scroll_track: ca(120, 100, 70, 36),
    scroll_thumb: ca(120, 100, 70, 130),
    again_bg: c(249, 230, 224),
    again_fg: c(183, 58, 44),
    hard_bg: c(250, 238, 214),
    hard_fg: c(163, 101, 10),
    good_bg: c(230, 240, 226),
    good_fg: c(43, 128, 73),
    easy_bg: c(228, 236, 246),
    easy_fg: c(38, 101, 166),
    shadow: ca(60, 40, 20, 26),
    diff_good: c(56, 130, 60),
    diff_bad: c(190, 60, 50),
    diff_missed: c(130, 120, 105),
};

const DARK: Theme = Theme {
    bg_top: c(30, 31, 34),
    bg_bottom: c(22, 23, 26),
    surface: c(38, 40, 44),
    surface_alt: c(46, 48, 53),
    surface_pressed: c(58, 60, 66),
    border: c(58, 61, 66),
    text: c(232, 234, 237),
    text_muted: c(160, 165, 173),
    text_faint: c(110, 114, 120),
    accent: c(138, 180, 248),
    on_accent: c(16, 19, 24),
    new: c(138, 180, 248),
    learn: c(242, 139, 130),
    review: c(129, 201, 149),
    cloze_bg: c(74, 67, 32),
    pitch: c(232, 234, 237),
    row_selected: c(51, 54, 59),
    tab_active_bg: c(43, 53, 70),
    tab_active_fg: c(138, 180, 248),
    tab_inactive_bg: c(46, 48, 53),
    tab_inactive_fg: c(160, 165, 173),
    scroll_track: ca(255, 255, 255, 22),
    scroll_thumb: ca(255, 255, 255, 88),
    again_bg: c(58, 38, 36),
    again_fg: c(242, 139, 130),
    hard_bg: c(58, 48, 32),
    hard_fg: c(253, 214, 99),
    good_bg: c(35, 51, 38),
    good_fg: c(129, 201, 149),
    easy_bg: c(32, 48, 63),
    easy_fg: c(138, 180, 248),
    shadow: ca(0, 0, 0, 70),
    diff_good: c(76, 175, 80),
    diff_bad: c(229, 115, 115),
    diff_missed: c(160, 165, 173),
};

const BLACK: Theme = Theme {
    bg_top: c(0, 0, 0),
    bg_bottom: c(0, 0, 0),
    surface: c(14, 14, 14),
    surface_alt: c(21, 21, 21),
    surface_pressed: c(33, 33, 33),
    border: c(38, 38, 38),
    text: c(237, 237, 237),
    text_muted: c(154, 160, 166),
    text_faint: c(95, 99, 104),
    accent: c(138, 180, 248),
    on_accent: c(0, 0, 0),
    new: c(138, 180, 248),
    learn: c(242, 139, 130),
    review: c(129, 201, 149),
    cloze_bg: c(58, 52, 22),
    pitch: c(237, 237, 237),
    row_selected: c(28, 28, 28),
    tab_active_bg: c(26, 33, 45),
    tab_active_fg: c(138, 180, 248),
    tab_inactive_bg: c(21, 21, 21),
    tab_inactive_fg: c(154, 160, 166),
    scroll_track: ca(255, 255, 255, 20),
    scroll_thumb: ca(255, 255, 255, 78),
    again_bg: c(42, 26, 24),
    again_fg: c(242, 139, 130),
    hard_bg: c(42, 36, 22),
    hard_fg: c(253, 214, 99),
    good_bg: c(23, 37, 26),
    good_fg: c(129, 201, 149),
    easy_bg: c(21, 35, 47),
    easy_fg: c(138, 180, 248),
    shadow: ca(0, 0, 0, 130),
    diff_good: c(76, 175, 80),
    diff_bad: c(229, 115, 115),
    diff_missed: c(154, 160, 166),
};

static THEMES: [Theme; 4] = [LIGHT, CREAM, DARK, BLACK];
static INDEX: AtomicUsize = AtomicUsize::new(0);

const _: () = assert!(std::mem::size_of::<Theme>() == 35 * 4);

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum ThemeKind {
    Light,
    Cream,
    Dark,
    Black,
}

impl ThemeKind {
    pub const ALL: [ThemeKind; 4] = [
        ThemeKind::Light,
        ThemeKind::Cream,
        ThemeKind::Dark,
        ThemeKind::Black,
    ];

    fn index(self) -> usize {
        match self {
            ThemeKind::Light => 0,
            ThemeKind::Cream => 1,
            ThemeKind::Dark => 2,
            ThemeKind::Black => 3,
        }
    }

    fn from_index(index: usize) -> ThemeKind {
        ThemeKind::ALL[index.min(ThemeKind::ALL.len() - 1)]
    }

    pub fn key(self) -> &'static str {
        match self {
            ThemeKind::Light => "light",
            ThemeKind::Cream => "cream",
            ThemeKind::Dark => "dark",
            ThemeKind::Black => "black",
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            ThemeKind::Light => "Light",
            ThemeKind::Cream => "Cream",
            ThemeKind::Dark => "Dark",
            ThemeKind::Black => "Black",
        }
    }
}

pub fn current() -> &'static Theme {
    &THEMES[INDEX.load(Ordering::Relaxed)]
}

pub fn kind() -> ThemeKind {
    ThemeKind::from_index(INDEX.load(Ordering::Relaxed))
}

pub fn init() {
    let Ok(text) = fs::read_to_string(SETTINGS_PATH) else {
        return;
    };
    let key = serde_json::from_str::<serde_json::Value>(&text)
        .ok()
        .and_then(|value| {
            value
                .get("theme")
                .and_then(|v| v.as_str())
                .map(str::to_owned)
        });
    if let Some(key) = key {
        if let Some(found) = ThemeKind::ALL.iter().find(|theme| theme.key() == key) {
            INDEX.store(found.index(), Ordering::Relaxed);
        }
    }
}

pub fn set(kind: ThemeKind) -> io::Result<()> {
    save(kind)?;
    INDEX.store(kind.index(), Ordering::Relaxed);
    Ok(())
}


fn save(kind: ThemeKind) -> io::Result<()> {
    let mut document = fs::read_to_string(SETTINGS_PATH)
        .ok()
        .and_then(|text| serde_json::from_str::<serde_json::Value>(&text).ok())
        .unwrap_or_else(|| serde_json::json!({}));
    if !document.is_object() {
        document = serde_json::json!({});
    }
    document["theme"] = serde_json::json!(kind.key());
    let text = serde_json::to_string_pretty(&document).map_err(io::Error::other)? + "\n";
    if let Some(parent) = std::path::Path::new(SETTINGS_PATH).parent() {
        fs::create_dir_all(parent)?;
    }
    let temporary = format!("{SETTINGS_PATH}.tmp");
    fs::write(&temporary, text.as_bytes())?;
    match fs::rename(&temporary, SETTINGS_PATH) {
        Ok(()) => Ok(()),
        Err(_) => {
            let _ = fs::remove_file(&temporary);
            fs::write(SETTINGS_PATH, text.as_bytes())
        }
    }
}
