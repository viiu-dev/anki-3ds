// Copyright: Anki 3DS contributors
// License: GNU AGPL, version 3 or later; http://www.gnu.org/licenses/agpl.html

use std::collections::BTreeMap;

use anki_proto::stats::graphs_response::buttons::ButtonCounts;
use anki_proto::stats::GraphsResponse;

use super::clock;
use super::i18n;
use super::i18n::tr;
use super::theme::current as current_theme;
use super::ui::Ui;

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Page {
    Overview,
    Forecast,
    Cards,
}

impl Page {
    pub const ALL: [Page; 3] = [Page::Overview, Page::Forecast, Page::Cards];

    pub fn label(self) -> &'static str {
        match self {
            Page::Overview => tr("Overview & Streak"),
            Page::Forecast => tr("Forecast & History"),
            Page::Cards => tr("Cards & Accuracy"),
        }
    }

    pub fn step(self, forward: bool) -> Self {
        let index = self as usize;
        let next = if forward {
            (index + 1) % Self::ALL.len()
        } else {
            (index + Self::ALL.len() - 1) % Self::ALL.len()
        };
        Self::ALL[next]
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Timeframe {
    OneMonth,
    ThreeMonths,
    OneYear,
    AllTime,
}

impl Timeframe {
    pub const ALL: [Timeframe; 4] = [
        Timeframe::OneMonth,
        Timeframe::ThreeMonths,
        Timeframe::OneYear,
        Timeframe::AllTime,
    ];

    pub fn label(self) -> &'static str {
        match self {
            Timeframe::OneMonth => tr("1 month"),
            Timeframe::ThreeMonths => tr("3 months"),
            Timeframe::OneYear => tr("1 year"),
            Timeframe::AllTime => tr("All time"),
        }
    }

    pub fn days(self) -> i32 {
        match self {
            Timeframe::OneMonth => 30,
            Timeframe::ThreeMonths => 90,
            Timeframe::OneYear => 365,
            Timeframe::AllTime => 0,
        }
    }

    pub fn step(self, forward: bool) -> Self {
        let index = self as usize;
        let next = if forward {
            (index + 1) % Self::ALL.len()
        } else {
            (index + Self::ALL.len() - 1) % Self::ALL.len()
        };
        Self::ALL[next]
    }
}

#[derive(Default, Clone, Copy)]
struct DayReviews {
    learn: u32,
    young: u32,
    mature: u32,
    filtered: u32,
}

impl DayReviews {
    fn total(self) -> u32 {
        self.learn + self.young + self.mature + self.filtered
    }
}

#[derive(Default, Clone, Copy)]
struct AccuracySet {
    learning: [u32; 4],
    young: [u32; 4],
    mature: [u32; 4],
}

impl AccuracySet {
    fn from_proto(proto: Option<&ButtonCounts>) -> Self {
        let Some(proto) = proto else {
            return Self::default();
        };
        Self {
            learning: to_buttons(&proto.learning),
            young: to_buttons(&proto.young),
            mature: to_buttons(&proto.mature),
        }
    }
}

fn to_buttons(values: &[u32]) -> [u32; 4] {
    [
        values.first().copied().unwrap_or(0),
        values.get(1).copied().unwrap_or(0),
        values.get(2).copied().unwrap_or(0),
        values.get(3).copied().unwrap_or(0),
    ]
}

#[derive(Default)]
pub struct CardStates {
    pub new_cards: u32,
    pub learning: u32,
    pub young: u32,
    pub mature: u32,
    pub suspended: u32,
    pub buried: u32,
}

impl CardStates {
    pub fn total(&self) -> u32 {
        self.new_cards + self.learning + self.young + self.mature + self.suspended + self.buried
    }
}

pub struct Dashboard {
    today_cards: u32,
    today_millis: u32,
    today_correct: u32,
    today_learn: u32,
    today_review: u32,
    today_relearn: u32,
    current_streak: u32,
    longest_streak: u32,
    reviews_by_day: BTreeMap<i32, DayReviews>,
    due_by_day: BTreeMap<i32, u32>,
    card_states: CardStates,
    accuracy: [AccuracySet; 4],
}

