// Copyright: Anki 3DS contributors
// License: GNU AGPL, version 3 or later; http://www.gnu.org/licenses/agpl.html

use std::collections::HashMap;
use std::fs;
use std::io;
use std::sync::atomic::AtomicBool;
use std::sync::atomic::AtomicU16;
use std::sync::atomic::AtomicU32;
use std::sync::atomic::AtomicU8;
use std::sync::atomic::Ordering;
use std::sync::OnceLock;

use ctru::services::cfgu::Cfgu;
use ctru::services::cfgu::Language;

pub const LANGUAGES: [(&str, &str); 9] = [
    ("en", "English"),
    ("es", "Español"),
    ("fr", "Français"),
    ("de", "Deutsch"),
    ("it", "Italiano"),
    ("pt", "Português"),
    ("ru", "Русский"),
    ("uk", "Українська"),
    ("ja", "日本語"),
];

const CATALOGS: [&str; 9] = [
    include_str!("../locales/en.json"),
    include_str!("../locales/es.json"),
    include_str!("../locales/fr.json"),
    include_str!("../locales/de.json"),
    include_str!("../locales/it.json"),
    include_str!("../locales/pt.json"),
    include_str!("../locales/ru.json"),
    include_str!("../locales/uk.json"),
    include_str!("../locales/ja.json"),
];

static CATALOG_CACHE: [OnceLock<HashMap<String, String>>; 9] = [const { OnceLock::new() }; 9];
static LANGUAGE: AtomicU8 = AtomicU8::new(0);

const PATH: &str = "sdmc:/3ds/anki-3ds/language.txt";
const SETTINGS_PATH: &str = "sdmc:/3ds/anki-3ds/settings.json";
pub const FONT_SCALES: [u16; 7] = [85, 100, 125, 150, 175, 200, 250];
const DEFAULT_FONT_SCALE: u16 = 100;
static FONT_SCALE: AtomicU16 = AtomicU16::new(DEFAULT_FONT_SCALE);

pub fn init() {
    load_font_scale();
    load_autoplay_audio();
    load_autoplay_video();
    load_download_connections();
    load_battery_linear();
    load_console_leds();
    load_rating_icon();
    load_card_counts();
    load_rating_intervals();
    load_deck_title();
    load_audio_speed();
    load_replay_mode();
    load_scroll_speed();
    load_spacing();
    load_toolbar();
    load_quickstart_done();
    let saved = fs::read_to_string(PATH)
        .ok()
        .and_then(|text| index_of(text.trim()));
    let index = saved.unwrap_or_else(|| index_of(system_language()).unwrap_or(0));
    LANGUAGE.store(index as u8, Ordering::Relaxed);
    let _ = catalog();
}

fn system_language() -> &'static str {
    match Cfgu::new().ok().and_then(|config| config.language().ok()) {
        Some(Language::Japanese) => "ja",
        Some(Language::French) => "fr",
        Some(Language::German) => "de",
        Some(Language::Italian) => "it",
        Some(Language::Spanish) => "es",
        Some(Language::Portuguese) => "pt",
        Some(Language::Russian) => "ru",
        _ => "en",
    }
}

fn index_of(code: &str) -> Option<usize> {
    LANGUAGES.iter().position(|(value, _)| *value == code)
}

fn catalog() -> Option<&'static HashMap<String, String>> {
    let index = usize::from(LANGUAGE.load(Ordering::Relaxed)).min(LANGUAGES.len() - 1);
    Some(
        CATALOG_CACHE[index]
            .get_or_init(|| serde_json::from_str(CATALOGS[index]).expect("locale catalog")),
    )
}

pub fn language() -> &'static str {
    LANGUAGES[language_index()].0
}

pub fn language_index() -> usize {
    usize::from(LANGUAGE.load(Ordering::Relaxed)).min(LANGUAGES.len() - 1)
}

