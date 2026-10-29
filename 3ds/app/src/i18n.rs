// Copyright: Anki 3DS contributors
// License: GNU AGPL, version 3 or later; http://www.gnu.org/licenses/agpl.html

use std::collections::HashMap;
use std::fs;
use std::io;
use std::sync::atomic::AtomicBool;
use std::sync::atomic::AtomicU16;
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
    load_download_connections();
    load_battery_linear();
    load_console_leds();
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
