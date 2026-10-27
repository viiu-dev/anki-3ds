// Copyright: Anki 3DS contributors
// License: GNU AGPL, version 3 or later; http://www.gnu.org/licenses/agpl.html

use std::sync::LazyLock;

use anki::collection::Collection;
use anki::collection::CollectionBuilder;
use anki::services::CardRenderingService;
use anki::services::CardsService;
use anki::services::DeckConfigService;
use anki::services::DecksService;
use anki::services::NotesService;
use anki::services::SchedulerService;
use anki::timestamp::TimestampMillis;
use anki::timestamp::TimestampSecs;
use anki_proto::card_rendering::rendered_template_node::Value as NodeValue;
use anki_proto::card_rendering::RenderExistingCardRequest;
use anki_proto::card_rendering::StripHtmlRequest;
use anki_proto::decks::DeckId;
use anki_proto::decks::DeckTreeNode;
use anki_proto::decks::DeckTreeRequest;
use anki_proto::scheduler::queued_cards::QueuedCard;
use anki_proto::scheduler::BuryOrSuspendCardsRequest;
use anki_proto::scheduler::CardAnswer;
use anki_proto::scheduler::GetQueuedCardsRequest;
use regex::Regex;

use super::i18n::tr;

pub const PATH: &str = "sdmc:/3ds/anki-3ds/prototype.anki2";
pub type Result<T> = std::result::Result<T, Box<dyn std::error::Error>>;

/// Standard Anki templates mark the boundary after the repeated FrontSide.
/// Rich HTML is kept for the review renderer; plain conversion is still used
/// by Browse previews.
static ANSWER_DIVIDER: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r#"(?is)<hr\b[^>]*\bid\s*=\s*(?:"answer"|'answer'|answer)(?:\s[^>]*)?/?>"#)
        .expect("static answer-divider pattern")
});

/// Upstream's own line-break tag set (`text::HTML_LINEBREAK_TAGS`), which the
/// console needs because `strip_html` deletes tags instead of spacing them.
/// Review cards use the rich renderer, but Browse previews still use this
/// plain-text conversion.
static LINE_BREAKS: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(
        r#"(?xsi)
            </?
            (?:
                br|address|article|aside|blockquote|canvas|dd|div
                |dl|dt|fieldset|figcaption|figure|footer|form
                |h[1-6]|header|hr|li|main|nav|noscript|ol
                |output|p|pre|section|table|tfoot|ul|video
            )
            \b[^>]*>
        "#,
    )
    .expect("static line-break pattern")
});

pub struct Backend {
    collection: Collection,
}
pub struct OfferedCard {
    queued: QueuedCard,
    pub id: i64,
    pub intervals: Vec<String>,
    pub question_html: String,
    pub answer_html: String,
    pub cloze_answer_html: Option<String>,
    pub new_count: u32,
    pub learn_count: u32,
    pub review_count: u32,
    shown_at: i64,
}

impl Backend {
    pub fn open_with_progress(
        progress: std::sync::Arc<std::sync::Mutex<anki::progress::ProgressState>>,
    ) -> Result<Self> {
        anki_3ds_sync_transport::storage::recover(std::path::Path::new(PATH))?;
        anki_3ds_sqlite_vfs::register().map_err(|code| format!("VFS: {code}"))?;
        let mut collection = CollectionBuilder::new(PATH)
            .set_shared_progress_state(progress)
            .set_tr(anki::prelude::I18n::new(&[super::i18n::language()]))
            .set_check_integrity(true)
            .build()?;
        collection.set_configured_utc_offset(-super::clock::offset())?;
        Ok(Self { collection })
    }