impl Dashboard {
    pub fn from_graphs(resp: GraphsResponse) -> Self {
        let today = resp.today.unwrap_or_default();
        let mut reviews_by_day = BTreeMap::new();
        if let Some(reviews) = resp.reviews {
            for (day, item) in reviews.count {
                if day > 0 {
                    continue;
                }
                let entry = DayReviews {
                    learn: item.learn + item.relearn,
                    young: item.young,
                    mature: item.mature,
                    filtered: item.filtered,
                };
                if entry.total() > 0 {
                    reviews_by_day.insert(day, entry);
                }
            }
        }
        let (current_streak, longest_streak) = compute_streaks(&reviews_by_day);

        let mut due_by_day = BTreeMap::new();
        if let Some(future) = resp.future_due {
            for (day, count) in future.future_due {
                if count > 0 {
                    *due_by_day.entry(day.max(0)).or_default() += count;
                }
            }
        }

        let counts = resp
            .card_counts
            .and_then(|c| c.excluding_inactive.or(c.including_inactive))
            .unwrap_or_default();
        let card_states = CardStates {
            new_cards: counts.new_cards,
            learning: counts.learn + counts.relearn,
            young: counts.young,
            mature: counts.mature,
            suspended: counts.suspended,
            buried: counts.buried,
        };

        let buttons = resp.buttons.unwrap_or_default();
        let accuracy = [
            AccuracySet::from_proto(buttons.one_month.as_ref()),
            AccuracySet::from_proto(buttons.three_months.as_ref()),
            AccuracySet::from_proto(buttons.one_year.as_ref()),
            AccuracySet::from_proto(buttons.all_time.as_ref()),
        ];

        Self {
            today_cards: today.answer_count,
            today_millis: today.answer_millis,
            today_correct: today.correct_count,
            today_learn: today.learn_count,
            today_review: today.review_count,
            today_relearn: today.relearn_count,
            current_streak,
            longest_streak,
            reviews_by_day,
            due_by_day,
            card_states,
            accuracy,
        }
    }
}

fn compute_streaks(reviews: &BTreeMap<i32, DayReviews>) -> (u32, u32) {
    let has_day = |d: i32| reviews.get(&d).is_some_and(|r| r.total() > 0);
    let mut current = 0u32;
    let mut cursor = if has_day(0) {
        Some(0)
    } else if has_day(-1) {
        Some(-1)
    } else {
        None
    };
    while let Some(d) = cursor {
        if has_day(d) {
            current += 1;
            cursor = Some(d - 1);
        } else {
            break;
        }
    }

    let mut longest = 0u32;
    let mut run = 0u32;
    let mut prev: Option<i32> = None;
    for (&day, entry) in reviews {
        if entry.total() == 0 {
            continue;
        }
        if prev.is_some_and(|p| day == p + 1) {
            run += 1;
        } else {
            run = 1;
        }
        longest = longest.max(run);
        prev = Some(day);
    }

    (current, longest)
}

fn mix(a: u32, b: u32, t: f32) -> u32 {
    let t = t.clamp(0.0, 1.0);
    let ar = (a & 0xff) as f32;
    let ag = ((a >> 8) & 0xff) as f32;
    let ab = ((a >> 16) & 0xff) as f32;
    let br = (b & 0xff) as f32;
    let bg = ((b >> 8) & 0xff) as f32;
    let bb = ((b >> 16) & 0xff) as f32;
    let r = (ar + (br - ar) * t).round() as u32;
    let g = (ag + (bg - ag) * t).round() as u32;
    let bl = (ab + (bb - ab) * t).round() as u32;
    (r & 0xff) | ((g & 0xff) << 8) | ((bl & 0xff) << 16) | (255u32 << 24)
}

fn draw_card(x: f32, y: f32, w: f32, h: f32) {
    let th = current_theme();
    Ui::draw_rounded_rect(x, y, w, h, 8.0, th.border);
    Ui::draw_rounded_rect(x + 1.0, y + 1.0, w - 2.0, h - 2.0, 7.0, th.surface);
}

fn format_duration(ms: u32) -> String {
    let secs = (ms + 500) / 1000;
    if secs < 60 {
        format!("{secs}s")
    } else if secs < 3600 {
        format!("{}m", (secs + 30) / 60)
    } else {
        format!("{:.1}h", secs as f32 / 3600.0)
    }
}

