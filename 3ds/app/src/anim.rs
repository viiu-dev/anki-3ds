// Copyright: Anki 3DS contributors
// License: GNU AGPL, version 3 or later; http://www.gnu.org/licenses/agpl.html

use std::f32::consts::{PI, TAU};
use std::sync::atomic::{AtomicU32, AtomicU8, Ordering};
use std::sync::OnceLock;
use std::time::Instant;

use super::i18n::{self, tr, AnimKind};
use super::theme::current as current_theme;
use super::ui::Ui;

const BUTTON_MS: u32 = 160;
const RIPPLE_MS: u32 = 220;
const CARD_REVEAL_MS: u32 = 180;
const CARD_NEXT_MS: u32 = 200;
const MENU_MS: u32 = 160;

static EPOCH: OnceLock<Instant> = OnceLock::new();
static PRESS_AT: AtomicU32 = AtomicU32::new(0);
static PRESS_MODE: AtomicU8 = AtomicU8::new(0);
static PRESS_STYLE: AtomicU8 = AtomicU8::new(0);
static PRESS_X: AtomicU32 = AtomicU32::new(0);
static PRESS_Y: AtomicU32 = AtomicU32::new(0);

fn now_ms() -> u32 {
    EPOCH.get_or_init(Instant::now).elapsed().as_millis().min(u32::MAX as u128 - 1) as u32
}

fn elapsed_since(stamp: &AtomicU32, window: u32) -> Option<u32> {
    let raw = stamp.load(Ordering::Relaxed);
    if raw == 0 {
        return None;
    }
    let dt = now_ms().saturating_sub(raw - 1);
    (dt < window).then_some(dt)
}

pub fn record_touch(x: f32, y: f32) {
    PRESS_X.store(x.to_bits(), Ordering::Relaxed);
    PRESS_Y.store(y.to_bits(), Ordering::Relaxed);
    PRESS_MODE.store(1, Ordering::Relaxed);
    PRESS_AT.store(now_ms().saturating_add(1), Ordering::Relaxed);
}

pub fn record_button_style(style: u8) {
    PRESS_STYLE.store(style, Ordering::Relaxed);
    PRESS_MODE.store(2, Ordering::Relaxed);
    PRESS_AT.store(now_ms().saturating_add(1), Ordering::Relaxed);
}

fn ease_out_cubic(t: f32) -> f32 {
    let t = t.clamp(0.0, 1.0);
    1.0 - (1.0 - t).powi(3)
}

fn scale_alpha(color: u32, alpha: f32) -> u32 {
    let base = (color >> 24) as f32;
    let a = (base * alpha.clamp(0.0, 1.0)).round().clamp(0.0, 255.0) as u32;
    (color & 0x00ff_ffff) | (a << 24)
}

fn draw_card(x: f32, y: f32, w: f32, h: f32, border: u32, fill: u32) {
    Ui::draw_rounded_rect(x, y, w, h, 8.0, border);
    Ui::draw_rounded_rect(x + 1.0, y + 1.0, w - 2.0, h - 2.0, 7.0, fill);
}

fn draw_circle_outline(cx: f32, cy: f32, radius: f32, thickness: f32, color: u32) {
    let steps = 16;
    for i in 0..steps {
        let a = i as f32 * TAU / steps as f32;
        let b = (i + 1) as f32 * TAU / steps as f32;
        Ui::draw_line(
            cx + a.cos() * radius,
            cy + a.sin() * radius,
            cx + b.cos() * radius,
            cy + b.sin() * radius,
            thickness,
            color,
        );
    }
}

