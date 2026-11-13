// Copyright: Anki 3DS contributors
// License: GNU AGPL, version 3 or later; http://www.gnu.org/licenses/agpl.html

use std::f32::consts::{PI, TAU};
use std::sync::OnceLock;

use super::i18n;
use super::ui::Ui;

const SAMPLE_RATE: i32 = 22050;

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum SfxKind {
    Reveal,
    GoodEasy,
    AgainHard,
    DeckComplete,
}

static PCM_REVEAL: OnceLock<Vec<i16>> = OnceLock::new();
static PCM_GOOD_EASY: OnceLock<Vec<i16>> = OnceLock::new();
static PCM_AGAIN_HARD: OnceLock<Vec<i16>> = OnceLock::new();
static PCM_DECK_COMPLETE: OnceLock<Vec<i16>> = OnceLock::new();

fn to_i16(sample: f32) -> i16 {
    (sample.clamp(-1.0, 1.0) * 32767.0).round() as i16
}

fn end_fade(t: f32, total: f32, fade: f32) -> f32 {
    if t >= total {
        0.0
    } else if t > total - fade {
        ((total - t) / fade).clamp(0.0, 1.0)
    } else {
        1.0
    }
}

fn bell_note(t: f32, start: f32, freq: f32, decay: f32, harm2: f32, harm3: f32) -> f32 {
    let dt = t - start;
    if dt < 0.0 {
        return 0.0;
    }
    let attack = (dt / 0.005).clamp(0.0, 1.0);
    let env = attack * attack * (-dt * decay).exp();
    let p = TAU * freq * dt;
    (p.sin() + harm2 * (p * 2.0).sin() + harm3 * (p * 3.0).sin()) * env
}

fn synth_reveal() -> Vec<i16> {
    let total = 0.045;
    let count = (total * SAMPLE_RATE as f32) as usize;
    let mut out = Vec::with_capacity(count);
    let mut phase = 0.0f32;
    for i in 0..count {
        let t = i as f32 / SAMPLE_RATE as f32;
        let freq = 660.0 - 350.0 * (1.0 - (-t * 48.0).exp());
        phase += TAU * freq / SAMPLE_RATE as f32;
        let attack = if t < 0.0025 {
            0.5 * (1.0 - (PI * t / 0.0025).cos())
        } else {
            1.0
        };
        let env = attack * (-t * 95.0).exp() * end_fade(t, total, 0.005);
        let sample = (phase.sin() + 0.18 * (phase * 2.0).sin()) * env * 0.36;
        out.push(to_i16(sample));
    }
    out
}

fn synth_good_easy() -> Vec<i16> {
    let total = 0.230;
    let count = (total * SAMPLE_RATE as f32) as usize;
    let mut out = Vec::with_capacity(count);
    for i in 0..count {
        let t = i as f32 / SAMPLE_RATE as f32;
        let n1 = bell_note(t, 0.0, 523.25, 16.0, 0.28, 0.10);
        let n2 = bell_note(t, 0.055, 659.25, 13.5, 0.28, 0.10);
        let n3 = bell_note(t, 0.055, 783.99, 14.5, 0.20, 0.06) * 0.35;
        let sample = (n1 * 0.8 + n2 + n3) * end_fade(t, total, 0.015) * 0.34;
        out.push(to_i16(sample));
    }
    out
}

fn synth_again_hard() -> Vec<i16> {
    let total = 0.165;
    let count = (total * SAMPLE_RATE as f32) as usize;
    let mut out = Vec::with_capacity(count);
    for i in 0..count {
        let t = i as f32 / SAMPLE_RATE as f32;
        let n1 = bell_note(t, 0.0, 261.63, 20.0, 0.15, 0.0) * 0.75;
        let n2 = bell_note(t, 0.050, 220.00, 16.5, 0.15, 0.0);
        let sample = (n1 + n2) * end_fade(t, total, 0.015) * 0.36;
        out.push(to_i16(sample));
    }
    out
}

fn synth_deck_complete() -> Vec<i16> {
    let total = 0.500;
    let count = (total * SAMPLE_RATE as f32) as usize;
    let mut out = Vec::with_capacity(count);
    for i in 0..count {
        let t = i as f32 / SAMPLE_RATE as f32;
        let n1 = bell_note(t, 0.000, 523.25, 11.0, 0.26, 0.08) * 0.75;
        let n2 = bell_note(t, 0.080, 659.25, 10.5, 0.26, 0.08) * 0.85;
        let n3 = bell_note(t, 0.160, 783.99, 10.0, 0.26, 0.08) * 0.90;
        let n4 = bell_note(t, 0.240, 1046.50, 7.5, 0.22, 0.06);
        let pad = (bell_note(t, 0.240, 659.25, 7.5, 0.14, 0.0)
            + bell_note(t, 0.240, 783.99, 7.5, 0.14, 0.0))
            * 0.35;
        let sample = (n1 + n2 + n3 + n4 + pad) * end_fade(t, total, 0.025) * 0.32;
        out.push(to_i16(sample));
    }
    out
}

fn samples(kind: SfxKind) -> &'static [i16] {
    match kind {
        SfxKind::Reveal => PCM_REVEAL.get_or_init(synth_reveal),
        SfxKind::GoodEasy => PCM_GOOD_EASY.get_or_init(synth_good_easy),
        SfxKind::AgainHard => PCM_AGAIN_HARD.get_or_init(synth_again_hard),
        SfxKind::DeckComplete => PCM_DECK_COMPLETE.get_or_init(synth_deck_complete),
    }
}

pub fn play(kind: SfxKind) {
    if i18n::ui_sfx() {
        Ui::sfx_play_pcm(samples(kind), SAMPLE_RATE, i18n::sfx_volume());
    }
}

pub fn play_rating(rating: i32) {
    if rating <= 1 {
        play(SfxKind::AgainHard);
    } else {
        play(SfxKind::GoodEasy);
    }
}

pub fn play_preview() {
    Ui::sfx_play_pcm(samples(SfxKind::GoodEasy), SAMPLE_RATE, i18n::sfx_volume());
}