    pub fn sync(
        mut self,
        client: anki_3ds_sync_transport::NativeSyncClient,
        mode: super::sync::Mode,
        auth: &mut Option<anki::sync::login::SyncAuth>,
        progress: std::sync::Arc<std::sync::Mutex<anki::progress::ProgressState>>,
    ) -> (Option<Self>, Result<super::sync::Outcome>) {
        use futures::executor::block_on;

        use super::sync::Mode;
        use super::sync::Outcome;
        let result = match mode {
            Mode::Login => {
                let result = (|| {
                    let mut client = client;
                    let state =
                        block_on(anki::sync::collection::status::online_sync_status_check(
                            self.collection.sync_meta()?,
                            &mut client,
                        ))?;
                    if let (Some(endpoint), Some(auth)) = (state.new_endpoint, auth.as_mut()) {
                        auth.endpoint = Some(ammonia::Url::parse(&endpoint)?);
                    }
                    Ok(Outcome::LoggedIn)
                })();
                return (Some(self), result);
            }
            Mode::Normal => {
                let output = block_on(self.collection.normal_sync_with_client(client));
                let result = output.map(|o| {
                    if let (Some(endpoint),Some(auth))=(o.new_endpoint,auth.as_mut()) {auth.endpoint=ammonia::Url::parse(&endpoint).ok();}
                    match o.required {
                    anki::sync::collection::normal::SyncActionRequired::FullSyncRequired {upload_ok,download_ok} => Outcome::FullRequired {upload_ok,download_ok},
                    _ => Outcome::Complete,
                }}).map_err(|e| -> Box<dyn std::error::Error> {Box::new(e)});
                return (Some(self), result);
            }
            Mode::Upload => block_on(self.collection.full_upload_with_client(client)),
            Mode::Download => block_on(self.collection.full_download_with_client(client)),
        };
        let result = result
            .map(|_| Outcome::Complete)
            .map_err(|e| -> Box<dyn std::error::Error> { Box::new(e) });
        match Self::open_with_progress(progress) {
            Ok(backend) => (Some(backend), result),
            Err(e) => (None, Err(e)),
        }
    }

    pub fn set_language(&mut self, card: Option<&mut OfferedCard>) -> Result<()> {
        self.collection
            .set_tr(anki::prelude::I18n::new(&[super::i18n::language()]));
        if let Some(card) = card {
            card.intervals = SchedulerService::describe_next_states(
                &mut self.collection,
                card.queued
                    .states
                    .clone()
                    .ok_or("queue omitted scheduling states")?,
            )?
            .vals;
        }
        Ok(())
    }

    pub fn set_clock_offset(&mut self, offset: i32) -> Result<()> {
        if self.collection.get_configured_utc_offset() != Some(-offset) {
            self.collection.set_configured_utc_offset(-offset)?;
        }
        Ok(())
    }

    pub fn deck_tree(&mut self) -> Result<DeckTreeNode> {
        DecksService::deck_tree(
            &mut self.collection,
            DeckTreeRequest {
                now: TimestampSecs::now().0,
            },
        )
        .map_err(Into::into)
    }

    pub fn select(&mut self, id: i64) -> Result<Option<OfferedCard>> {
        let _changes = DecksService::set_current_deck(&mut self.collection, DeckId { did: id })?;
        self.next()
    }

    pub fn next(&mut self) -> Result<Option<OfferedCard>> {
        let queued = SchedulerService::get_queued_cards(
            &mut self.collection,
            GetQueuedCardsRequest {
                fetch_limit: 1,
                intraday_learning_only: false,
            },
        )?;
        let new_count = queued.new_count;
        let learn_count = queued.learning_count;
        let review_count = queued.review_count;
        let Some(queued) = queued.cards.into_iter().next() else {
            return Ok(None);
        };
        let queued_card = queued.card.as_ref().ok_or("queue omitted card")?;
        let id = queued_card.id;
        super::event(&format!(
            "OFFER card={id} now={} due={} queue={} new={new_count} learning={learn_count} review={review_count}",
            TimestampSecs::now().0, queued_card.due, queued_card.queue
        ));
        let rendered = CardRenderingService::render_existing_card(
            &mut self.collection,
            RenderExistingCardRequest {
                card_id: id,
                browser: false,
                partial_render: false,
            },
        )?;
        if rendered.is_empty {
            return Err("the card rendered to nothing".into());
        }
        let question_html = self.to_html(&render_nodes(rendered.question_nodes))?;
        let answer_html_full = self.to_html(&render_nodes(rendered.answer_nodes))?;
        let cloze_answer_html = super::review_text::mark_clozes(&answer_html_full);
        let answer_html = self.answer_html(&answer_html_full);
        let intervals = SchedulerService::describe_next_states(
            &mut self.collection,
            queued
                .states
                .clone()
                .ok_or("queue omitted scheduling states")?,
        )?
        .vals;
        Ok(Some(OfferedCard {
            id,
            intervals,
            question_html,
            answer_html,
            cloze_answer_html,
            queued,
            new_count,
            learn_count,
            review_count,
            shown_at: TimestampMillis::now().0,
        }))
    }

