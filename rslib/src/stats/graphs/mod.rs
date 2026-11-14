// Copyright: Anki Software, LLC and contributors
// License: GNU AGPL, version 3 or later; http://www.gnu.org/licenses/agpl.html

mod added;
mod buttons;
mod card_counts;
mod eases;
mod future_due;
mod hours;
mod intervals;
mod retention;
mod retrievability;
mod reviews;
mod today;

use crate::config::BoolKey;
use crate::config::Weekday;
use crate::prelude::*;
use crate::revlog::RevlogEntry;
use crate::search::SortMode;

struct GraphsContext {
    revlog: Vec<RevlogEntry>,
    cards: Vec<Card>,
    next_day_start: TimestampSecs,
    days_elapsed: u32,
    local_offset_secs: i64,
}

impl Collection {
    pub(crate) fn graph_data_for_search(
        &mut self,
        search: &str,
        days: u32,
    ) -> Result<anki_proto::stats::GraphsResponse> {
        let guard = self.search_cards_into_table(search, SortMode::NoOrder)?;
        let all = search.trim().is_empty();
        guard.col.graph_data(all, days)
    }

    fn graph_data(&mut self, all: bool, days: u32) -> Result<anki_proto::stats::GraphsResponse> {
        let timing = self.timing_today()?;
        let revlog_start = if days > 0 {
            timing
                .next_day_at
                .adding_secs(-(((days as i64) + 1) * 86_400))
        } else {
            TimestampSecs(0)
        };
        let offset = self.local_utc_offset_for_user()?;
        let local_offset_secs = offset.local_minus_utc() as i64;
        let revlog = if all {
            self.storage.get_all_revlog_entries(revlog_start)?
        } else {
            self.storage
                .get_revlog_entries_for_searched_cards_after_stamp(revlog_start)?
        };
        let ctx = GraphsContext {
            revlog,
            days_elapsed: timing.days_elapsed,
            cards: self.storage.all_searched_cards()?,
            next_day_start: timing.next_day_at,
            local_offset_secs,
        };
        let (eases, difficulty) = ctx.eases();
        let resp = anki_proto::stats::GraphsResponse {
            added: Some(ctx.added_days()),
            reviews: Some(ctx.review_counts_and_times()),
            true_retention: Some(ctx.calculate_true_retention()),
            future_due: Some(ctx.future_due()),
            intervals: Some(ctx.intervals()),
            stability: Some(ctx.stability()),
            eases: Some(eases),
            difficulty: Some(difficulty),
            today: Some(ctx.today()),
            hours: Some(ctx.hours()),
            buttons: Some(ctx.buttons()),
            card_counts: Some(ctx.card_counts()),
            rollover_hour: self.rollover_for_current_scheduler()? as u32,
            retrievability: Some(ctx.retrievability()),
            fsrs: self.fsrs_enabled(),
        };
        Ok(resp)
    }

    pub(crate) fn get_graph_preferences(&self) -> anki_proto::stats::GraphPreferences {
        anki_proto::stats::GraphPreferences {
            calendar_first_day_of_week: self.get_first_day_of_week() as i32,
            card_counts_separate_inactive: self
                .get_config_bool(BoolKey::CardCountsSeparateInactive),
            browser_links_supported: true,
            future_due_show_backlog: self.get_config_bool(BoolKey::FutureDueShowBacklog),
        }
    }

    pub(crate) fn set_graph_preferences(
        &mut self,
        prefs: anki_proto::stats::GraphPreferences,
    ) -> Result<()> {
        self.set_first_day_of_week(match prefs.calendar_first_day_of_week {
            1 => Weekday::Monday,
            5 => Weekday::Friday,
            6 => Weekday::Saturday,
            _ => Weekday::Sunday,
        })?;
        self.set_config_bool_inner(
            BoolKey::CardCountsSeparateInactive,
            prefs.card_counts_separate_inactive,
        )?;
        self.set_config_bool_inner(BoolKey::FutureDueShowBacklog, prefs.future_due_show_backlog)?;
        Ok(())
    }

