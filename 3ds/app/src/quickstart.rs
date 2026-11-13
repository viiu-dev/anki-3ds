// Copyright: Anki 3DS contributors
// License: GNU AGPL, version 3 or later; http://www.gnu.org/licenses/agpl.html

use super::i18n;
use super::i18n::tr;
use super::theme;
use super::theme::current as current_theme;
use super::theme::ThemeKind;
use super::ui::Ui;

pub const STEPS: usize = 4;

pub enum TouchAction {
    None,
    SelectLanguage(usize),
    SelectRow(usize),
    Next,
    Back,
    Skip,
    Finish,
    LoginSync,
}

fn draw_card(x: f32, y: f32, w: f32, h: f32, border_color: u32, fill_color: u32) {
    Ui::draw_rounded_rect(x, y, w, h, 8.0, border_color);
    Ui::draw_rounded_rect(x + 1.0, y + 1.0, w - 2.0, h - 2.0, 7.0, fill_color);
}

pub fn draw_top(step: usize, selected: usize) {
    let th = current_theme();
    let title = if step == 3 {
        tr("Where everything lives")
    } else {
        tr("Quickstart")
    };
    Ui::draw_rect(0.0, 0.0, 400.0, 24.0, th.surface_alt);
    Ui::draw_line(0.0, 24.0, 400.0, 24.0, 1.0, th.border);
    Ui::draw_text(10.0, 5.0, 0.48, th.text, &Ui::ellipsize(title, 0.48, 250.0));
    let badge = i18n::format("Step {0} of {1}", &[&(step + 1).to_string(), "4"]);
    Ui::draw_text_right(390.0, 5.0, 0.44, th.accent, &badge);

    for i in 0..STEPS {
        let px = 10.0 + i as f32 * 96.0;
        let color = if i <= step { th.accent } else { th.border };
        Ui::draw_rect(px, 27.0, 92.0, 3.0, color);
    }

    match step {
        0 => draw_top_language(),
        1 => draw_top_appearance(),
        2 => draw_top_preferences(selected),
        _ => draw_top_guide(),
    }
}

fn draw_top_language() {
    let th = current_theme();
    draw_card(10.0, 34.0, 380.0, 194.0, th.border, th.surface);
    Ui::fit_text(
        200.0,
        52.0,
        0.64,
        356.0,
        th.text,
        tr("Welcome to Anki 3DS"),
    );
    Ui::fit_text(
        200.0,
        82.0,
        0.46,
        356.0,
        th.text_muted,
        tr("Choose your interface language."),
    );

    draw_card(105.0, 110.0, 190.0, 32.0, th.accent, th.tab_active_bg);
    let active_name = i18n::LANGUAGES[i18n::language_index()].1;
    Ui::fit_text(200.0, 118.0, 0.54, 176.0, th.accent, active_name);

    let pills = [
        tr("Decks"),
        tr("Review"),
        tr("Statistics"),
        tr("Settings"),
    ];
    for (i, label) in pills.iter().enumerate() {
        let px = 18.0 + i as f32 * 92.0;
        draw_card(px, 168.0, 86.0, 28.0, th.border, th.surface_alt);
        Ui::fit_text(px + 43.0, 175.0, 0.42, 78.0, th.text_muted, label);
    }
}

fn draw_top_appearance() {
    let th = current_theme();
    draw_card(10.0, 34.0, 380.0, 194.0, th.border, th.surface);
    Ui::draw_rect(11.0, 35.0, 378.0, 22.0, th.surface_alt);
    Ui::draw_line(11.0, 57.0, 389.0, 57.0, 1.0, th.border);
    Ui::draw_text(
        20.0,
        39.0,
        0.42,
        th.accent,
        &Ui::ellipsize(tr("Live preview"), 0.42, 180.0),
    );
    Ui::draw_text_right(336.0, 39.0, 0.42, th.new, "12");
    Ui::draw_text_right(358.0, 39.0, 0.42, th.learn, "4");
    Ui::draw_text_right(380.0, 39.0, 0.42, th.review, "28");

    let factor = i18n::font_scale_factor();
    let q_scale = (0.50 * factor).clamp(0.42, 0.88);
    let a_scale = (0.48 * factor).clamp(0.40, 0.80);
    Ui::fit_text(
        200.0,
        74.0,
        q_scale,
        356.0,
        th.text,
        tr("Sample question"),
    );
    Ui::draw_line(40.0, 114.0, 360.0, 114.0, 1.0, th.border);
    Ui::draw_rounded_rect(76.0, 126.0, 248.0, 34.0, 8.0, th.cloze_bg);
    Ui::fit_text(
        200.0,
        134.0,
        a_scale,
        236.0,
        th.text,
        tr("Sample answer"),
    );

    let ratings = [
        (tr("Again"), th.again_bg, th.again_fg),
        (tr("Hard"), th.hard_bg, th.hard_fg),
        (tr("Good"), th.good_bg, th.good_fg),
        (tr("Easy"), th.easy_bg, th.easy_fg),
    ];
    for (i, &(label, bg, fg)) in ratings.iter().enumerate() {
        let bx = 22.0 + i as f32 * 90.0;
        Ui::draw_rounded_rect(bx, 188.0, 84.0, 26.0, 6.0, bg);
        Ui::fit_text(bx + 42.0, 194.0, 0.42, 76.0, fg, label);
    }
}

