// Copyright: Anki 3DS contributors
// License: GNU AGPL, version 3 or later; http://www.gnu.org/licenses/agpl.html

use std::fs;
use std::io;
use std::sync::atomic::AtomicU8;
use std::sync::atomic::Ordering;

use ctru::services::hid::KeyPad;

const SETTINGS_PATH: &str = "sdmc:/3ds/anki-3ds/settings.json";

#[repr(u8)]
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Button {
    A = 0,
    B = 1,
    X = 2,
    Y = 3,
    L = 4,
    R = 5,
    ZL = 6,
    ZR = 7,
    Up = 8,
    Down = 9,
    Left = 10,
    Right = 11,
    CPadUp = 12,
    CPadDown = 13,
    CPadLeft = 14,
    CPadRight = 15,
    CStickUp = 16,
    CStickDown = 17,
    CStickLeft = 18,
    CStickRight = 19,
}

impl Button {
    pub const ALL: [Button; 20] = [
        Button::A,
        Button::B,
        Button::X,
        Button::Y,
        Button::L,
        Button::R,
        Button::ZL,
        Button::ZR,
        Button::Up,
        Button::Down,
        Button::Left,
        Button::Right,
        Button::CPadUp,
        Button::CPadDown,
        Button::CPadLeft,
        Button::CPadRight,
        Button::CStickUp,
        Button::CStickDown,
        Button::CStickLeft,
        Button::CStickRight,
    ];

    pub fn key(self) -> &'static str {
        match self {
            Button::A => "a",
            Button::B => "b",
            Button::X => "x",
            Button::Y => "y",
            Button::L => "l",
            Button::R => "r",
            Button::ZL => "zl",
            Button::ZR => "zr",
            Button::Up => "up",
            Button::Down => "down",
            Button::Left => "left",
            Button::Right => "right",
            Button::CPadUp => "cpad_up",
            Button::CPadDown => "cpad_down",
            Button::CPadLeft => "cpad_left",
            Button::CPadRight => "cpad_right",
            Button::CStickUp => "cstick_up",
            Button::CStickDown => "cstick_down",
            Button::CStickLeft => "cstick_left",
            Button::CStickRight => "cstick_right",
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Button::A => "A",
            Button::B => "B",
            Button::X => "X",
            Button::Y => "Y",
            Button::L => "L",
            Button::R => "R",
            Button::ZL => "ZL",
            Button::ZR => "ZR",
            Button::Up => "D-Pad Up",
            Button::Down => "D-Pad Down",
            Button::Left => "D-Pad Left",
            Button::Right => "D-Pad Right",
            Button::CPadUp => "Circle Pad Up",
            Button::CPadDown => "Circle Pad Down",
            Button::CPadLeft => "Circle Pad Left",
            Button::CPadRight => "Circle Pad Right",
            Button::CStickUp => "C-Stick Up",
            Button::CStickDown => "C-Stick Down",
            Button::CStickLeft => "C-Stick Left",
            Button::CStickRight => "C-Stick Right",
        }
    }

    fn from_key(key: &str) -> Option<Self> {
        Self::ALL.iter().copied().find(|button| button.key() == key)
    }

    fn from_index(index: u8) -> Option<Self> {
        Self::ALL.get(usize::from(index)).copied()
    }

    fn index(self) -> u8 {
        self as u8
    }

    pub fn pressed(self, keys: KeyPad) -> bool {
        keys.intersects(match self {
            Button::A => KeyPad::A,
            Button::B => KeyPad::B,
            Button::X => KeyPad::X,
            Button::Y => KeyPad::Y,
            Button::L => KeyPad::L,
            Button::R => KeyPad::R,
            Button::ZL => KeyPad::ZL,
            Button::ZR => KeyPad::ZR,
            Button::Up => KeyPad::DPAD_UP,
            Button::Down => KeyPad::DPAD_DOWN,
            Button::Left => KeyPad::DPAD_LEFT,
            Button::Right => KeyPad::DPAD_RIGHT,
            Button::CPadUp => KeyPad::CPAD_UP,
            Button::CPadDown => KeyPad::CPAD_DOWN,
            Button::CPadLeft => KeyPad::CPAD_LEFT,
            Button::CPadRight => KeyPad::CPAD_RIGHT,
            Button::CStickUp => KeyPad::CSTICK_UP,
            Button::CStickDown => KeyPad::CSTICK_DOWN,
            Button::CStickLeft => KeyPad::CSTICK_LEFT,
            Button::CStickRight => KeyPad::CSTICK_RIGHT,
        })
    }
}

#[repr(u8)]
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum ReviewAction {
    ShowAnswer = 0,
    Again = 1,
    Hard = 2,
    Good = 3,
    Easy = 4,
    PlayAudio = 5,
    Undo = 6,
    Bury = 7,
    Hint = 8,
    ScrollUp = 9,
    ScrollDown = 10,
}

const FRONT: u8 = 1;
const BACK: u8 = 2;

impl ReviewAction {
    pub const ALL: [ReviewAction; 11] = [
        ReviewAction::ShowAnswer,
        ReviewAction::Again,
        ReviewAction::Hard,
        ReviewAction::Good,
        ReviewAction::Easy,
        ReviewAction::PlayAudio,
        ReviewAction::Undo,
        ReviewAction::Bury,
        ReviewAction::Hint,
        ReviewAction::ScrollUp,
        ReviewAction::ScrollDown,
    ];