pub fn set_language(index: usize) -> io::Result<()> {
    let index = index.min(LANGUAGES.len() - 1);
    fs::write(PATH, LANGUAGES[index].0)?;
    LANGUAGE.store(index as u8, Ordering::Relaxed);
    Ok(())
}

pub fn cycle_language(direction: i32) -> io::Result<usize> {
    let count = LANGUAGES.len();
    let index = language_index();
    let next = if direction < 0 {
        (index + count - 1) % count
    } else {
        (index + 1) % count
    };
    set_language(next)?;
    Ok(next)
}

pub fn toggle() -> std::io::Result<()> {
    cycle_language(1).map(|_| ())
}

fn load_font_scale() {
    let Ok(text) = fs::read_to_string(SETTINGS_PATH) else {
        return;
    };
    let Some(percent) = serde_json::from_str::<serde_json::Value>(&text)
        .ok()
        .and_then(|value| value.get("font_scale").and_then(|v| v.as_u64()))
        .and_then(|value| u16::try_from(value).ok())
    else {
        return;
    };
    if FONT_SCALES.contains(&percent) {
        FONT_SCALE.store(percent, Ordering::Relaxed);
    }
}

/// Card font size as a percentage (85, 100, 125 or 150).
pub fn font_scale() -> u16 {
    FONT_SCALE.load(Ordering::Relaxed)
}

pub fn font_scale_factor() -> f32 {
    f32::from(font_scale()) / 100.0
}

pub fn set_font_scale(percent: u16) -> io::Result<u16> {
    if !FONT_SCALES.contains(&percent) {
        return Ok(font_scale());
    }
    save_font_scale(percent)?;
    FONT_SCALE.store(percent, Ordering::Relaxed);
    Ok(percent)
}

fn save_font_scale(percent: u16) -> io::Result<()> {
    write_setting("font_scale", serde_json::json!(percent))
}