pub fn adjust_button(
    x: f32,
    y: f32,
    w: f32,
    h: f32,
    style: i32,
    pressed: bool,
) -> (f32, f32, f32, f32, bool, Option<u32>) {
    if !i18n::anim_active(AnimKind::ButtonFeedback) {
        return (x, y, w, h, pressed, None);
    }
    let Some(elapsed) = elapsed_since(&PRESS_AT, BUTTON_MS) else {
        return (x, y, w, h, pressed, None);
    };
    let mode = PRESS_MODE.load(Ordering::Relaxed);
    let matched = match mode {
        1 => {
            let tx = f32::from_bits(PRESS_X.load(Ordering::Relaxed));
            let ty = f32::from_bits(PRESS_Y.load(Ordering::Relaxed));
            tx >= x - 2.0 && tx <= x + w + 2.0 && ty >= y - 2.0 && ty <= y + h + 2.0
        }
        2 => {
            let target = PRESS_STYLE.load(Ordering::Relaxed);
            style == i32::from(target)
        }
        _ => false,
    };
    if !matched {
        return (x, y, w, h, pressed, None);
    }
    let t = elapsed as f32 / BUTTON_MS as f32;
    let pulse = (t * PI).sin();
    let th = current_theme();
    let halo_base = match style {
        2 => th.again_fg,
        3 => th.hard_fg,
        4 => th.good_fg,
        5 => th.easy_fg,
        _ => th.accent,
    };
    (
        x + 0.8 * pulse,
        y + 1.4 * pulse,
        (w - 1.6 * pulse).max(4.0),
        (h - 1.4 * pulse).max(4.0),
        pressed || t < 0.55,
        Some(scale_alpha(halo_base, (1.0 - t) * 0.55)),
    )
}