fn draw_top_preferences(selected: usize) {
    let th = current_theme();
    let cards = [
        (
            tr("Console LEDs"),
            tr(if i18n::console_leds() { "On" } else { "Off" }),
            tr("Flash LEDs on ratings and sync."),
        ),
        (
            tr("Battery indicator"),
            tr(if i18n::battery_linear() {
                "Linear (25%)"
            } else {
                "System default"
            }),
            tr("4-step (25%) or system battery bar."),
        ),
        (
            tr("Audio playback"),
            tr(if i18n::autoplay_audio() { "On" } else { "Off" }),
            tr("Play card audio automatically."),
        ),
        (
            tr("Animations"),
            tr(if i18n::animations_enabled() {
                "On"
            } else {
                "Off"
            }),
            tr("Smooth transitions and button feedback."),
        ),
        (
            tr("UI Sound Effects"),
            tr(if i18n::ui_sfx() { "On" } else { "Off" }),
            tr("Subtle chimes for reveal, ratings, and deck completion."),
        ),
    ];
    for (i, &(title, value, desc)) in cards.iter().enumerate() {
        let cy = 31.0 + i as f32 * 40.0;
        let active = i == selected;
        draw_card(
            10.0,
            cy,
            380.0,
            37.0,
            if active { th.accent } else { th.border },
            if active { th.row_selected } else { th.surface },
        );
        Ui::draw_text(
            20.0,
            cy + 4.0,
            0.43,
            th.text,
            &Ui::ellipsize(title, 0.43, 220.0),
        );
        Ui::draw_text_right(
            380.0,
            cy + 4.0,
            0.41,
            if active { th.accent } else { th.text_muted },
            &Ui::ellipsize(value, 0.41, 130.0),
        );
        Ui::draw_text(
            20.0,
            cy + 19.0,
            0.36,
            th.text_muted,
            &Ui::ellipsize(desc, 0.36, 360.0),
        );
    }
}

fn draw_top_guide() {
    let th = current_theme();
    let panels = [
        (
            tr("Top screen"),
            tr("Full card view & vertical scroll."),
            tr("D-Pad / Circle Pad"),
        ),
        (
            tr("Bottom screen"),
            tr("Audio clips, Undo & ratings."),
            tr("Again / Hard / Good / Easy"),
        ),
        (
            tr("Menu [SELECT]"),
            tr("Sync, Browse, Backups & Stats."),
            tr("Options & Settings"),
        ),
        (
            tr("Shortcuts"),
            tr("Custom buttons & scroll speed."),
            "A / B / X / Y · L / R · ZL / ZR",
        ),
    ];
    for (i, &(heading, line1, line2)) in panels.iter().enumerate() {
        let col = i % 2;
        let row = i / 2;
        let x = 10.0 + col as f32 * 194.0;
        let y = 35.0 + row as f32 * 100.0;
        draw_card(x, y, 186.0, 92.0, th.border, th.surface);
        Ui::draw_rect(x + 1.0, y + 1.0, 184.0, 22.0, th.surface_alt);
        Ui::draw_line(x + 1.0, y + 23.0, x + 185.0, y + 23.0, 1.0, th.border);
        Ui::draw_text(
            x + 8.0,
            y + 5.0,
            0.44,
            th.accent,
            &Ui::ellipsize(heading, 0.44, 170.0),
        );
        Ui::draw_text(
            x + 8.0,
            y + 34.0,
            0.39,
            th.text,
            &Ui::ellipsize(line1, 0.39, 170.0),
        );
        Ui::draw_text(
            x + 8.0,
            y + 58.0,
            0.38,
            th.text_muted,
            &Ui::ellipsize(line2, 0.38, 170.0),
        );
    }
}