fn write_setting(key: &str, value: serde_json::Value) -> io::Result<()> {
    let mut document = fs::read_to_string(SETTINGS_PATH)
        .ok()
        .and_then(|text| serde_json::from_str::<serde_json::Value>(&text).ok())
        .unwrap_or_else(|| serde_json::json!({}));
    if !document.is_object() {
        document = serde_json::json!({});
    }
    document[key] = value;
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

static AUTOPLAY_AUDIO: AtomicBool = AtomicBool::new(false);

pub fn autoplay_audio() -> bool {
    AUTOPLAY_AUDIO.load(Ordering::Relaxed)
}

pub fn toggle_autoplay_audio() -> io::Result<bool> {
    let value = !autoplay_audio();
    write_setting("autoplay_audio", serde_json::json!(value))?;
    AUTOPLAY_AUDIO.store(value, Ordering::Relaxed);
    Ok(value)
}

fn load_autoplay_audio() {
    let Ok(text) = fs::read_to_string(SETTINGS_PATH) else {
        return;
    };
    let value = serde_json::from_str::<serde_json::Value>(&text)
        .ok()
        .and_then(|value| value.get("autoplay_audio").and_then(|v| v.as_bool()));
    if let Some(value) = value {
        AUTOPLAY_AUDIO.store(value, Ordering::Relaxed);
    }
}

static AUTOPLAY_VIDEO: AtomicBool = AtomicBool::new(false);

/// Whether card video autoplays. Unlike audio, video only starts on its own
/// with this setting (or an explicit autoplay attribute).
pub fn autoplay_video() -> bool {
    AUTOPLAY_VIDEO.load(Ordering::Relaxed)
}

pub fn toggle_autoplay_video() -> io::Result<bool> {
    let value = !autoplay_video();
    write_setting("autoplay_video", serde_json::json!(value))?;
    AUTOPLAY_VIDEO.store(value, Ordering::Relaxed);
    Ok(value)
}

fn load_autoplay_video() {
    let Ok(text) = fs::read_to_string(SETTINGS_PATH) else {
        return;
    };
    let value = serde_json::from_str::<serde_json::Value>(&text)
        .ok()
        .and_then(|value| value.get("autoplay_video").and_then(|v| v.as_bool()));
    if let Some(value) = value {
        AUTOPLAY_VIDEO.store(value, Ordering::Relaxed);
    }
}

static BATTERY_LINEAR: AtomicBool = AtomicBool::new(true);

pub fn battery_linear() -> bool {
    BATTERY_LINEAR.load(Ordering::Relaxed)
}

pub fn set_battery_linear(value: bool) -> io::Result<()> {
    write_setting("battery_linear", serde_json::json!(value))?;
    BATTERY_LINEAR.store(value, Ordering::Relaxed);
    Ok(())
}

fn load_battery_linear() {
    let Ok(text) = fs::read_to_string(SETTINGS_PATH) else {
        return;
    };
    let value = serde_json::from_str::<serde_json::Value>(&text)
        .ok()
        .and_then(|value| value.get("battery_linear").and_then(|v| v.as_bool()));
    if let Some(value) = value {
        BATTERY_LINEAR.store(value, Ordering::Relaxed);
    }
}

static CONSOLE_LEDS: AtomicBool = AtomicBool::new(true);

pub fn console_leds() -> bool {
    CONSOLE_LEDS.load(Ordering::Relaxed)
}

pub fn set_console_leds(value: bool) -> io::Result<()> {
    write_setting("console_leds", serde_json::json!(value))?;
    CONSOLE_LEDS.store(value, Ordering::Relaxed);
    Ok(())
}

pub fn toggle_console_leds() -> io::Result<bool> {
    let value = !console_leds();
    set_console_leds(value)?;
    Ok(value)
}

fn load_console_leds() {
    let Ok(text) = fs::read_to_string(SETTINGS_PATH) else {
        return;
    };
    let value = serde_json::from_str::<serde_json::Value>(&text)
        .ok()
        .and_then(|value| value.get("console_leds").and_then(|v| v.as_bool()));
    if let Some(value) = value {
        CONSOLE_LEDS.store(value, Ordering::Relaxed);
    }
}

fn load_bool_setting(key: &str, target: &AtomicBool) {
    let Ok(text) = fs::read_to_string(SETTINGS_PATH) else {
        return;
    };
    let value = serde_json::from_str::<serde_json::Value>(&text)
        .ok()
        .and_then(|value| value.get(key).and_then(|v| v.as_bool()));
    if let Some(value) = value {
        target.store(value, Ordering::Relaxed);
    }
}

static RATING_ICON: AtomicBool = AtomicBool::new(true);

pub fn rating_icon() -> bool {
    RATING_ICON.load(Ordering::Relaxed)
}

pub fn toggle_rating_icon() -> io::Result<bool> {
    let value = !rating_icon();
    write_setting("rating_icon", serde_json::json!(value))?;
    RATING_ICON.store(value, Ordering::Relaxed);
    Ok(value)
}

fn load_rating_icon() {
    load_bool_setting("rating_icon", &RATING_ICON);
}

static CARD_COUNTS: AtomicBool = AtomicBool::new(true);

pub fn card_counts() -> bool {
    CARD_COUNTS.load(Ordering::Relaxed)
}

pub fn toggle_card_counts() -> io::Result<bool> {
    let value = !card_counts();
    write_setting("card_counts", serde_json::json!(value))?;
    CARD_COUNTS.store(value, Ordering::Relaxed);
    Ok(value)
}

fn load_card_counts() {
    load_bool_setting("card_counts", &CARD_COUNTS);
}

static RATING_INTERVALS: AtomicBool = AtomicBool::new(true);

pub fn rating_intervals() -> bool {
    RATING_INTERVALS.load(Ordering::Relaxed)
}

pub fn toggle_rating_intervals() -> io::Result<bool> {
    let value = !rating_intervals();
    write_setting("rating_intervals", serde_json::json!(value))?;
    RATING_INTERVALS.store(value, Ordering::Relaxed);
    Ok(value)
}

fn load_rating_intervals() {
    load_bool_setting("rating_intervals", &RATING_INTERVALS);
}

pub const AUDIO_SPEEDS: [f32; 4] = [0.75, 1.0, 1.25, 1.5];
pub const AUDIO_SPEED_LABELS: [&str; 4] = ["0.75x", "1.0x", "1.25x", "1.5x"];
static AUDIO_SPEED: AtomicU8 = AtomicU8::new(1);

pub fn audio_speed_index() -> usize {
    usize::from(AUDIO_SPEED.load(Ordering::Relaxed)).min(AUDIO_SPEEDS.len() - 1)
}

pub fn audio_speed() -> f32 {
    AUDIO_SPEEDS[audio_speed_index()]
}

pub fn set_audio_speed_index(index: usize) -> io::Result<usize> {
    let index = index.min(AUDIO_SPEEDS.len() - 1);
    write_setting("audio_speed", serde_json::json!(index))?;
    AUDIO_SPEED.store(index as u8, Ordering::Relaxed);
    Ok(index)
}

fn load_audio_speed() {
    let Ok(text) = fs::read_to_string(SETTINGS_PATH) else {
        return;
    };
    let value = serde_json::from_str::<serde_json::Value>(&text)
        .ok()
        .and_then(|value| value.get("audio_speed").and_then(|v| v.as_u64()))
        .unwrap_or(1)
        .min(AUDIO_SPEEDS.len() as u64 - 1);
    AUDIO_SPEED.store(value as u8, Ordering::Relaxed);
}

static REPLAY_ALL_AUDIO: AtomicBool = AtomicBool::new(false);

pub fn replay_all_audio() -> bool {
    REPLAY_ALL_AUDIO.load(Ordering::Relaxed)
}

pub fn set_replay_all_audio(value: bool) -> io::Result<bool> {
    write_setting("replay_all_audio", serde_json::json!(value))?;
    REPLAY_ALL_AUDIO.store(value, Ordering::Relaxed);
    Ok(value)
}

fn load_replay_mode() {
    load_bool_setting("replay_all_audio", &REPLAY_ALL_AUDIO);
}

pub const SCROLL_SPEEDS: [f32; 5] = [0.6, 1.0, 1.6, 2.5, 4.0];
static SCROLL_SPEED: AtomicU8 = AtomicU8::new(1);

pub fn scroll_speed_index() -> usize {
    usize::from(SCROLL_SPEED.load(Ordering::Relaxed)).min(SCROLL_SPEEDS.len() - 1)
}

pub fn scroll_speed_factor() -> f32 {
    SCROLL_SPEEDS[scroll_speed_index()]
}

pub fn set_scroll_speed_index(index: usize) -> io::Result<usize> {
    let index = index.min(SCROLL_SPEEDS.len() - 1);
    write_setting("scroll_speed", serde_json::json!(index))?;
    SCROLL_SPEED.store(index as u8, Ordering::Relaxed);
    Ok(index)
}

fn load_scroll_speed() {
    let Ok(text) = fs::read_to_string(SETTINGS_PATH) else {
        return;
    };
    let value = serde_json::from_str::<serde_json::Value>(&text)
        .ok()
        .and_then(|value| value.get("scroll_speed").and_then(|v| v.as_u64()))
        .unwrap_or(1)
        .min(SCROLL_SPEEDS.len() as u64 - 1);
    SCROLL_SPEED.store(value as u8, Ordering::Relaxed);
}

pub const CARD_SPACING: [f32; 3] = [0.88, 1.0, 1.16];
static SPACING: AtomicU8 = AtomicU8::new(1);

pub fn spacing_index() -> usize {
    usize::from(SPACING.load(Ordering::Relaxed)).min(CARD_SPACING.len() - 1)
}

pub fn spacing_factor() -> f32 {
    CARD_SPACING[spacing_index()]
}

pub fn set_spacing_index(index: usize) -> io::Result<usize> {
    let index = index.min(CARD_SPACING.len() - 1);
    write_setting("card_spacing", serde_json::json!(index))?;
    SPACING.store(index as u8, Ordering::Relaxed);
    Ok(index)
}

fn load_spacing() {
    let Ok(text) = fs::read_to_string(SETTINGS_PATH) else {
        return;
    };
    let value = serde_json::from_str::<serde_json::Value>(&text)
        .ok()
        .and_then(|value| value.get("card_spacing").and_then(|v| v.as_u64()))
        .unwrap_or(1)
        .min(CARD_SPACING.len() as u64 - 1);
    SPACING.store(value as u8, Ordering::Relaxed);
}

static DECK_TITLE: AtomicBool = AtomicBool::new(true);

pub fn deck_title_visible() -> bool {
    DECK_TITLE.load(Ordering::Relaxed)
}

pub fn toggle_deck_title() -> io::Result<bool> {
    let value = !deck_title_visible();
    write_setting("deck_title_visible", serde_json::json!(value))?;
    DECK_TITLE.store(value, Ordering::Relaxed);
    Ok(value)
}

fn load_deck_title() {
    load_bool_setting("deck_title_visible", &DECK_TITLE);
}

#[repr(u8)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ToolbarItem {
    Sync = 0,
    Wifi = 1,
    Battery = 2,
    Time = 3,
}