    fn answer_html(&self, html: &str) -> String {
        ANSWER_DIVIDER
            .find(html)
            .map(|m| &html[m.end()..])
            .unwrap_or(html)
            .to_owned()
    }

    fn answer_text(&mut self, html: &str) -> Result<String> {
        let answer = self.answer_html(html);
        self.to_text(&answer)
    }

    fn to_html(&mut self, html: &str) -> Result<String> {
        Ok(CardRenderingService::strip_av_tags(
            &mut self.collection,
            anki_proto::generic::String {
                val: html.to_owned(),
            },
        )?
        .val)
    }

    fn to_text(&mut self, html: &str) -> Result<String> {
        let spaced = LINE_BREAKS.replace_all(html, "\n");
        let without_media = self.to_html(&spaced)?;
        let text = CardRenderingService::strip_html(
            &mut self.collection,
            StripHtmlRequest {
                text: without_media,
                mode: 0,
            },
        )?
        .val;
        let mut output = String::with_capacity(text.len());
        let mut blank = 0usize;
        for line in text.lines() {
            let line = line.trim_end();
            if line.is_empty() {
                blank += 1;
                if blank > 1 {
                    continue;
                }
            } else {
                blank = 0;
            }
            output.push_str(line);
            output.push('\n');
        }
        Ok(output.trim().to_string())
    }

    pub fn answer(&mut self, card: OfferedCard, rating: i32) -> Result<i64> {
        let states = card
            .queued
            .states
            .ok_or("queue omitted scheduling states")?;
        let new_state = match rating {
            0 => states.again,
            1 => states.hard,
            2 => states.good,
            3 => states.easy,
            _ => return Err("invalid rating".into()),
        };
        let id = card.queued.card.ok_or("queue omitted card")?.id;
        let now = TimestampMillis::now().0;
        let _changes = SchedulerService::answer_card(
            &mut self.collection,
            CardAnswer {
                card_id: id,
                current_state: states.current,
                new_state,
                rating,
                answered_at_millis: now,
                milliseconds_taken: (now - card.shown_at).clamp(0, 60_000) as u32,
            },
        )?;
        Ok(id)
    }

    pub fn search(&mut self, query: &str) -> Result<Vec<i64>> {
        Ok(self
            .collection
            .search_cards(query, anki::search::SortMode::NoOrder)?
            .into_iter()
            .map(|id| id.0)
            .collect())
    }

    pub fn preview(&mut self, id: i64) -> Result<(String, String)> {
        let rendered = CardRenderingService::render_existing_card(
            &mut self.collection,
            RenderExistingCardRequest {
                card_id: id,
                browser: true,
                partial_render: false,
            },
        )?;
        Ok((
            self.to_text(&render_nodes(rendered.question_nodes))?,
            self.answer_text(&render_nodes(rendered.answer_nodes))?,
        ))
    }

    pub fn new_draft(&mut self, deck_id: i64) -> Result<NoteDraft> {
        use anki::services::NotesService;
        let defaults = NotesService::defaults_for_adding(
            &mut self.collection,
            anki_proto::notes::DefaultsForAddingRequest {
                home_deck_of_current_review_card: 0,
            },
        )?;
        let nt = self
            .collection
            .get_notetype(defaults.notetype_id.into())?
            .ok_or("note type missing")?;
        Ok(NoteDraft {
            note: nt.new_note(),
            labels: nt.fields.iter().map(|f| f.name.clone()).collect(),
            deck_id,
        })
    }