pub fn draw_bottom(step: usize, selected: usize) {
    let th = current_theme();
    let header_title = match step {
        0 => tr("Step 1: Language"),
        1 => tr("Step 2: Appearance"),
        2 => tr("Step 3: Preferences"),
        _ => tr("Step 4: Quick guide"),
    };
    Ui::draw_rect(0.0, 0.0, 320.0, 42.0, th.surface_alt);
    Ui::draw_line(0.0, 42.0, 320.0, 42.0, 1.0, th.border);
    Ui::fit_text(160.0, 12.0, 0.58, 296.0, th.text, header_title);

    match step {
        0 => {
            let active = i18n::language_index();
            for (index, (_, name)) in i18n::LANGUAGES.iter().enumerate() {
                let col = index % 3;
                let row = index / 3;
                let x = 10.0 + col as f32 * 102.0;
                let y = 52.0 + row as f32 * 36.0;
                let style = if index == selected || index == active {
                    0
                } else {
                    6
                };
                Ui::draw_button(x, y, 96.0, 28.0, name, None, style, false);
                if index == active {
                    Ui::draw_sync_icon(x + 82.0, y + 9.0, 10.0, 0);
                }
            }
            Ui::draw_button(12.0, 196.0, 142.0, 28.0, tr("Skip [B]"), None, 6, false);
            Ui::draw_button(166.0, 196.0, 142.0, 28.0, tr("Next [A]"), None, 0, false);
        }
        1 => {
            let rows = [
                i18n::format("Theme: {0}", &[tr(theme::kind().label())]),
                i18n::format("Font scale: {0}%", &[&i18n::font_scale().to_string()]),
                i18n::format(
                    "{0}: {1}",
                    &[tr("Accent color"), tr(theme::accent_kind().label())],
                ),
            ];
            for (i, label) in rows.iter().enumerate() {
                let y = 50.0 + i as f32 * 40.0;
                let style = if i == selected { 0 } else { 6 };
                Ui::draw_button(12.0, y, 296.0, 32.0, label, None, style, false);
            }
            Ui::fit_text(
                160.0,
                172.0,
                0.38,
                296.0,
                th.text_muted,
                tr("Tap or LEFT/RIGHT changes."),
            );
            Ui::draw_button(12.0, 196.0, 94.0, 28.0, tr("Back [B]"), None, 6, false);
            Ui::draw_button(113.0, 196.0, 94.0, 28.0, tr("Skip"), None, 6, false);
            Ui::draw_button(214.0, 196.0, 94.0, 28.0, tr("Next [A]"), None, 0, false);
        }
        2 => {
            let rows = [
                i18n::format(
                    "Console LEDs: {0}",
                    &[tr(if i18n::console_leds() { "On" } else { "Off" })],
                ),
                i18n::format(
                    "Battery indicator: {0}",
                    &[tr(if i18n::battery_linear() {
                        "Linear (25%)"
                    } else {
                        "System default"
                    })],
                ),
                i18n::format(
                    "Auto-play audio: {0}",
                    &[tr(if i18n::autoplay_audio() { "On" } else { "Off" })],
                ),
                i18n::format(
                    "Animations: {0}",
                    &[tr(if i18n::animations_enabled() {
                        "On"
                    } else {
                        "Off"
                    })],
                ),
                i18n::format(
                    "UI Sound Effects: {0}",
                    &[tr(if i18n::ui_sfx() { "On" } else { "Off" })],
                ),
            ];
            for (i, label) in rows.iter().enumerate() {
                let y = 45.0 + i as f32 * 26.0;
                let style = if i == selected { 0 } else { 6 };
                Ui::draw_button(12.0, y, 296.0, 23.0, label, None, style, false);
            }
            Ui::fit_text(
                160.0,
                176.0,
                0.37,
                296.0,
                th.text_muted,
                tr("Tap or LEFT/RIGHT toggles."),
            );
            Ui::draw_button(12.0, 196.0, 94.0, 28.0, tr("Back [B]"), None, 6, false);
            Ui::draw_button(113.0, 196.0, 94.0, 28.0, tr("Skip"), None, 6, false);
            Ui::draw_button(214.0, 196.0, 94.0, 28.0, tr("Next [A]"), None, 0, false);
        }
        _ => {
            draw_card(12.0, 48.0, 296.0, 50.0, th.border, th.surface);
            Ui::fit_text(
                160.0,
                56.0,
                0.48,
                280.0,
                th.text,
                tr("You are ready to study!"),
            );
            Ui::fit_text(
                160.0,
                77.0,
                0.38,
                280.0,
                th.text_muted,
                tr("Reopen anytime in Settings > System."),
            );
            Ui::draw_button(
                12.0,
                108.0,
                296.0,
                34.0,
                tr("Finish"),
                None,
                if selected == 0 { 0 } else { 6 },
                false,
            );
            Ui::draw_button(
                12.0,
                150.0,
                296.0,
                34.0,
                tr("Log in and Sync"),
                None,
                if selected == 1 { 0 } else { 6 },
                false,
            );
            Ui::draw_button(60.0, 196.0, 200.0, 28.0, tr("Back [B]"), None, 6, false);
        }
    }
}