impl ToolbarItem {
    const ALL: [ToolbarItem; 4] = [
        ToolbarItem::Sync,
        ToolbarItem::Wifi,
        ToolbarItem::Battery,
        ToolbarItem::Time,
    ];

    pub fn key(self) -> &'static str {
        match self {
            ToolbarItem::Sync => "sync",
            ToolbarItem::Wifi => "wifi",
            ToolbarItem::Battery => "battery",
            ToolbarItem::Time => "time",
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            ToolbarItem::Sync => "Sync icon",
            ToolbarItem::Wifi => "Wi-Fi",
            ToolbarItem::Battery => "Battery",
            ToolbarItem::Time => "Clock",
        }
    }

    fn from_key(key: &str) -> Option<Self> {
        match key {
            "sync" => Some(ToolbarItem::Sync),
            "wifi" => Some(ToolbarItem::Wifi),
            "battery" => Some(ToolbarItem::Battery),
            "time" => Some(ToolbarItem::Time),
            _ => None,
        }
    }

    fn from_index(index: u8) -> Self {
        match index {
            1 => ToolbarItem::Wifi,
            2 => ToolbarItem::Battery,
            3 => ToolbarItem::Time,
            _ => ToolbarItem::Sync,
        }
    }
}

const TOOLBAR_VISIBLE_BIT: u32 = 1;
const DEFAULT_TOOLBAR_STATE: u32 = 0x0000_E41F;
static TOOLBAR_STATE: AtomicU32 = AtomicU32::new(DEFAULT_TOOLBAR_STATE);