    pub fn dashboard_graphs(&mut self) -> Result<anki_proto::stats::GraphsResponse> {
        use anki_proto::stats::graphs_response::buttons::ButtonCounts;
        use anki_proto::stats::graphs_response::card_counts::Counts;
        use anki_proto::stats::graphs_response::Buttons;
        use anki_proto::stats::graphs_response::CardCounts;
        use anki_proto::stats::graphs_response::FutureDue;
        use anki_proto::stats::graphs_response::ReviewCountsAndTimes;
        use anki_proto::stats::graphs_response::Today;

        use crate::card::CardQueue;
        use crate::card::CardType;
        use crate::revlog::RevlogReviewKind;
        use crate::scheduler::timing::is_unix_epoch_timestamp;

        let timing = self.timing_today()?;
        let next_day_start = timing.next_day_at;
        let days_elapsed = timing.days_elapsed as i32;
        let start_of_today_ms = next_day_start.adding_secs(-86_400).as_millis().0;
        let stamp_1y = next_day_start.adding_secs(-86_400 * 365);
        let stamp_3m = next_day_start.adding_secs(-86_400 * 90);
        let stamp_1m = next_day_start.adding_secs(-86_400 * 30);

        let mut today = Today::default();
        let mut reviews = ReviewCountsAndTimes::default();
        let empty_buttons = || ButtonCounts {
            learning: vec![0; 4],
            young: vec![0; 4],
            mature: vec![0; 4],
        };
        let mut btn_all = empty_buttons();
        let mut btn_1y = empty_buttons();
        let mut btn_3m = empty_buttons();
        let mut btn_1m = empty_buttons();

        {
            let mut stmt = self.storage.db.prepare_cached(
                "SELECT id, ease, cast(lastIvl AS integer), time, type FROM revlog",
            )?;
            let mut rows = stmt.query([])?;
            while let Some(row) = rows.next()? {
                let review_kind: RevlogReviewKind = row.get(4).unwrap_or_default();
                if matches!(
                    review_kind,
                    RevlogReviewKind::Manual | RevlogReviewKind::Rescheduled
                ) {
                    continue;
                }
                let id_ms: i64 = row.get(0)?;
                let button_chosen: u8 = row.get(1)?;
                let last_interval: i32 = row.get(2)?;
                let taken_millis: u32 = row.get(3).unwrap_or_default();

                if id_ms >= start_of_today_ms {
                    today.answer_count += 1;
                    today.answer_millis = today.answer_millis.saturating_add(taken_millis);
                    if button_chosen > 1 {
                        today.correct_count += 1;
                    }
                    if last_interval >= 21 {
                        today.mature_count += 1;
                        if button_chosen > 1 {
                            today.mature_correct += 1;
                        }
                    }
                    match review_kind {
                        RevlogReviewKind::Learning => today.learn_count += 1,
                        RevlogReviewKind::Review => today.review_count += 1,
                        RevlogReviewKind::Relearning => today.relearn_count += 1,
                        RevlogReviewKind::Filtered => today.early_review_count += 1,
                        RevlogReviewKind::Manual | RevlogReviewKind::Rescheduled => {}
                    }
                }

                let review_secs = TimestampSecs(id_ms / 1000);
                let day = (review_secs.elapsed_secs_since(next_day_start) / 86_400) as i32;
                let count = reviews.count.entry(day).or_default();
                match review_kind {
                    RevlogReviewKind::Learning => count.learn += 1,
                    RevlogReviewKind::Relearning => count.relearn += 1,
                    RevlogReviewKind::Review => {
                        if last_interval < 21 {
                            count.young += 1;
                        } else {
                            count.mature += 1;
                        }
                    }
                    RevlogReviewKind::Filtered => count.filtered += 1,
                    RevlogReviewKind::Manual | RevlogReviewKind::Rescheduled => {}
                }

                if (1..=4).contains(&button_chosen) {
                    let b = (button_chosen - 1) as usize;
                    let inc = |dst: &mut ButtonCounts| match review_kind {
                        RevlogReviewKind::Learning
                        | RevlogReviewKind::Relearning
                        | RevlogReviewKind::Filtered => dst.learning[b] += 1,
                        RevlogReviewKind::Review => {
                            if last_interval < 21 {
                                dst.young[b] += 1;
                            } else {
                                dst.mature[b] += 1;
                            }
                        }
                        RevlogReviewKind::Manual | RevlogReviewKind::Rescheduled => {}
                    };
                    inc(&mut btn_all);
                    if review_secs >= stamp_1y {
                        inc(&mut btn_1y);
                        if review_secs >= stamp_3m {
                            inc(&mut btn_3m);
                            if review_secs >= stamp_1m {
                                inc(&mut btn_1m);
                            }
                        }
                    }
                }
            }
        }

        let mut excluding_inactive = Counts::default();
        let mut including_inactive = Counts::default();
        let mut future_due = FutureDue::default();
        let mut daily_load = 0.0f32;
        {
            let mut stmt = self.storage.db.prepare_cached(
                "SELECT type, queue, due, cast(ivl AS integer), odue, odid FROM cards",
            )?;
            let mut rows = stmt.query([])?;
            while let Some(row) = rows.next()? {
                let ctype: CardType = row.get(0)?;
                let queue: CardQueue = row.get(1)?;
                let due: i32 = row.get(2).ok().unwrap_or_default();
                let interval: u32 = row.get(3)?;
                let original_due: i32 = row.get(4).ok().unwrap_or_default();
                let original_deck_id: i64 = row.get(5)?;

                let bump = |counts: &mut Counts| match ctype {
                    CardType::New => counts.new_cards += 1,
                    CardType::Learn => counts.learn += 1,
                    CardType::Review => {
                        if interval < 21 {
                            counts.young += 1;
                        } else {
                            counts.mature += 1;
                        }
                    }
                    CardType::Relearn => counts.relearn += 1,
                };
                match queue {
                    CardQueue::Suspended => excluding_inactive.suspended += 1,
                    CardQueue::SchedBuried | CardQueue::UserBuried => {
                        excluding_inactive.buried += 1
                    }
                    _ => bump(&mut excluding_inactive),
                }
                bump(&mut including_inactive);

                if ctype != CardType::New && queue != CardQueue::Suspended {
                    let effective_due = if original_deck_id > 0 {
                        original_due
                    } else {
                        due
                    };
                    let due_day = if is_unix_epoch_timestamp(effective_due) {
                        ((effective_due as i64 - next_day_start.0) / 86_400) as i32
                    } else {
                        effective_due - days_elapsed
                    };
                    daily_load += 1.0 / interval.max(1) as f32;
                    if !(due_day <= 0
                        && matches!(queue, CardQueue::UserBuried | CardQueue::SchedBuried))
                    {
                        future_due.have_backlog |= due_day < 0;
                        *future_due.future_due.entry(due_day).or_default() += 1;
                    }
                }
            }
        }
        future_due.daily_load = daily_load as u32;

        Ok(anki_proto::stats::GraphsResponse {
            today: Some(today),
            reviews: Some(reviews),
            future_due: Some(future_due),
            card_counts: Some(CardCounts {
                excluding_inactive: Some(excluding_inactive),
                including_inactive: Some(including_inactive),
            }),
            buttons: Some(Buttons {
                one_month: Some(btn_1m),
                three_months: Some(btn_3m),
                one_year: Some(btn_1y),
                all_time: Some(btn_all),
            }),
            rollover_hour: self.rollover_for_current_scheduler()? as u32,
            fsrs: self.fsrs_enabled(),
            ..Default::default()
        })
    }
}