pub fn touch_hit(step: usize, x: f32, y: f32) -> TouchAction {
    match step {
        0 => {
            for index in 0..i18n::LANGUAGES.len() {
                let col = index % 3;
                let row = index / 3;
                let bx = 10.0 + col as f32 * 102.0;
                let by = 52.0 + row as f32 * 36.0;
                if x >= bx && x < bx + 96.0 && y >= by && y < by + 28.0 {
                    return TouchAction::SelectLanguage(index);
                }
            }
            if (196.0..224.0).contains(&y) {
                if (12.0..154.0).contains(&x) {
                    return TouchAction::Skip;
                }
                if (166.0..308.0).contains(&x) {
                    return TouchAction::Next;
                }
            }
            TouchAction::None
        }
        1 | 2 => {
            let (rows, y0, step_h, h) = if step == 1 {
                (3, 50.0, 40.0, 32.0)
            } else {
                (5, 45.0, 26.0, 23.0)
            };
            for i in 0..rows {
                let by = y0 + i as f32 * step_h;
                if (12.0..308.0).contains(&x) && y >= by && y < by + h {
                    return TouchAction::SelectRow(i);
                }
            }
            if (196.0..224.0).contains(&y) {
                if (12.0..106.0).contains(&x) {
                    return TouchAction::Back;
                }
                if (113.0..207.0).contains(&x) {
                    return TouchAction::Skip;
                }
                if (214.0..308.0).contains(&x) {
                    return TouchAction::Next;
                }
            }
            TouchAction::None
        }
        _ => {
            if (12.0..308.0).contains(&x) {
                if (108.0..142.0).contains(&y) {
                    return TouchAction::Finish;
                }
                if (150.0..184.0).contains(&y) {
                    return TouchAction::LoginSync;
                }
            }
            if (196.0..224.0).contains(&y) && (60.0..260.0).contains(&x) {
                return TouchAction::Back;
            }
            TouchAction::None
        }
    }
}

pub fn apply_step_change(step: usize, row: usize, forward: bool) -> std::io::Result<()> {
    let delta: i32 = if forward { 1 } else { -1 };
    match (step, row) {
        (1, 0) => {
            let count = ThemeKind::ALL.len();
            let cur = ThemeKind::ALL
                .iter()
                .position(|v| *v == theme::kind())
                .unwrap_or(0);
            let next = (cur as i32 + delta).rem_euclid(count as i32) as usize;
            theme::set(ThemeKind::ALL[next])?;
            Ui::apply_theme();
        }
        (1, 1) => {
            let count = i18n::FONT_SCALES.len();
            let cur = i18n::FONT_SCALES
                .iter()
                .position(|v| *v == i18n::font_scale())
                .unwrap_or(1);
            let next = (cur as i32 + delta).rem_euclid(count as i32) as usize;
            i18n::set_font_scale(i18n::FONT_SCALES[next])?;
        }
        (1, 2) => {
            let count = theme::AccentKind::ALL.len();
            let cur = theme::AccentKind::ALL
                .iter()
                .position(|v| *v == theme::accent_kind())
                .unwrap_or(0);
            let next = (cur as i32 + delta).rem_euclid(count as i32) as usize;
            theme::set_accent(theme::AccentKind::ALL[next])?;
            Ui::apply_theme();
        }
        (2, 0) => {
            let value = i18n::toggle_console_leds()?;
            super::leds::apply(value);
        }
        (2, 1) => {
            let value = !i18n::battery_linear();
            i18n::set_battery_linear(value)?;
            Ui::set_battery_linear(value);
        }
        (2, 2) => {
            i18n::toggle_autoplay_audio()?;
        }
        (2, 3) => {
            i18n::toggle_animations()?;
        }
        (2, 4) => {
            if i18n::toggle_ui_sfx()? {
                super::sfx::play_preview();
            }
        }
        _ => {}
    }
    Ok(())
}