pub fn draw_top(dashboard: &Dashboard, page: Page, timeframe: Timeframe) {
    let th = current_theme();
    Ui::draw_rect(0.0, 0.0, 400.0, 24.0, th.surface_alt);
    Ui::draw_line(0.0, 24.0, 400.0, 24.0, 1.0, th.border);
    Ui::draw_text(
        10.0,
        5.0,
        0.48,
        th.text,
        &Ui::ellipsize(page.label(), 0.48, 250.0),
    );
    Ui::draw_text_right(390.0, 5.0, 0.44, th.accent, timeframe.label());

    match page {
        Page::Overview => draw_overview(dashboard, timeframe),
        Page::Forecast => draw_forecast(dashboard, timeframe),
        Page::Cards => draw_cards(dashboard, timeframe),
    }
}

fn draw_overview(dashboard: &Dashboard, timeframe: Timeframe) {
    let th = current_theme();

    draw_card(10.0, 29.0, 200.0, 68.0);
    Ui::draw_text(
        18.0,
        34.0,
        0.42,
        th.text_muted,
        &Ui::ellipsize(tr("Reviewed today"), 0.42, 184.0),
    );
    let cards_str = dashboard.today_cards.to_string();
    let dur_str = format_duration(dashboard.today_millis);
    Ui::draw_text(
        18.0,
        50.0,
        0.52,
        th.text,
        &Ui::ellipsize(
            &i18n::format("{0} cards ({1})", &[&cards_str, &dur_str]),
            0.52,
            184.0,
        ),
    );
    let pct = if dashboard.today_cards > 0 {
        ((dashboard.today_correct as f32 / dashboard.today_cards as f32) * 100.0).round() as u32
    } else {
        0
    };
    let acc_str = i18n::format("Correct: {0}%", &[&pct.to_string()]);
    Ui::draw_text(18.0, 72.0, 0.40, th.review, &acc_str);
    let types = format!(
        "L{} R{} +{}",
        dashboard.today_learn, dashboard.today_review, dashboard.today_relearn
    );
    Ui::draw_text_right(202.0, 72.0, 0.38, th.text_muted, &types);

    draw_card(216.0, 29.0, 174.0, 68.0);
    Ui::draw_text(
        224.0,
        34.0,
        0.42,
        th.text_muted,
        &Ui::ellipsize(tr("Daily streak"), 0.42, 158.0),
    );
    Ui::draw_text(
        224.0,
        51.0,
        0.50,
        if dashboard.current_streak > 0 {
            th.accent
        } else {
            th.text
        },
        &Ui::ellipsize(
            &i18n::format("Current: {0}d", &[&dashboard.current_streak.to_string()]),
            0.50,
            158.0,
        ),
    );
    Ui::draw_text(
        224.0,
        72.0,
        0.42,
        th.text_muted,
        &Ui::ellipsize(
            &i18n::format("Longest: {0}d", &[&dashboard.longest_streak.to_string()]),
            0.42,
            158.0,
        ),
    );

    draw_card(10.0, 103.0, 380.0, 130.0);
    let cutoff = match timeframe.days() {
        0 => i32::MIN,
        d => -(d - 1),
    };
    let mut period_reviews = 0u32;
    let mut active_days = 0u32;
    for (&day, entry) in &dashboard.reviews_by_day {
        if day >= cutoff && day <= 0 {
            let t = entry.total();
            if t > 0 {
                period_reviews += t;
                active_days += 1;
            }
        }
    }
    Ui::draw_text(18.0, 109.0, 0.44, th.text, tr("Activity heatmap"));
    let summary = i18n::format(
        "{0} reviews, {1} days",
        &[&period_reviews.to_string(), &active_days.to_string()],
    );
    Ui::draw_text_right(382.0, 110.0, 0.39, th.text_muted, &summary);

    let cols: usize = match timeframe {
        Timeframe::OneMonth => 5,
        Timeframe::ThreeMonths => 13,
        Timeframe::OneYear | Timeframe::AllTime => 52,
    };
    let grid_w = 364.0f32;
    let step_x = (grid_w / cols as f32).clamp(7.0, 28.0);
    let cell_w = (step_x - 2.0).max(4.5);
    let total_w = (cols as f32 - 1.0) * step_x + cell_w;
    let start_x = 18.0 + (grid_w - total_w) * 0.5;
    let start_y = 132.0;
    let step_y = 10.5;
    let cell_h = 8.5;

    // Unix epoch 1970-01-01 was Thursday (offset +4 aligns Sun=0..Sat=6).
    let today_row = (clock::local_time().div_euclid(86_400) + 4).rem_euclid(7) as i32;
    let mut max_day = 1u32;
    for col in 0..cols {
        for row in 0..7 {
            let offset = (col as i32 - (cols as i32 - 1)) * 7 + (row as i32 - today_row);
            if offset <= 0 {
                if let Some(entry) = dashboard.reviews_by_day.get(&offset) {
                    max_day = max_day.max(entry.total());
                }
            }
        }
    }

    for col in 0..cols {
        for row in 0..7 {
            let offset = (col as i32 - (cols as i32 - 1)) * 7 + (row as i32 - today_row);
            if offset > 0 {
                continue;
            }
            let count = dashboard
                .reviews_by_day
                .get(&offset)
                .map_or(0, |e| e.total());
            let color = if count == 0 {
                th.surface_alt
            } else {
                let ratio = (count as f32 / max_day as f32).sqrt();
                let level = if ratio < 0.25 {
                    0.35
                } else if ratio < 0.55 {
                    0.58
                } else if ratio < 0.80 {
                    0.80
                } else {
                    1.0
                };
                mix(th.surface_alt, th.review, level)
            };
            let cx = start_x + col as f32 * step_x;
            let cy = start_y + row as f32 * step_y;
            if offset == 0 {
                Ui::draw_rect(cx - 1.0, cy - 1.0, cell_w + 2.0, cell_h + 2.0, th.accent);
            }
            Ui::draw_rect(cx, cy, cell_w, cell_h, color);
        }
    }

    let left_label = match timeframe {
        Timeframe::OneMonth => "-30d",
        Timeframe::ThreeMonths => "-90d",
        Timeframe::OneYear | Timeframe::AllTime => "-365d",
    };
    Ui::draw_text(18.0, 212.0, 0.37, th.text_muted, left_label);
    for (i, level) in [0.0f32, 0.35, 0.58, 0.80, 1.0].iter().enumerate() {
        let sx = 166.0 + i as f32 * 13.0;
        let color = if *level == 0.0 {
            th.surface_alt
        } else {
            mix(th.surface_alt, th.review, *level)
        };
        Ui::draw_rect(sx, 213.0, 9.0, 9.0, color);
    }
    Ui::draw_text_right(382.0, 212.0, 0.37, th.text_muted, tr("Today"));
}