    pub fn edit_draft(&mut self, card_id: i64) -> Result<NoteDraft> {
        use anki::services::CardsService;
        let card = CardsService::get_card(
            &mut self.collection,
            anki_proto::cards::CardId { cid: card_id },
        )?;
        let note = self
            .collection
            .storage
            .get_note(card.note_id.into())?
            .ok_or("note missing")?;
        let nt = self
            .collection
            .get_notetype(note.notetype_id)?
            .ok_or("note type missing")?;
        Ok(NoteDraft {
            note,
            labels: nt.fields.iter().map(|f| f.name.clone()).collect(),
            deck_id: card.deck_id,
        })
    }

    pub fn save_draft(&mut self, draft: &mut NoteDraft) -> Result<i64> {
        if draft
            .note
            .fields()
            .first()
            .is_none_or(|s| s.trim().is_empty())
        {
            return Err(tr("The first field cannot be empty.").into());
        }
        // Upstream may assign an ID before an operation fails. Keep the draft
        // unchanged until the transaction succeeds, so retry remains an add.
        let mut note = draft.note.clone();
        if note.id.0 == 0 {
            let _changes = self
                .collection
                .add_note(&mut note, anki::decks::DeckId(draft.deck_id))?;
        } else {
            let _changes = self.collection.update_note(&mut note)?;
        }
        draft.note = note;
        Ok(draft.note.id.0)
    }

    pub fn options(&mut self, deck_id: i64) -> Result<DeckOptions> {
        let snapshot = self
            .collection
            .get_deck_configs_for_update(anki::decks::DeckId(deck_id))?;
        let deck = snapshot.current_deck.as_ref().ok_or("deck missing")?;
        let config = snapshot
            .all_config
            .iter()
            .filter_map(|c| c.config.as_ref())
            .find(|c| c.id == deck.config_id)
            .ok_or("deck config missing")?;
        let inner = config.config.as_ref().ok_or("config missing")?;
        let limits = deck.limits.as_ref();
        let new = limits.and_then(|l| l.new).unwrap_or(inner.new_per_day);
        let reviews = limits
            .and_then(|l| l.review)
            .unwrap_or(inner.reviews_per_day);
        let learn_steps = inner.learn_steps.clone();
        let relearn_steps = inner.relearn_steps.clone();
        let insert_order = inner.new_card_insert_order;
        let leech_threshold = inner.leech_threshold;
        let leech_action = inner.leech_action;
        let new_mix = inner.new_mix;
        let bury_new = inner.bury_new;
        let bury_reviews = inner.bury_reviews;
        let desired_retention = if inner.desired_retention > 0.0 {
            inner.desired_retention
        } else {
            0.9
        };
        let fsrs = snapshot.fsrs;
        Ok(DeckOptions {
            deck_id,
            new,
            reviews,
            learn_steps,
            relearn_steps,
            insert_order,
            leech_threshold,
            leech_action,
            desired_retention,
            new_mix,
            bury_new,
            bury_reviews,
            fsrs,
            snapshot,
        })
    }

    pub fn save_options(&mut self, options: &DeckOptions) -> Result<()> {
        let current = options
            .snapshot
            .current_deck
            .as_ref()
            .ok_or("deck missing")?;
        let mut config = options
            .snapshot
            .all_config
            .iter()
            .filter_map(|c| c.config.clone())
            .find(|c| c.id == current.config_id)
            .ok_or("deck config missing")?;
        let inner = config.config.as_mut().ok_or("config missing")?;
        inner.new_per_day = options.new;
        inner.reviews_per_day = options.reviews;
        inner.learn_steps = options.learn_steps.clone();
        inner.relearn_steps = options.relearn_steps.clone();
        inner.new_card_insert_order = options.insert_order;
        inner.leech_threshold = options.leech_threshold;
        inner.leech_action = options.leech_action;
        inner.new_mix = options.new_mix;
        inner.bury_new = options.bury_new;
        inner.bury_reviews = options.bury_reviews;
        inner.desired_retention = options.desired_retention;
        if inner.fsrs_params_6.is_empty() {
            if let Some(defaults) = options
                .snapshot
                .defaults
                .as_ref()
                .and_then(|deck| deck.config.as_ref())
            {
                inner.fsrs_params_6 = defaults.fsrs_params_6.clone();
            }
        }
        let mut limits = current.limits.clone().unwrap_or_default();
        limits.new = Some(options.new);
        limits.review = Some(options.reviews);
        let request = anki_proto::deck_config::UpdateDeckConfigsRequest {
            target_deck_id: options.deck_id,
            configs: vec![config],
            limits: Some(limits),
            fsrs: options.fsrs,
            mode: anki_proto::deck_config::UpdateDeckConfigsMode::Normal as i32,
            ..Default::default()
        };
        let _ = DeckConfigService::update_deck_configs(&mut self.collection, request)?;
        Ok(())
    }

