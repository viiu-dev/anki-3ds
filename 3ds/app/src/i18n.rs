// Copyright: Anki 3DS contributors
// License: GNU AGPL, version 3 or later; http://www.gnu.org/licenses/agpl.html

use std::collections::HashMap;
use std::fs;
use std::io;
use std::sync::atomic::AtomicBool;
use std::sync::atomic::AtomicU16;
use std::sync::atomic::Ordering;
use std::sync::OnceLock;

use ctru::services::cfgu::Cfgu;
use ctru::services::cfgu::Language;
static SPANISH: AtomicBool = AtomicBool::new(false);
static EN: OnceLock<HashMap<String, String>> = OnceLock::new();
static ES: OnceLock<HashMap<String, String>> = OnceLock::new();
const PATH: &str = "sdmc:/3ds/anki-3ds/language.txt";

const SETTINGS_PATH: &str = "sdmc:/3ds/anki-3ds/settings.json";
pub const FONT_SCALES: [u16; 4] = [85, 100, 125, 150];
const DEFAULT_FONT_SCALE: u16 = 100;
static FONT_SCALE: AtomicU16 = AtomicU16::new(DEFAULT_FONT_SCALE);

pub fn init() {
    load_font_scale();
    let system_spanish =
        Cfgu::new().ok().and_then(|c| c.language().ok()) == Some(Language::Spanish);
    let spanish = match std::fs::read_to_string(PATH).ok().as_deref().map(str::trim) {
        Some("es") => true,
        Some("en") => false,
        _ => system_spanish,
    };
    SPANISH.store(spanish, Ordering::Relaxed);
    EN.get_or_init(|| {
        serde_json::from_str(include_str!("../locales/en.json")).expect("English catalog")
    });
    ES.get_or_init(|| {
        serde_json::from_str(include_str!("../locales/es.json")).expect("Spanish catalog")
    });
}
pub fn language() -> &'static str {
    if SPANISH.load(Ordering::Relaxed) {
        "es"
    } else {
        "en"
    }
}
pub fn toggle() -> std::io::Result<()> {
    let spanish = !SPANISH.load(Ordering::Relaxed);
    std::fs::write(PATH, if spanish { "es" } else { "en" })?;
    SPANISH.store(spanish, Ordering::Relaxed);
    Ok(())
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

/// Move to the next/previous supported font size and persist it under
/// SD:/3ds/anki-3ds/settings.json.
pub fn adjust_font_scale(direction: i32) -> io::Result<u16> {
    let current = font_scale();
    let index = FONT_SCALES
        .iter()
        .position(|value| *value == current)
        .unwrap_or(1);
    let next = if direction < 0 {
        (index + FONT_SCALES.len() - 1) % FONT_SCALES.len()
    } else {
        (index + 1) % FONT_SCALES.len()
    };
    let percent = FONT_SCALES[next];
    save_font_scale(percent)?;
    FONT_SCALE.store(percent, Ordering::Relaxed);
    Ok(percent)
}

fn save_font_scale(percent: u16) -> io::Result<()> {
    let mut document = fs::read_to_string(SETTINGS_PATH)
        .ok()
        .and_then(|text| serde_json::from_str::<serde_json::Value>(&text).ok())
        .unwrap_or_else(|| serde_json::json!({}));
    if !document.is_object() {
        document = serde_json::json!({});
    }
    document["font_scale"] = serde_json::json!(percent);
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

pub fn tr(message: &'static str) -> &'static str {
    let catalog = if SPANISH.load(Ordering::Relaxed) {
        ES.get()
    } else {
        EN.get()
    };
    catalog
        .and_then(|c| c.get(message))
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