#[cfg(test)]
mod tests {
    use crate::card::Card;
    use crate::card::CardQueue;
    use crate::card::CardType;
    use crate::collection::Collection;
    use crate::revlog::RevlogEntry;
    use crate::revlog::RevlogReviewKind;

    #[test]
    fn dashboard_graphs_matches_full_graphs() {
        let mut col = Collection::new();
        let now = col.timing_today().unwrap().next_day_at;
        let mut card1 = Card {
            ctype: CardType::Review,
            queue: CardQueue::Review,
            due: 5,
            interval: 30,
            ..Default::default()
        };
        let mut card2 = Card {
            ctype: CardType::New,
            queue: CardQueue::New,
            ..Default::default()
        };
        let mut card3 = Card {
            ctype: CardType::Review,
            queue: CardQueue::Suspended,
            due: 2,
            interval: 5,
            ..Default::default()
        };
        col.add_card(&mut card1).unwrap();
        col.add_card(&mut card2).unwrap();
        col.add_card(&mut card3).unwrap();
        col.storage
            .add_revlog_entry(
                &RevlogEntry {
                    id: now.adding_secs(-3600).as_millis().into(),
                    cid: card1.id,
                    usn: Default::default(),
                    button_chosen: 3,
                    interval: 30,
                    last_interval: 21,
                    ease_factor: 2500,
                    taken_millis: 1200,
                    review_kind: RevlogReviewKind::Review,
                },
                false,
            )
            .unwrap();
        col.storage
            .add_revlog_entry(
                &RevlogEntry {
                    id: now.adding_secs(-86_400 * 40).as_millis().into(),
                    cid: card1.id,
                    usn: Default::default(),
                    button_chosen: 1,
                    interval: 10,
                    last_interval: 5,
                    ease_factor: 2500,
                    taken_millis: 800,
                    review_kind: RevlogReviewKind::Learning,
                },
                false,
            )
            .unwrap();

        let full = col.graph_data_for_search("", 0).unwrap();
        let fast = col.dashboard_graphs().unwrap();

        assert_eq!(fast.today, full.today);
        assert_eq!(
            fast.reviews.as_ref().map(|r| &r.count),
            full.reviews.as_ref().map(|r| &r.count)
        );
        assert_eq!(fast.future_due, full.future_due);
        assert_eq!(fast.card_counts, full.card_counts);
        assert_eq!(fast.buttons, full.buttons);
    }
}