fn toolbar_item_bit(item: ToolbarItem) -> u32 {
    1 << (item as u32 + 1)
}

fn toolbar_order_shift(slot: usize) -> u32 {
    8 + slot as u32 * 2
}

fn toolbar_order_item(state: u32, slot: usize) -> ToolbarItem {
    ToolbarItem::from_index(((state >> toolbar_order_shift(slot)) & 0x3) as u8)
}

fn set_toolbar_order_item(state: u32, slot: usize, item: ToolbarItem) -> u32 {
    let shift = toolbar_order_shift(slot);
    (state & !(0x3 << shift)) | ((item as u32 & 0x3) << shift)
}

pub fn toolbar_visible() -> bool {
    TOOLBAR_STATE.load(Ordering::Relaxed) & TOOLBAR_VISIBLE_BIT != 0
}

pub fn toggle_toolbar_visible() -> io::Result<bool> {
    let value = !toolbar_visible();
    write_setting("toolbar_visible", serde_json::json!(value))?;
    let mut state = TOOLBAR_STATE.load(Ordering::Relaxed);
    if value {
        state |= TOOLBAR_VISIBLE_BIT;
    } else {
        state &= !TOOLBAR_VISIBLE_BIT;
    }
    TOOLBAR_STATE.store(state, Ordering::Relaxed);
    Ok(value)
}