    pub fn key(self) -> &'static str {
        match self {
            ReviewAction::ShowAnswer => "show_answer",
            ReviewAction::Again => "again",
            ReviewAction::Hard => "hard",
            ReviewAction::Good => "good",
            ReviewAction::Easy => "easy",
            ReviewAction::PlayAudio => "play_audio",
            ReviewAction::Undo => "undo",
            ReviewAction::Bury => "bury",
            ReviewAction::Hint => "hint",
            ReviewAction::ScrollUp => "scroll_up",
            ReviewAction::ScrollDown => "scroll_down",
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            ReviewAction::ShowAnswer => "Show answer",
            ReviewAction::Again => "Again",
            ReviewAction::Hard => "Hard",
            ReviewAction::Good => "Good",
            ReviewAction::Easy => "Easy",
            ReviewAction::PlayAudio => "Play/Replay audio",
            ReviewAction::Undo => "Undo",
            ReviewAction::Bury => "Bury",
            ReviewAction::Hint => "Reveal hint/Type answer",
            ReviewAction::ScrollUp => "Scroll up",
            ReviewAction::ScrollDown => "Scroll down",
        }
    }

    fn states(self) -> u8 {
        match self {
            ReviewAction::ShowAnswer | ReviewAction::Hint => FRONT,
            ReviewAction::Again | ReviewAction::Hard | ReviewAction::Good | ReviewAction::Easy => {
                BACK
            }
            ReviewAction::PlayAudio
            | ReviewAction::Undo
            | ReviewAction::Bury
            | ReviewAction::ScrollUp
            | ReviewAction::ScrollDown => FRONT | BACK,
        }
    }

    fn index(self) -> usize {
        self as usize
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum ScreenState {
    Front,
    Back,
}

impl ScreenState {
    fn bit(self) -> u8 {
        match self {
            ScreenState::Front => FRONT,
            ScreenState::Back => BACK,
        }
    }
}

const NONE: u8 = u8::MAX;

const DEFAULTS: [Button; 11] = [
    Button::A,
    Button::Y,
    Button::X,
    Button::A,
    Button::R,
    Button::L,
    Button::ZL,
    Button::ZR,
    Button::X,
    Button::Up,
    Button::Down,
];

static MAP: [AtomicU8; 11] = [const { AtomicU8::new(NONE) }; 11];

fn store_defaults() {
    for (action, button) in ReviewAction::ALL.iter().zip(DEFAULTS) {
        MAP[action.index()].store(button.index(), Ordering::Relaxed);
    }
}

pub fn init() {
    store_defaults();
    let Ok(text) = fs::read_to_string(SETTINGS_PATH) else {
        return;
    };
    let Ok(document) = serde_json::from_str::<serde_json::Value>(&text) else {
        return;
    };
    let Some(bindings) = document.get("controls").and_then(|value| value.as_object()) else {
        return;
    };
    let mut assigned = [NONE; 11];
    for action in ReviewAction::ALL {
        let Some(value) = bindings.get(action.key()) else {
            assigned[action.index()] = MAP[action.index()].load(Ordering::Relaxed);
            continue;
        };
        let Some(button) = value.as_str().and_then(Button::from_key) else {
            assigned[action.index()] = NONE;
            continue;
        };
        let conflict = ReviewAction::ALL.iter().any(|other| {
            other.index() != action.index()
                && other.states() & action.states() != 0
                && assigned[other.index()] == button.index()
        });
        if !conflict {
            assigned[action.index()] = button.index();
        }
    }
    for action in ReviewAction::ALL {
        MAP[action.index()].store(assigned[action.index()], Ordering::Relaxed);
    }
}

pub fn button_for(action: ReviewAction) -> Option<Button> {
    let value = MAP[action.index()].load(Ordering::Relaxed);
    if value == NONE {
        None
    } else {
        Button::from_index(value)
    }
}

pub fn action_for(button: Button, state: ScreenState) -> Option<ReviewAction> {
    ReviewAction::ALL
        .iter()
        .copied()
        .find(|action| action.states() & state.bit() != 0 && button_for(*action) == Some(button))
}

pub fn conflict_for(action: ReviewAction, button: Button) -> Option<ReviewAction> {
    ReviewAction::ALL.iter().copied().find(|other| {
        *other != action
            && other.states() & action.states() != 0
            && button_for(*other) == Some(button)
    })
}

pub fn try_assign(
    action: ReviewAction,
    button: Option<Button>,
) -> io::Result<Option<ReviewAction>> {
    if let Some(button) = button {
        if let Some(conflict) = conflict_for(action, button) {
            return Ok(Some(conflict));
        }
    }
    MAP[action.index()].store(button.map_or(NONE, Button::index), Ordering::Relaxed);
    save()?;
    Ok(None)
}

pub fn reset() -> io::Result<()> {
    store_defaults();
    save()
}

fn save() -> io::Result<()> {
    let mut document = fs::read_to_string(SETTINGS_PATH)
        .ok()
        .and_then(|text| serde_json::from_str::<serde_json::Value>(&text).ok())
        .unwrap_or_else(|| serde_json::json!({}));
    if !document.is_object() {
        document = serde_json::json!({});
    }
    let mut bindings = serde_json::Map::new();
    for action in ReviewAction::ALL {
        let value = match button_for(action) {
            Some(button) => serde_json::json!(button.key()),
            None => serde_json::Value::Null,
        };
        bindings.insert(action.key().to_owned(), value);
    }
    document["controls"] = serde_json::Value::Object(bindings);
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

pub fn button_option(index: usize) -> Option<Button> {
    if index == 0 {
        None
    } else {
        Button::ALL.get(index - 1).copied()
    }
}

pub fn button_option_index(button: Option<Button>) -> usize {
    button.map_or(0, |button| usize::from(button.index()) + 1)
}

pub fn button_option_count() -> usize {
    Button::ALL.len() + 1
}