fn draw_forecast(dashboard: &Dashboard, timeframe: Timeframe) {
    let th = current_theme();

    let future_span = match timeframe.days() {
        0 => dashboard
            .due_by_day
            .keys()
            .copied()
            .max()
            .map_or(30, |d| (d + 1).clamp(30, 3650)),
        d => d,
    };
    let future_total: u32 = dashboard
        .due_by_day
        .iter()
        .filter(|(&d, _)| d >= 0 && d < future_span)
        .map(|(_, &c)| c)
        .sum();
    let future_avg = future_total as f32 / future_span.max(1) as f32;
    let future_sub = i18n::format(
        "Total: {0}  Avg: {1}/d",
        &[&future_total.to_string(), &format!("{future_avg:.1}")],
    );
    let mut future_bins = [0u32; 30];
    for (&day, &count) in &dashboard.due_by_day {
        if day >= 0 && day < future_span {
            let idx = ((day as usize) * 30 / future_span as usize).min(29);
            future_bins[idx] += count;
        }
    }
    draw_bar_card(
        10.0,
        28.0,
        380.0,
        100.0,
        tr("Future due"),
        &future_sub,
        &future_bins,
        th.accent,
        tr("Today"),
        &format!("+{future_span}d"),
    );

    let history_span = match timeframe.days() {
        0 => dashboard
            .reviews_by_day
            .keys()
            .copied()
            .min()
            .map_or(30, |d| (-d + 1).max(30)),
        d => d,
    };
    let mut history_total = 0u32;
    let mut hist_learn = [0u32; 30];
    let mut hist_young = [0u32; 30];
    let mut hist_mature = [0u32; 30];
    let oldest = -(history_span - 1);
    for (&day, entry) in &dashboard.reviews_by_day {
        if day >= oldest && day <= 0 {
            let total = entry.total();
            history_total += total;
            let pos = (day - oldest) as usize;
            let idx = (pos * 30 / history_span as usize).min(29);
            hist_learn[idx] += entry.learn + entry.filtered;
            hist_young[idx] += entry.young;
            hist_mature[idx] += entry.mature;
        }
    }
    let history_avg = history_total as f32 / history_span.max(1) as f32;
    let history_sub = i18n::format(
        "Total: {0}  Avg: {1}/d",
        &[&history_total.to_string(), &format!("{history_avg:.1}")],
    );
    draw_stacked_bar_card(
        10.0,
        134.0,
        380.0,
        100.0,
        tr("Review history"),
        &history_sub,
        &hist_mature,
        &hist_young,
        &hist_learn,
        &format!("-{history_span}d"),
        tr("Today"),
    );
}