pub fn toolbar_order() -> [ToolbarItem; 4] {
    let state = TOOLBAR_STATE.load(Ordering::Relaxed);
    [
        toolbar_order_item(state, 0),
        toolbar_order_item(state, 1),
        toolbar_order_item(state, 2),
        toolbar_order_item(state, 3),
    ]
}

pub fn toolbar_item_visible(item: ToolbarItem) -> bool {
    TOOLBAR_STATE.load(Ordering::Relaxed) & toolbar_item_bit(item) != 0
}

pub fn toggle_toolbar_item(item: ToolbarItem) -> io::Result<bool> {
    let value = !toolbar_item_visible(item);
    let mut state = TOOLBAR_STATE.load(Ordering::Relaxed);
    if value {
        state |= toolbar_item_bit(item);
    } else {
        state &= !toolbar_item_bit(item);
    }
    TOOLBAR_STATE.store(state, Ordering::Relaxed);
    save_toolbar_hidden()?;
    Ok(value)
}

pub fn move_toolbar_item(slot: usize, delta: i32) -> io::Result<usize> {
    let slot = slot.min(3);
    let target = (slot as i32 + delta).clamp(0, 3) as usize;
    if target == slot {
        return Ok(slot);
    }
    let mut order = toolbar_order();
    order.swap(slot, target);
    let keys: Vec<&str> = order.iter().map(|item| item.key()).collect();
    write_setting("toolbar_order", serde_json::json!(keys))?;
    let mut state = TOOLBAR_STATE.load(Ordering::Relaxed);
    for (index, item) in order.iter().enumerate() {
        state = set_toolbar_order_item(state, index, *item);
    }
    TOOLBAR_STATE.store(state, Ordering::Relaxed);
    Ok(target)
}

pub fn active_toolbar_items() -> ([u8; 4], usize) {
    if !toolbar_visible() {
        return ([0; 4], 0);
    }
    let mut items = [0u8; 4];
    let mut count = 0;
    for item in toolbar_order() {
        if toolbar_item_visible(item) {
            items[count] = item as u8;
            count += 1;
        }
    }
    (items, count)
}

fn save_toolbar_hidden() -> io::Result<()> {
    let hidden: Vec<&str> = ToolbarItem::ALL
        .iter()
        .filter(|item| !toolbar_item_visible(**item))
        .map(|item| item.key())
        .collect();
    write_setting("toolbar_hidden", serde_json::json!(hidden))
}

fn load_toolbar() {
    let mut state = DEFAULT_TOOLBAR_STATE;
    let Ok(text) = fs::read_to_string(SETTINGS_PATH) else {
        TOOLBAR_STATE.store(state, Ordering::Relaxed);
        return;
    };
    let Ok(document) = serde_json::from_str::<serde_json::Value>(&text) else {
        TOOLBAR_STATE.store(state, Ordering::Relaxed);
        return;
    };
    if let Some(value) = document
        .get("toolbar_visible")
        .and_then(|value| value.as_bool())
    {
        if value {
            state |= TOOLBAR_VISIBLE_BIT;
        } else {
            state &= !TOOLBAR_VISIBLE_BIT;
        }
    }
    if let Some(hidden) = document
        .get("toolbar_hidden")
        .and_then(|value| value.as_array())
    {
        for item in ToolbarItem::ALL {
            let is_hidden = hidden
                .iter()
                .any(|value| value.as_str() == Some(item.key()));
            if is_hidden {
                state &= !toolbar_item_bit(item);
            } else {
                state |= toolbar_item_bit(item);
            }
        }
    }
    if let Some(order) = document
        .get("toolbar_order")
        .and_then(|value| value.as_array())
    {
        let mut seen = [false; 4];
        let mut items = Vec::with_capacity(4);
        for value in order {
            if let Some(item) = value.as_str().and_then(ToolbarItem::from_key) {
                if !seen[item as usize] {
                    seen[item as usize] = true;
                    items.push(item);
                }
            }
        }
        for item in ToolbarItem::ALL {
            if !seen[item as usize] {
                items.push(item);
            }
        }
        for (slot, item) in items.into_iter().take(4).enumerate() {
            state = set_toolbar_order_item(state, slot, item);
        }
    }
    TOOLBAR_STATE.store(state, Ordering::Relaxed);
}