    pub fn statistics(&mut self) -> Result<String> {
        let mut counts = Vec::new();
        for (label, query) in [
            (tr("Total"), ""),
            (tr("New"), "is:new"),
            (tr("Learning"), "is:learn"),
            (tr("Review"), "is:review"),
            (tr("Suspended"), "is:suspended"),
            (tr("Reviewed today"), "rated:1"),
        ] {
            counts.push(format!("{label}: {}", self.search(query)?.len()));
        }
        Ok(counts.join("\n"))
    }

    pub fn undo(&mut self) -> Result<()> {
        self.collection.undo()?;
        Ok(())
    }

    pub fn bury_or_suspend(&mut self, card_id: i64, mode: i32) -> Result<()> {
        use anki_proto::scheduler::bury_or_suspend_cards_request::Mode as BuryMode;
        let mode = match mode {
            0 => BuryMode::Suspend,
            1 => BuryMode::BurySched,
            _ => BuryMode::BuryUser,
        };
        let _ = SchedulerService::bury_or_suspend_cards(
            &mut self.collection,
            BuryOrSuspendCardsRequest {
                card_ids: vec![card_id],
                note_ids: Vec::new(),
                mode: mode as i32,
            },
        )?;
        Ok(())
    }

    pub fn card_status(&mut self, card_id: i64) -> Result<CardStatus> {
        let card = CardsService::get_card(
            &mut self.collection,
            anki_proto::cards::CardId { cid: card_id },
        )?;
        let suspended = card.queue == anki::card::CardQueue::Suspended as i32;
        let buried = card.queue == anki::card::CardQueue::SchedBuried as i32
            || card.queue == anki::card::CardQueue::UserBuried as i32;
        let note = NotesService::get_note(
            &mut self.collection,
            anki_proto::notes::NoteId { nid: card.note_id },
        )?;
        let leech = note
            .tags
            .iter()
            .any(|tag| tag.eq_ignore_ascii_case("leech"));
        Ok(CardStatus {
            buried,
            suspended,
            leech,
        })
    }

    pub fn restore_card(&mut self, card_id: i64) -> Result<()> {
        let _ = SchedulerService::restore_buried_and_suspended_cards(
            &mut self.collection,
            anki_proto::cards::CardIds {
                cids: vec![card_id],
            },
        )?;
        Ok(())
    }

    pub fn close(self) -> Result<()> {
        self.collection.close(None)?;
        Ok(())
    }
}

fn render_nodes(nodes: Vec<anki_proto::card_rendering::RenderedTemplateNode>) -> String {
    let mut output = String::new();
    for node in nodes {
        match node.value {
            Some(NodeValue::Text(text)) => output.push_str(&text),
            Some(NodeValue::Replacement(replacement)) => {
                output.push_str(&replacement.current_text);
            }
            None => {}
        }
    }
    output
}

pub struct NoteDraft {
    pub note: anki::notes::Note,
    pub labels: Vec<String>,
    pub deck_id: i64,
}

pub struct DeckOptions {
    pub deck_id: i64,
    pub new: u32,
    pub reviews: u32,
    pub learn_steps: Vec<f32>,
    pub relearn_steps: Vec<f32>,
    pub insert_order: i32,
    pub leech_threshold: u32,
    pub leech_action: i32,
    pub desired_retention: f32,
    pub new_mix: i32,
    pub bury_new: bool,
    pub bury_reviews: bool,
    pub fsrs: bool,
    pub snapshot: anki_proto::deck_config::DeckConfigsForUpdate,
}

#[derive(Clone, Copy, Default)]
pub struct CardStatus {
    pub buried: bool,
    pub suspended: bool,
    pub leech: bool,
}