fn draw_bar_card(
    x: f32,
    y: f32,
    w: f32,
    h: f32,
    title: &str,
    sub: &str,
    bins: &[u32; 30],
    bar_color: u32,
    x_left: &str,
    x_right: &str,
) {
    let th = current_theme();
    draw_card(x, y, w, h);
    Ui::draw_text(
        x + 8.0,
        y + 5.0,
        0.44,
        th.text,
        &Ui::ellipsize(title, 0.44, 150.0),
    );
    Ui::draw_text_right(x + w - 8.0, y + 6.0, 0.39, th.text_muted, sub);

    let px = x + 36.0;
    let py = y + 25.0;
    let pw = w - 46.0;
    let ph = h - 42.0;
    let max_val = bins.iter().copied().max().unwrap_or(0).max(1);

    for step in [0.0f32, 0.5, 1.0] {
        let gy = py + ph * (1.0 - step);
        Ui::draw_line(px, gy, px + pw, gy, 1.0, th.border);
    }
    Ui::draw_text_right(px - 4.0, py - 3.0, 0.35, th.text_muted, &max_val.to_string());
    Ui::draw_text_right(
        px - 4.0,
        py + ph * 0.5 - 4.0,
        0.35,
        th.text_muted,
        &(max_val / 2).to_string(),
    );
    Ui::draw_text_right(px - 4.0, py + ph - 7.0, 0.35, th.text_muted, "0");

    let slot = pw / bins.len() as f32;
    let bw = (slot - 2.0).max(3.0);
    for (i, &val) in bins.iter().enumerate() {
        if val == 0 {
            continue;
        }
        let bh = ((val as f32 / max_val as f32) * ph).max(2.0);
        let bx = px + i as f32 * slot + (slot - bw) * 0.5;
        Ui::draw_rect(bx, py + ph - bh, bw, bh, bar_color);
    }

    Ui::draw_text(px, py + ph + 3.0, 0.36, th.text_muted, x_left);
    Ui::draw_text_right(px + pw, py + ph + 3.0, 0.36, th.text_muted, x_right);
}

#[allow(clippy::too_many_arguments)]
fn draw_stacked_bar_card(
    x: f32,
    y: f32,
    w: f32,
    h: f32,
    title: &str,
    sub: &str,
    mature: &[u32; 30],
    young: &[u32; 30],
    learn: &[u32; 30],
    x_left: &str,
    x_right: &str,
) {
    let th = current_theme();
    draw_card(x, y, w, h);
    Ui::draw_text(
        x + 8.0,
        y + 5.0,
        0.44,
        th.text,
        &Ui::ellipsize(title, 0.44, 150.0),
    );
    Ui::draw_text_right(x + w - 8.0, y + 6.0, 0.39, th.text_muted, sub);

    let px = x + 36.0;
    let py = y + 25.0;
    let pw = w - 46.0;
    let ph = h - 42.0;
    let max_val = (0..30)
        .map(|i| mature[i] + young[i] + learn[i])
        .max()
        .unwrap_or(0)
        .max(1);

    for step in [0.0f32, 0.5, 1.0] {
        let gy = py + ph * (1.0 - step);
        Ui::draw_line(px, gy, px + pw, gy, 1.0, th.border);
    }
    Ui::draw_text_right(px - 4.0, py - 3.0, 0.35, th.text_muted, &max_val.to_string());
    Ui::draw_text_right(
        px - 4.0,
        py + ph * 0.5 - 4.0,
        0.35,
        th.text_muted,
        &(max_val / 2).to_string(),
    );
    Ui::draw_text_right(px - 4.0, py + ph - 7.0, 0.35, th.text_muted, "0");

    let slot = pw / 30.0;
    let bw = (slot - 2.0).max(3.0);
    for i in 0..30 {
        let total = mature[i] + young[i] + learn[i];
        if total == 0 {
            continue;
        }
        let total_h = ((total as f32 / max_val as f32) * ph).max(2.0);
        let bx = px + i as f32 * slot + (slot - bw) * 0.5;
        let mut cursor_y = py + ph;
        for &(val, color) in &[
            (mature[i], th.review),
            (young[i], th.new),
            (learn[i], th.learn),
        ] {
            if val == 0 {
                continue;
            }
            let seg_h = (val as f32 / total as f32) * total_h;
            cursor_y -= seg_h;
            Ui::draw_rect(bx, cursor_y, bw, seg_h.max(1.0), color);
        }
    }

    Ui::draw_text(px, py + ph + 3.0, 0.36, th.text_muted, x_left);
    Ui::draw_text_right(px + pw, py + ph + 3.0, 0.36, th.text_muted, x_right);
}