pub fn sync_stamp() -> i64 {
    let Ok(text) = fs::read_to_string(SETTINGS_PATH) else {
        return 0;
    };
    serde_json::from_str::<serde_json::Value>(&text)
        .ok()
        .and_then(|value| {
            value
                .get("sync_collection_modified")
                .and_then(|v| v.as_i64())
        })
        .unwrap_or(0)
}

pub fn save_sync_stamp(value: i64) -> io::Result<()> {
    write_setting("sync_collection_modified", serde_json::json!(value))
}

fn read_i64_setting(key: &str) -> Option<i64> {
    let text = fs::read_to_string(SETTINGS_PATH).ok()?;
    serde_json::from_str::<serde_json::Value>(&text)
        .ok()?
        .get(key)
        .and_then(|v| v.as_i64())
}

pub fn backup_stamp() -> i64 {
    read_i64_setting("backup_collection_modified").unwrap_or(0)
}

pub fn save_backup_stamp(value: i64) -> io::Result<()> {
    write_setting("backup_collection_modified", serde_json::json!(value))
}

pub fn backup_day() -> Option<i64> {
    read_i64_setting("backup_day")
}

pub fn save_backup_day(day: i64) -> io::Result<()> {
    write_setting("backup_day", serde_json::json!(day))
}

const MAX_DOWNLOAD_CONNECTIONS: u8 = 4;
static DOWNLOAD_CONNECTIONS: AtomicU8 = AtomicU8::new(1);

/// Number of parallel media download connections (1-4).
pub fn download_connections() -> usize {
    usize::from(
        DOWNLOAD_CONNECTIONS
            .load(Ordering::Relaxed)
            .clamp(1, MAX_DOWNLOAD_CONNECTIONS),
    )
}

pub fn set_download_connections(value: u8) -> io::Result<u8> {
    let value = value.clamp(1, MAX_DOWNLOAD_CONNECTIONS);
    write_setting("download_connections", serde_json::json!(value))?;
    DOWNLOAD_CONNECTIONS.store(value, Ordering::Relaxed);
    Ok(value)
}

fn load_download_connections() {
    let Ok(text) = fs::read_to_string(SETTINGS_PATH) else {
        return;
    };
    let value = serde_json::from_str::<serde_json::Value>(&text)
        .ok()
        .and_then(|value| value.get("download_connections").and_then(|v| v.as_u64()))
        .and_then(|value| u8::try_from(value).ok())
        .unwrap_or(1)
        .clamp(1, MAX_DOWNLOAD_CONNECTIONS);
    DOWNLOAD_CONNECTIONS.store(value, Ordering::Relaxed);
}

static QUICKSTART_DONE: AtomicBool = AtomicBool::new(false);

pub fn quickstart_done() -> bool {
    QUICKSTART_DONE.load(Ordering::Relaxed)
}

pub fn set_quickstart_done(value: bool) -> io::Result<()> {
    write_setting("quickstart_done", serde_json::json!(value))?;
    QUICKSTART_DONE.store(value, Ordering::Relaxed);
    Ok(())
}

fn load_quickstart_done() {
    load_bool_setting("quickstart_done", &QUICKSTART_DONE);
}

pub fn tr(message: &'static str) -> &'static str {
    catalog()
        .and_then(|catalog| catalog.get(message))
        .map(String::as_str)
        .unwrap_or(message)
}

pub fn format(message: &'static str, values: &[&str]) -> String {
    let mut text = tr(message).to_owned();
    for (index, value) in values.iter().enumerate() {
        text = text.replace(&format!("{{{index}}}"), value);
    }
    text
}