pub fn draw_button_halo(x: f32, y: f32, w: f32, h: f32, color: u32) {
    let radius = (h * 0.5).min(16.0) + 2.0;
    Ui::draw_rounded_rect(x - 2.0, y - 2.0, w + 4.0, h + 4.0, radius, color);
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum CardAnimKind {
    Reveal,
    NextCard,
}

pub struct UiAnimState {
    card_anim: Option<(CardAnimKind, Instant)>,
    menu_at: Option<Instant>,
    celebration_at: Instant,
}

impl UiAnimState {
    pub fn new() -> Self {
        Self {
            card_anim: None,
            menu_at: None,
            celebration_at: Instant::now(),
        }
    }

    pub fn trigger_card_reveal(&mut self) {
        if i18n::anim_active(AnimKind::CardTransitions) {
            self.card_anim = Some((CardAnimKind::Reveal, Instant::now()));
        }
    }

    pub fn trigger_next_card(&mut self) {
        if i18n::anim_active(AnimKind::CardTransitions) {
            self.card_anim = Some((CardAnimKind::NextCard, Instant::now()));
        }
    }

    pub fn clear_card_transition(&mut self) {
        self.card_anim = None;
    }

    pub fn trigger_menu_transition(&mut self) {
        if i18n::anim_active(AnimKind::MenuTransitions) {
            self.menu_at = Some(Instant::now());
        }
    }

    pub fn trigger_celebration(&mut self) {
        self.celebration_at = Instant::now();
    }

    pub fn card_transition(&mut self) -> (f32, f32, f32) {
        if !i18n::anim_active(AnimKind::CardTransitions) {
            self.card_anim = None;
            return (0.0, 0.0, 0.0);
        }
        let Some((kind, at)) = self.card_anim else {
            return (0.0, 0.0, 0.0);
        };
        let duration = match kind {
            CardAnimKind::Reveal => CARD_REVEAL_MS,
            CardAnimKind::NextCard => CARD_NEXT_MS,
        } as f32;
        let ms = at.elapsed().as_secs_f32() * 1000.0;
        if ms >= duration {
            self.card_anim = None;
            return (0.0, 0.0, 0.0);
        }
        let ease = ease_out_cubic(ms / duration);
        let rem = 1.0 - ease;
        match kind {
            CardAnimKind::Reveal => (0.0, 14.0 * rem, rem),
            CardAnimKind::NextCard => (28.0 * rem, 0.0, rem),
        }
    }

    pub fn draw_top_overlay(&mut self) {
        if !i18n::anim_active(AnimKind::MenuTransitions) {
            self.menu_at = None;
            return;
        }
        let Some(at) = self.menu_at else {
            return;
        };
        let ms = at.elapsed().as_secs_f32() * 1000.0;
        if ms >= MENU_MS as f32 {
            return;
        }
        let ease = ease_out_cubic(ms / MENU_MS as f32);
        let rem = 1.0 - ease;
        let th = current_theme();
        Ui::draw_rect(0.0, 0.0, 400.0, 240.0, scale_alpha(th.bg_top, rem * 0.42));
        Ui::draw_rect(
            0.0,
            0.0,
            400.0 * ease,
            2.0,
            scale_alpha(th.accent, rem * 0.85),
        );
    }

    pub fn draw_bottom_overlay(&mut self) {
        let th = current_theme();
        if i18n::anim_active(AnimKind::ButtonFeedback) && PRESS_MODE.load(Ordering::Relaxed) == 1 {
            if let Some(elapsed) = elapsed_since(&PRESS_AT, RIPPLE_MS) {
                let t = elapsed as f32 / RIPPLE_MS as f32;
                let tx = f32::from_bits(PRESS_X.load(Ordering::Relaxed));
                let ty = f32::from_bits(PRESS_Y.load(Ordering::Relaxed));
                let ease = ease_out_cubic(t);
                let radius = 5.0 + 18.0 * ease;
                draw_circle_outline(tx, ty, radius, 1.4, scale_alpha(th.accent, (1.0 - t) * 0.65));
            }
        }
        if !i18n::anim_active(AnimKind::MenuTransitions) {
            self.menu_at = None;
            return;
        }
        if let Some(at) = self.menu_at {
            let ms = at.elapsed().as_secs_f32() * 1000.0;
            if ms >= MENU_MS as f32 {
                self.menu_at = None;
            } else {
                let ease = ease_out_cubic(ms / MENU_MS as f32);
                let rem = 1.0 - ease;
                Ui::draw_rect(
                    0.0,
                    0.0,
                    320.0,
                    240.0,
                    scale_alpha(th.bg_bottom, rem * 0.42),
                );
                Ui::draw_rect(
                    0.0,
                    0.0,
                    320.0 * ease,
                    2.0,
                    scale_alpha(th.accent, rem * 0.85),
                );
            }
        }
    }

    pub fn draw_deck_celebration(&self, deck_name: &str, daily_studied: &str) {
        let secs = self.celebration_at.elapsed().as_secs_f32();
        let entry = ease_out_cubic((secs / 0.36).clamp(0.0, 1.0));
        let th = current_theme();
        draw_confetti(10.0, 8.0, 380.0, 224.0, secs, 24);
        let card_y = 24.0 + (1.0 - entry) * 14.0;
        draw_card(24.0, card_y, 352.0, 192.0, th.accent, th.surface);
        draw_celebration_badge(200.0, card_y + 42.0, secs, entry);
        Ui::fit_text(
            200.0,
            card_y + 78.0,
            0.62,
            320.0,
            th.accent,
            tr("Deck complete!"),
        );
        Ui::fit_text(
            200.0,
            card_y + 108.0,
            0.48,
            320.0,
            th.text,
            tr("No cards available now"),
        );
        Ui::fit_text(
            200.0,
            card_y + 132.0,
            0.42,
            320.0,
            th.text_muted,
            &i18n::format("Waiting for learning cards in {0}.", &[deck_name]),
        );
        if !daily_studied.is_empty() {
            Ui::fit_text(
                200.0,
                card_y + 158.0,
                0.40,
                320.0,
                th.good_fg,
                daily_studied,
            );
        }
    }
}

pub fn draw_card_transition_overlay(fade: f32, answer: bool) {
    if fade <= 0.001 {
        return;
    }
    let th = current_theme();
    Ui::draw_rect(0.0, 0.0, 400.0, 240.0, scale_alpha(th.bg_top, fade * 0.55));
    if answer {
        let w = 360.0 * (1.0 - fade);
        Ui::draw_rect(20.0, 2.0, w, 2.0, scale_alpha(th.accent, fade * 0.85));
    }
}

fn draw_confetti(x: f32, y: f32, w: f32, h: f32, secs: f32, count: usize) {
    let th = current_theme();
    let palette = [
        th.accent, th.new, th.learn, th.review, th.good_fg, th.easy_fg,
    ];
    for i in 0..count {
        let seed = i as f32 * 1.37;
        let speed = 22.0 + (i % 5) as f32 * 6.5;
        let cycle = (seed * 17.0 + secs * speed).rem_euclid(h);
        let sway = (secs * (2.2 + (i % 3) as f32 * 0.6) + seed).sin() * 9.0;
        let px = x + ((i as f32 * 43.0 + 19.0).rem_euclid(w - 16.0)) + 8.0 + sway;
        let py = y + cycle;
        let alpha = (cycle / 20.0).min(1.0) * ((h - cycle) / 24.0).min(1.0);
        let color = scale_alpha(palette[i % palette.len()], alpha * 0.85);
        let size = 4.0 + (i % 3) as f32 * 1.5;
        if i % 2 == 0 {
            Ui::draw_rect(px, py, size, size * 0.65, color);
        } else {
            Ui::draw_rounded_rect(px, py, size, size, size * 0.5, color);
        }
    }
}

fn draw_celebration_badge(cx: f32, cy: f32, secs: f32, entry: f32) {
    let th = current_theme();
    let ring = (secs * 0.85).fract();
    let pulse = 18.0 + ring * 18.0;
    draw_circle_outline(
        cx,
        cy,
        pulse,
        1.5,
        scale_alpha(th.accent, (1.0 - ring) * 0.55 * entry),
    );
    let r = 18.0 * entry.max(0.2);
    Ui::draw_rounded_rect(cx - r, cy - r, r * 2.0, r * 2.0, r, th.good_bg);
    draw_circle_outline(cx, cy, r, 1.5, th.good_fg);
    Ui::draw_line(
        cx - 7.0 * entry,
        cy + 1.0 * entry,
        cx - 2.0 * entry,
        cy + 6.0 * entry,
        2.5,
        th.good_fg,
    );
    Ui::draw_line(
        cx - 2.0 * entry,
        cy + 6.0 * entry,
        cx + 8.0 * entry,
        cy - 6.0 * entry,
        2.5,
        th.good_fg,
    );
}

pub fn draw_settings_preview(selected: usize, frame: u64) {
    let th = current_theme();
    let (title, desc, active) = if selected == 0 {
        (
            tr("All animations"),
            tr("Smooth transitions and button feedback."),
            i18n::animations_enabled(),
        )
    } else {
        let kind = AnimKind::ALL[(selected - 1).min(AnimKind::ALL.len() - 1)];
        (
            tr(kind.label()),
            tr(kind.description()),
            i18n::anim_active(kind),
        )
    };
    Ui::draw_rect(0.0, 0.0, 400.0, 24.0, th.surface_alt);
    Ui::draw_line(0.0, 24.0, 400.0, 24.0, 1.0, th.border);
    Ui::draw_text(10.0, 5.0, 0.48, th.text, &Ui::ellipsize(title, 0.48, 280.0));
    let badge_color = if active { th.accent } else { th.text_muted };
    Ui::draw_text_right(390.0, 5.0, 0.44, badge_color, tr(if active { "On" } else { "Off" }));
    Ui::fit_text(200.0, 29.0, 0.40, 380.0, th.text_muted, desc);

    draw_card(12.0, 48.0, 376.0, 180.0, th.border, th.surface);
    let stage = if selected == 0 {
        ((frame / 150) % 4) as usize
    } else {
        (selected - 1).min(3)
    };
    let stage_frame = frame % 150;
    match stage {
        0 => draw_stage_card_transitions(stage_frame),
        1 => draw_stage_button_feedback(stage_frame),
        2 => draw_stage_menu_transitions(stage_frame),
        _ => draw_stage_deck_celebration(stage_frame),
    }
    if selected == 0 {
        for i in 0..4 {
            let dot_x = 182.0 + i as f32 * 12.0;
            let color = if i == stage { th.accent } else { th.border };
            Ui::draw_rounded_rect(dot_x - 3.0, 214.0, 6.0, 6.0, 3.0, color);
        }
    }
}

fn draw_stage_card_transitions(stage_frame: u64) {
    let th = current_theme();
    let phase = (stage_frame as f32) / 150.0;
    let enter = ease_out_cubic((phase / 0.25).clamp(0.0, 1.0));
    let reveal = if phase >= 0.45 {
        ease_out_cubic(((phase - 0.45) / 0.25).clamp(0.0, 1.0))
    } else {
        0.0
    };
    let card_x = 46.0 + (1.0 - enter) * 32.0;
    draw_card(card_x, 62.0, 308.0, 136.0, th.border, th.surface_alt);
    Ui::fit_text(
        card_x + 154.0,
        82.0,
        0.54,
        280.0,
        scale_alpha(th.text, enter),
        tr("Sample question"),
    );
    if reveal > 0.01 {
        let div_w = 240.0 * reveal;
        Ui::draw_line(
            card_x + 154.0 - div_w * 0.5,
            116.0,
            card_x + 154.0 + div_w * 0.5,
            116.0,
            1.5,
            th.accent,
        );
        let ans_y = 132.0 + (1.0 - reveal) * 14.0;
        Ui::draw_rounded_rect(
            card_x + 34.0,
            ans_y,
            240.0,
            34.0,
            8.0,
            scale_alpha(th.cloze_bg, reveal),
        );
        Ui::fit_text(
            card_x + 154.0,
            ans_y + 8.0,
            0.50,
            220.0,
            scale_alpha(th.text, reveal),
            tr("Sample answer"),
        );
    }
}

fn draw_stage_button_feedback(stage_frame: u64) {
    let th = current_theme();
    let slot = ((stage_frame / 36) % 4) as usize;
    let local = (stage_frame % 36) as f32 / 36.0;
    let pulse = if local < 0.45 {
        (local / 0.45 * PI).sin()
    } else {
        0.0
    };
    let ratings = [
        (tr("Again"), th.again_bg, th.again_fg),
        (tr("Hard"), th.hard_bg, th.hard_fg),
        (tr("Good"), th.good_bg, th.good_fg),
        (tr("Easy"), th.easy_bg, th.easy_fg),
    ];
    for (i, &(label, bg, fg)) in ratings.iter().enumerate() {
        let active = i == slot && pulse > 0.01;
        let shift = if active { pulse } else { 0.0 };
        let bx = 26.0 + i as f32 * 88.0 + shift * 1.0;
        let by = 114.0 + shift * 2.0;
        let bw = 80.0 - shift * 2.0;
        let bh = 36.0 - shift * 2.0;
        if active {
            Ui::draw_rounded_rect(
                bx - 3.0,
                by - 3.0,
                bw + 6.0,
                bh + 6.0,
                11.0,
                scale_alpha(fg, pulse * 0.55),
            );
            draw_circle_outline(
                bx + bw * 0.5,
                by + bh * 0.5,
                10.0 + (local / 0.45) * 20.0,
                1.5,
                scale_alpha(fg, (1.0 - local / 0.45) * 0.65),
            );
        }
        Ui::draw_rounded_rect(bx, by + 1.5, bw, bh, 8.0, th.shadow);
        Ui::draw_rounded_rect(bx, by, bw, bh, 8.0, bg);
        Ui::fit_text(bx + bw * 0.5, by + 10.0, 0.44, bw - 8.0, fg, label);
    }
}

fn draw_stage_menu_transitions(stage_frame: u64) {
    let th = current_theme();
    let t = (stage_frame % 75) as f32 / 75.0;
    let ease = ease_out_cubic((t / 0.40).clamp(0.0, 1.0));
    let rem = 1.0 - ease;
    let page_b = (stage_frame / 75) % 2 == 1;
    let labels = if page_b {
        [tr("Menu"), tr("Decks"), tr("Browse"), tr("Settings")]
    } else {
        [tr("Settings"), tr("Statistics"), tr("Review"), tr("Menu")]
    };
    let panel_x = 68.0 + rem * 24.0;
    draw_card(panel_x, 62.0, 264.0, 136.0, th.border, th.surface_alt);
    Ui::draw_rect(
        panel_x + 1.0,
        63.0,
        262.0 * ease,
        2.5,
        scale_alpha(th.accent, 0.90),
    );
    let sel = ((stage_frame / 25) % 4) as usize;
    for (i, label) in labels.iter().enumerate() {
        let ry = 74.0 + i as f32 * 28.0;
        let is_sel = i == sel;
        Ui::draw_rounded_rect(
            panel_x + 12.0,
            ry,
            240.0,
            22.0,
            6.0,
            scale_alpha(
                if is_sel { th.tab_active_bg } else { th.surface },
                ease.max(0.2),
            ),
        );
        Ui::fit_text(
            panel_x + 132.0,
            ry + 4.0,
            0.44,
            220.0,
            scale_alpha(if is_sel { th.accent } else { th.text }, ease.max(0.2)),
            label,
        );
    }
}

fn draw_stage_deck_celebration(stage_frame: u64) {
    let th = current_theme();
    let secs = stage_frame as f32 / 60.0;
    let entry = ease_out_cubic((secs / 0.35).clamp(0.0, 1.0));
    draw_confetti(20.0, 56.0, 360.0, 160.0, secs, 16);
    draw_celebration_badge(200.0, 108.0, secs, entry);
    Ui::fit_text(
        200.0,
        148.0,
        0.56,
        320.0,
        th.accent,
        tr("Deck complete!"),
    );
    Ui::fit_text(
        200.0,
        176.0,
        0.42,
        320.0,
        th.text_muted,
        tr("No cards available now"),
    );
}