fn draw_cards(dashboard: &Dashboard, timeframe: Timeframe) {
    let th = current_theme();
    let cs = &dashboard.card_states;
    let total = cs.total();

    draw_card(10.0, 28.0, 380.0, 100.0);
    Ui::draw_text(18.0, 33.0, 0.44, th.text, tr("Card states"));
    Ui::draw_text_right(
        382.0,
        34.0,
        0.40,
        th.text_muted,
        &i18n::format("{0}: {1}", &[tr("Total"), &total.to_string()]),
    );

    let young_color = mix(th.review, th.new, 0.45);
    let segments = [
        (tr("New"), cs.new_cards, th.new),
        (tr("Learning"), cs.learning, th.learn),
        (tr("Young"), cs.young, young_color),
        (tr("Mature"), cs.mature, th.review),
        (tr("Suspended"), cs.suspended, th.hard_fg),
        (tr("Buried"), cs.buried, th.text_muted),
    ];

    let bar_x = 18.0;
    let bar_y = 51.0;
    let bar_w = 364.0;
    let bar_h = 13.0;
    Ui::draw_rect(bar_x, bar_y, bar_w, bar_h, th.surface_alt);
    if total > 0 {
        let mut cx = bar_x;
        let remaining_total = total as f32;
        for (i, &(_, count, color)) in segments.iter().enumerate() {
            if count == 0 {
                continue;
            }
            let seg_w = if i == segments.len() - 1
                || segments[i + 1..].iter().all(|s| s.1 == 0)
            {
                (bar_x + bar_w - cx).max(0.0)
            } else {
                ((count as f32 / remaining_total) * bar_w).max(2.0).min(bar_x + bar_w - cx)
            };
            Ui::draw_rect(cx, bar_y, seg_w, bar_h, color);
            cx += seg_w;
        }
    }

    for (index, &(label, count, color)) in segments.iter().enumerate() {
        let col = index % 3;
        let row = index / 3;
        let lx = 18.0 + col as f32 * 123.0;
        let ly = 72.0 + row as f32 * 24.0;
        Ui::draw_rect(lx, ly + 3.0, 8.0, 8.0, color);
        Ui::draw_text(
            lx + 12.0,
            ly + 1.0,
            0.39,
            th.text,
            &Ui::ellipsize(label, 0.39, 102.0),
        );
        let pct = if total > 0 {
            (count as f32 / total as f32) * 100.0
        } else {
            0.0
        };
        Ui::draw_text(
            lx + 12.0,
            ly + 12.0,
            0.36,
            th.text_muted,
            &format!("{count} ({pct:.0}%)"),
        );
    }

    draw_card(10.0, 134.0, 380.0, 100.0);
    Ui::draw_text(
        18.0,
        139.0,
        0.44,
        th.text,
        &Ui::ellipsize(tr("Answer accuracy"), 0.44, 150.0),
    );
    let btn_colors = [th.again_fg, th.hard_fg, th.good_fg, th.easy_fg];
    let btn_labels = [tr("Again"), tr("Hard"), tr("Good"), tr("Easy")];
    let mut lx = 382.0;
    for i in (0..4).rev() {
        let lbl = btn_labels[i];
        let tw = Ui::get_text_width(0.35, lbl);
        lx -= tw;
        Ui::draw_text(lx, 140.0, 0.35, th.text_muted, lbl);
        lx -= 10.0;
        Ui::draw_rect(lx, 142.0, 7.0, 7.0, btn_colors[i]);
        lx -= 7.0;
    }

    let set = dashboard.accuracy[timeframe as usize];
    for (row_idx, (label, counts)) in [
        (tr("Learning"), set.learning),
        (tr("Young"), set.young),
        (tr("Mature"), set.mature),
    ]
    .iter()
    .enumerate()
    {
        let ry = 162.0 + row_idx as f32 * 23.0;
        Ui::draw_text(
            18.0,
            ry + 1.0,
            0.41,
            th.text,
            &Ui::ellipsize(label, 0.41, 74.0),
        );
        let bx = 96.0;
        let bw = 188.0;
        let bh = 12.0;
        Ui::draw_rect(bx, ry + 2.0, bw, bh, th.surface_alt);
        let row_total: u32 = counts.iter().sum();
        if row_total > 0 {
            let mut cx = bx;
            for (bi, &cnt) in counts.iter().enumerate() {
                if cnt == 0 {
                    continue;
                }
                let sw = if counts[bi + 1..].iter().all(|&v| v == 0) {
                    (bx + bw - cx).max(0.0)
                } else {
                    ((cnt as f32 / row_total as f32) * bw).max(2.0).min(bx + bw - cx)
                };
                Ui::draw_rect(cx, ry + 2.0, sw, bh, btn_colors[bi]);
                cx += sw;
            }
            let correct = counts[1] + counts[2] + counts[3];
            let acc = (correct as f32 / row_total as f32) * 100.0;
            Ui::draw_text_right(
                382.0,
                ry + 2.0,
                0.38,
                th.text,
                &format!("{acc:.1}% ({correct}/{row_total})"),
            );
        } else {
            Ui::draw_text_right(382.0, ry + 2.0, 0.38, th.text_muted, "-");
        }
    }
}

pub fn draw_bottom(page: Page, timeframe: Timeframe) {
    let th = current_theme();
    Ui::draw_rect(0.0, 0.0, 320.0, 42.0, th.surface_alt);
    Ui::draw_line(0.0, 42.0, 320.0, 42.0, 1.0, th.border);
    Ui::fit_text(160.0, 12.0, 0.60, 296.0, th.text, tr("Statistics"));

    for (index, &item) in Page::ALL.iter().enumerate() {
        let y = 46.0 + index as f32 * 25.0;
        let style = if item == page { 0 } else { 6 };
        Ui::draw_button(12.0, y, 296.0, 22.0, item.label(), None, style, false);
    }

    Ui::draw_text(14.0, 123.0, 0.42, th.text_muted, tr("Timeframe"));
    for (index, &item) in Timeframe::ALL.iter().enumerate() {
        let col = index % 2;
        let row = index / 2;
        let x = 12.0 + col as f32 * 152.0;
        let y = 138.0 + row as f32 * 28.0;
        let style = if item == timeframe { 0 } else { 6 };
        Ui::draw_button(x, y, 144.0, 24.0, item.label(), None, style, false);
    }

    Ui::draw_button(60.0, 198.0, 200.0, 28.0, tr("Back [B]"), None, 6, false);
}

pub fn touch_hit(x: f32, y: f32) -> TouchAction {
    if (46.0..121.0).contains(&y) && (12.0..308.0).contains(&x) {
        let index = ((y - 46.0) / 25.0) as usize;
        let offset_in_row = (y - 46.0) % 25.0;
        if index < Page::ALL.len() && offset_in_row < 22.0 {
            return TouchAction::Page(Page::ALL[index]);
        }
    }
    for (index, &item) in Timeframe::ALL.iter().enumerate() {
        let col = index % 2;
        let row = index / 2;
        let bx = 12.0 + col as f32 * 152.0;
        let by = 138.0 + row as f32 * 28.0;
        if x >= bx && x < bx + 144.0 && y >= by && y < by + 24.0 {
            return TouchAction::Timeframe(item);
        }
    }
    if (198.0..226.0).contains(&y) && (60.0..260.0).contains(&x) {
        return TouchAction::Back;
    }
    TouchAction::None
}

pub enum TouchAction {
    None,
    Page(Page),
    Timeframe(Timeframe),
    Back,
}
