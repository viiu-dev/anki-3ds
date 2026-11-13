// Copyright: Anki 3DS contributors
// License: GNU AGPL, version 3 or later; http://www.gnu.org/licenses/agpl.html

use std::collections::HashSet;
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
use super::review_text::media_kind_for_filename;
pub use super::review_text::MediaKind;

pub const PATH: &str = "sdmc:/3ds/anki-3ds/prototype.anki2";
pub const MEDIA_DIR: &str = "sdmc:/3ds/anki-3ds/prototype.media";
pub const MEDIA_DB: &str = "sdmc:/3ds/anki-3ds/prototype.mdb";
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
static IMG_SRC: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r#"(?is)<img\b[^>]*?\ssrc\s*=\s*(?:"([^"]*)"|'([^']*)'|([^\s>]+))"#)
        .expect("static img-src pattern")
});

static SOUND_TAG: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"(?is)\[sound:([^\]]+)\]").expect("static sound-tag pattern"));

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
    media_index: std::sync::Arc<anki::media::files::MediaNameIndex>,
}
#[derive(Clone)]
pub struct MediaRef {
    pub filename: String,
    pub audio: bool,
    pub kind: MediaKind,
}

/// A `[[type:...]]` answer field found in the rendered question. `expected`
/// is the note field text (narrowed to the active cloze ordinal for
/// `cloze:`), `combining` is false for the `nc` modifier, and `ignore_case`
/// is true for the `ci` modifier.
#[derive(Clone, Debug)]
pub struct TypeAnswer {
    pub field: String,
    pub expected: String,
    pub combining: bool,
    pub ignore_case: bool,
}

impl MediaRef {
    pub fn path(&self) -> String {
        anki::media::files::sharded_path(std::path::Path::new(MEDIA_DIR), &self.filename)
            .to_string_lossy()
            .into_owned()
    }
}

static TYPE_ANSWER_TAG: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"\[\[type:(.+?)\]\]").expect("static type-answer pattern"));

fn media_names(field: &str) -> Vec<String> {
    let mut names = Vec::new();
    for captures in IMG_SRC.captures_iter(field) {
        if let Some(src) = captures
            .get(1)
            .or_else(|| captures.get(2))
            .or_else(|| captures.get(3))
        {
            let src = src.as_str();
            if !src.starts_with("data:") && !src.contains("://") {
                if let Some(name) = clean_media_ref(src) {
                    names.push(name);
                }
            }
        }
    }
    for captures in SOUND_TAG.captures_iter(field) {
        if let Some(name) = captures.get(1).and_then(|m| clean_media_ref(m.as_str())) {
            names.push(name);
        }
    }
    names
}

/// Turn a media reference from rendered HTML into the filename on disk:
/// HTML entities and percent escapes decoded, path stripped to its last part.
fn clean_media_ref(raw: &str) -> Option<String> {
    let decoded = super::review_text::decode_entities(raw);
    let decoded = percent_decode(&decoded);
    let name = decoded
        .rsplit(|ch| ch == '/' || ch == '\\')
        .next()
        .unwrap_or(decoded.as_str())
        .trim()
        .trim_matches('"')
        .trim_matches('\'');
    if name.is_empty() {
        return None;
    }
    let normalized = anki::media::files::normalize_filename(name).into_owned();
    if normalized.is_empty() {
        None
    } else {
        Some(normalized)
    }
}

fn percent_decode(input: &str) -> String {
    let bytes = input.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut index = 0;
    while index < bytes.len() {
        if bytes[index] == b'%' && index + 2 < bytes.len() {
            if let (Some(hi), Some(lo)) = (hex_value(bytes[index + 1]), hex_value(bytes[index + 2]))
            {
                out.push(hi * 16 + lo);
                index += 3;
                continue;
            }
        }
        out.push(bytes[index]);
        index += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

fn hex_value(byte: u8) -> Option<u8> {
    match byte {
        b'0'..=b'9' => Some(byte - b'0'),
        b'a'..=b'f' => Some(byte - b'a' + 10),
        b'A'..=b'F' => Some(byte - b'A' + 10),
        _ => None,
    }
}

/// Replace every `[[type:...]]` placeholder in card HTML with `replacement`.
/// The replacement is inserted literally (no `$` expansion), so compared
/// answer HTML passes through unchanged.
pub fn fill_type_answer(html: &str, replacement: &str) -> String {
    TYPE_ANSWER_TAG
        .replace_all(html, regex::NoExpand(replacement))
        .into_owned()
}

/// Surface video refs as `<video>` markers so the card parser routes them to
/// the animated-media player. The original `[sound:...]` tags were already
/// removed by AV stripping, so the markers are appended at the end of the
/// side's HTML.
fn append_video_markers(html: &mut String, media: &[MediaRef]) {
    for item in media.iter().filter(|item| item.kind == MediaKind::Video) {
        let src = item.filename.replace('&', "&amp;").replace('"', "&quot;");
        html.push_str(&format!("<video src=\"{src}\"></video>"));
    }
}

fn clear_stale_locks() {
    static CLEARED: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);
    if CLEARED.swap(true, std::sync::atomic::Ordering::AcqRel) {
        return;
    }
    for path in [PATH, MEDIA_DB] {
        let lock = format!("{path}.lock");
        match std::fs::remove_dir_all(&lock) {
            Ok(()) => super::event(&format!("STALE_LOCK_CLEARED {lock}")),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => super::event(&format!("STALE_LOCK_ERROR {lock}: {error}")),
        }
    }
}

pub fn media_db_recoverable(error: &anki::error::AnkiError) -> bool {
    let anki::error::AnkiError::DbError { source } = error else {
        return false;
    };
    let info = source.info.as_str();
    info.contains("extended_code: 266")
        || info.contains("SystemIoFailure")
        || info.contains("DatabaseCorrupt")
        || info.contains("NotADatabase")
}

pub fn reset_media_db() {
    for suffix in ["", "-journal", "-wal", "-shm"] {
        let path = format!("{MEDIA_DB}{suffix}");
        match std::fs::remove_file(&path) {
            Ok(()) => super::event(&format!("MEDIA_DB_REMOVED {path}")),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => super::event(&format!("MEDIA_DB_REMOVE_ERROR {path}: {error}")),
        }
    }
    let _ = std::fs::remove_dir_all(format!("{MEDIA_DB}.lock"));
}

pub fn open_media_manager() -> std::result::Result<anki::media::MediaManager, String> {
    match anki::media::MediaManager::new(MEDIA_DIR, MEDIA_DB) {
        Ok(manager) => Ok(manager),
        Err(error) if media_db_recoverable(&error) => {
            super::event(&format!("MEDIA_DB_RESET {}", super::error_text(&error)));
            reset_media_db();
            anki::media::MediaManager::new(MEDIA_DIR, MEDIA_DB)
                .map_err(|error| super::error_text(&error))
        }
        Err(error) => Err(super::error_text(&error)),
    }
}

pub struct OfferedCard {
    queued: QueuedCard,
    pub id: i64,
    pub intervals: Vec<String>,
    pub question_html: String,
    pub answer_html: String,
    pub css: String,
    pub cloze_answer_html: Option<String>,
    pub type_answer: Option<TypeAnswer>,
    pub question_media: Vec<MediaRef>,
    pub answer_media: Vec<MediaRef>,
    pub new_count: u32,
    pub learn_count: u32,
    pub review_count: u32,
    shown_at: i64,
}

pub struct AnswerOutcome {
    pub card_id: i64,
    pub leech: Option<LeechEvent>,
}

#[derive(Clone, Copy, Debug)]
pub enum LeechEvent {
    Suspended,
    Tagged,
}

impl Backend {
    pub fn open_with_progress(
        progress: std::sync::Arc<std::sync::Mutex<anki::progress::ProgressState>>,
    ) -> Result<Self> {
        clear_stale_locks();
        anki_3ds_sync_transport::storage::recover(std::path::Path::new(PATH))?;
        anki_3ds_sqlite_vfs::register().map_err(|code| format!("VFS: {code}"))?;
        let mut collection = CollectionBuilder::new(PATH)
            .set_media_paths(MEDIA_DIR, MEDIA_DB)
            .set_shared_progress_state(progress)
            .set_tr(anki::prelude::I18n::new(&[super::i18n::language()]))
            .set_check_integrity(true)
            .build()?;
        collection.set_configured_utc_offset(-super::clock::offset())?;
        let media_index = if super::media_sync::worker_active() {
            super::event("MEDIA_INIT_SKIPPED worker_active");
            anki::media::files::load_media_name_index(std::path::Path::new(MEDIA_DIR))
                .unwrap_or_else(anki::media::files::MediaNameIndex::new)
        } else {
            match open_media_manager() {
                Ok(manager) => {
                    match anki::media::files::load_media_name_index(std::path::Path::new(MEDIA_DIR))
                    {
                        Some(cached) => cached,
                        None => {
                            let index = anki::media::files::MediaNameIndex::new();
                            match manager.known_file_names() {
                                Ok(names) => {
                                    super::event(&format!("MEDIA_INDEX_SEEDED {}", names.len()));
                                    for name in names {
                                        index.insert(&name, false);
                                    }
                                }
                                Err(error) => {
                                    super::event(&format!(
                                        "MEDIA_INDEX_SEED_ERROR {}",
                                        super::error_text(&error)
                                    ));
                                }
                            }
                            if let Err(error) = manager.save_media_index(&index) {
                                super::event(&format!(
                                    "MEDIA_INDEX_SAVE_ERROR {}",
                                    super::error_text(&error)
                                ));
                            }
                            index
                        }
                    }
                }
                Err(error) => {
                    super::event(&format!("MEDIA_INIT_ERROR {error}"));
                    anki::media::files::MediaNameIndex::new()
                }
            }
        };
        super::event(&format!("MEDIA_INDEX {}", media_index.len()));
        Ok(Self {
            collection,
            media_index,
        })
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
                let result = output
                    .map(|o| {
                        if let (Some(endpoint), Some(auth)) = (o.new_endpoint, auth.as_mut()) {
                            auth.endpoint = ammonia::Url::parse(&endpoint).ok();
                        }
                        match o.required {
                            anki::sync::collection::normal::SyncActionRequired::FullSyncRequired {
                                upload_ok,
                                download_ok,
                            } => Outcome::FullRequired {
                                upload_ok,
                                download_ok,
                            },
                            _ => Outcome::Complete,
                        }
                    })
                    .map_err(|e| -> Box<dyn std::error::Error> { Box::new(e) });
                return (Some(self), result);
            }
            Mode::Upload => block_on(self.collection.full_upload_with_client(client)),
            Mode::Download => block_on(self.collection.full_download_with_client(client)),
        };
        let result = result
            .map(|_| Outcome::Complete)
            .map_err(|e| -> Box<dyn std::error::Error> { Box::new(e) });
        match Self::open_with_progress(progress.clone()) {
            Ok(backend) => (Some(backend), result),
            Err(e) => (None, Err(e)),
        }
    }

    #[allow(dead_code)]
    pub fn media_manager(&self) -> Result<anki::media::MediaManager> {
        self.collection
            .media()
            .map_err(|error| Box::new(error) as Box<dyn std::error::Error>)
    }

    /// Media filenames referenced by cards that are queued for study today,
    /// so background sync can fetch them before the rest of the deck's media.
    pub fn priority_media(&mut self, limit: usize) -> Vec<String> {
        let Ok(queued) = SchedulerService::get_queued_cards(
            &mut self.collection,
            GetQueuedCardsRequest {
                fetch_limit: limit as u32,
                intraday_learning_only: false,
            },
        ) else {
            return Vec::new();
        };
        let mut files = Vec::new();
        let mut seen = HashSet::new();
        for queued in queued.cards {
            let Some(card) = queued.card else {
                continue;
            };
            let Ok(Some(note)) = self.collection.storage.get_note(card.note_id.into()) else {
                continue;
            };
            for field in note.fields() {
                for name in media_names(field) {
                    if seen.insert(name.clone()) && !self.media_index.contains(&name) {
                        files.push(name);
                    }
                }
            }
        }
        files
    }

    pub fn media_exists(&self, filename: &str) -> bool {
        self.media_index.contains(filename)
    }

    pub fn media_index_handle(&self) -> std::sync::Arc<anki::media::files::MediaNameIndex> {
        self.media_index.clone()
    }

    pub fn media_path(&self, filename: &str) -> String {
        if self.media_index.is_unmigrated(filename) {
            format!("{MEDIA_DIR}/{filename}")
        } else {
            anki::media::files::sharded_path(std::path::Path::new(MEDIA_DIR), filename)
                .to_string_lossy()
                .into_owned()
        }
    }

    pub fn card_missing_files(&self, card: &OfferedCard, answer: bool) -> Vec<String> {
        let media = if answer {
            &card.answer_media
        } else {
            &card.question_media
        };
        media
            .iter()
            .filter(|item| !self.media_index.contains(&item.filename))
            .map(|item| item.filename.clone())
            .collect()
    }

    pub fn card_media_missing(&self, card: &OfferedCard, answer: bool) -> bool {
        let media = if answer {
            &card.answer_media
        } else {
            &card.question_media
        };
        media
            .iter()
            .any(|item| !self.media_index.contains(&item.filename))
    }

    pub fn card_all_files(&self, card: &OfferedCard) -> Vec<String> {
        let mut files: Vec<String> = card
            .question_media
            .iter()
            .chain(card.answer_media.iter())
            .map(|item| item.filename.clone())
            .collect();
        files.sort();
        files.dedup();
        files
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
        self.offer(queued, new_count, learn_count, review_count)
            .map(Some)
    }

    pub fn reopen_card(&mut self, card_id: i64) -> Result<Option<OfferedCard>> {
        let queued = SchedulerService::get_queued_cards(
            &mut self.collection,
            GetQueuedCardsRequest {
                fetch_limit: 500,
                intraday_learning_only: false,
            },
        )?;
        let new_count = queued.new_count;
        let learn_count = queued.learning_count;
        let review_count = queued.review_count;
        let Some(queued) = queued
            .cards
            .into_iter()
            .find(|queued| queued.card.as_ref().is_some_and(|card| card.id == card_id))
        else {
            return self.next();
        };
        self.offer(queued, new_count, learn_count, review_count)
            .map(Some)
    }

    fn offer(
        &mut self,
        queued: QueuedCard,
        new_count: u32,
        learn_count: u32,
        review_count: u32,
    ) -> Result<OfferedCard> {
        let queued_card = queued.card.as_ref().ok_or("queue omitted card")?;
        let id = queued_card.id;
        super::event(&format!(
            "OFFER card={id} now={} due={} queue={} new={new_count} learning={learn_count} review={review_count}",
            TimestampSecs::now().0,
            queued_card.due,
            queued_card.queue
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
        let question_nodes = render_nodes(rendered.question_nodes);
        let answer_nodes = render_nodes(rendered.answer_nodes);
        let mut question_html = self.to_html(&question_nodes)?;
        let mut answer_html_full = self.to_html(&answer_nodes)?;
        let mut question_media = self.media_refs(&question_nodes, true)?;
        // AV stripping removed the original `[sound:clip.mp4]` tags; surface
        // video refs as `<video>` markers so the parser routes them to the
        // animated-media player instead of the still-image list. (Animated
        // `<img>` tags are handled by the parser directly.)
        append_video_markers(&mut question_html, &question_media);
        question_media.extend(self.image_refs(&question_nodes));
        let mut answer_media = self.media_refs(&answer_nodes, false)?;
        append_video_markers(&mut answer_html_full, &answer_media);
        answer_media.extend(self.image_refs(&answer_nodes));
        let type_answer = self.type_answer_for_card(
            &question_html,
            queued_card.note_id,
            queued_card.template_idx,
        );
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
        Ok(OfferedCard {
            id,
            intervals,
            question_html,
            answer_html,
            css: rendered.css,
            cloze_answer_html,
            type_answer,
            question_media,
            answer_media,
            queued,
            new_count,
            learn_count,
            review_count,
            shown_at: TimestampMillis::now().0,
        })
    }

    /// Look for a `[[type:...]]` placeholder in the rendered question and
    /// resolve the expected answer from the note field. For `cloze:`, the
    /// active cloze ordinal's text is collected (multiple entries joined with
    /// `, `) as upstream does. Returns `None` when there is no placeholder,
    /// the field is unknown, or the expected text is empty.
    fn type_answer_for_card(
        &mut self,
        question_html: &str,
        note_id: i64,
        template_idx: u32,
    ) -> Option<TypeAnswer> {
        let captures = TYPE_ANSWER_TAG.captures(question_html)?;
        let spec = captures.get(1)?.as_str();
        // Field names cannot contain a colon, so anything before the last
        // one is a modifier.
        let (modifiers, field) = spec.rsplit_once(':').unwrap_or(("", spec));
        let mut combining = true;
        let mut ignore_case = false;
        let mut cloze = false;
        for modifier in modifiers.split(':') {
            match modifier {
                "cloze" => cloze = true,
                "nc" => combining = false,
                "ci" => ignore_case = true,
                _ => {}
            }
        }
        if field.is_empty() {
            return None;
        }
        let note = self.collection.storage.get_note(note_id.into()).ok()??;
        let notetype = self.collection.get_notetype(note.notetype_id).ok()??;
        let index = notetype.fields.iter().position(|f| f.name == field)?;
        let field_text = note.fields().get(index)?.clone();
        let expected = if cloze {
            anki::cloze::extract_cloze_for_typing(&field_text, (template_idx + 1) as u16)
                .into_owned()
        } else {
            field_text
        };
        if expected.trim().is_empty() {
            return None;
        }
        Some(TypeAnswer {
            field: field.to_owned(),
            expected,
            combining,
            ignore_case,
        })
    }

    /// Compare a typed answer against the expected text, returning
    /// `<code id=typeans>` HTML with `typeGood`/`typeBad`/`typeMissed`
    /// spans. Thin wrapper over the core type-answer comparison.
    pub fn compare_answer(&mut self, expected: &str, provided: &str, combining: bool) -> String {
        self.compare_answer_full(expected, provided, combining, false)
    }

    /// [`Backend::compare_answer`] with an explicit `ci` (ignore-case) flag.
    pub fn compare_answer_full(
        &mut self,
        expected: &str,
        provided: &str,
        combining: bool,
        ignore_case: bool,
    ) -> String {
        CardRenderingService::compare_answer(
            &mut self.collection,
            anki_proto::card_rendering::CompareAnswerRequest {
                expected: expected.to_owned(),
                provided: provided.to_owned(),
                combining,
                ignore_case,
            },
        )
        .map(|response| response.val)
        .unwrap_or_else(|_| expected.to_owned())
    }

    pub fn collection_modified_ms(&mut self) -> Result<i64> {
        Ok(self.collection.sync_meta()?.modified.0)
    }

    pub fn studied_today_text(&mut self) -> Result<String> {
        let text = self.collection.studied_today()?;
        Ok(text.split_whitespace().collect::<Vec<_>>().join(" "))
    }

    fn answer_html(&self, html: &str) -> String {
        ANSWER_DIVIDER
            .find(html)
            .map(|m| &html[m.end()..])
            .unwrap_or(html)
            .to_owned()
    }

    fn image_refs(&self, html: &str) -> Vec<MediaRef> {
        let tags = html.to_ascii_lowercase().matches("<img").count();
        let mut matched = 0;
        let mut refs = Vec::new();
        for captures in IMG_SRC.captures_iter(html) {
            matched += 1;
            let Some(src) = captures
                .get(1)
                .or_else(|| captures.get(2))
                .or_else(|| captures.get(3))
                .map(|m| m.as_str())
            else {
                continue;
            };
            let skipped = if src.starts_with("data:") {
                Some("inline_data")
            } else if src.contains("://") {
                Some("remote_url")
            } else {
                None
            };
            if let Some(reason) = skipped {
                super::event(&format!(
                    "IMAGE_REF_SKIP reason={reason} src={}",
                    src.chars().take(60).collect::<String>()
                ));
                continue;
            }
            match clean_media_ref(src) {
                Some(filename) => {
                    let kind = media_kind_for_filename(&filename);
                    refs.push(MediaRef {
                        audio: kind == MediaKind::Audio,
                        kind,
                        filename,
                    })
                }
                None => super::event(&format!(
                    "IMAGE_REF_SKIP reason=bad_name src={}",
                    src.chars().take(160).collect::<String>()
                )),
            }
        }
        if tags > matched {
            let snippet = html
                .to_ascii_lowercase()
                .find("<img")
                .map(|start| html[start..].chars().take(160).collect::<String>())
                .unwrap_or_default();
            super::event(&format!(
                "IMAGE_REF_SKIP reason=no_src tags={tags} matched={matched} html={snippet}"
            ));
        }
        refs
    }

    fn media_refs(&mut self, html: &str, question_side: bool) -> Result<Vec<MediaRef>> {
        let response = CardRenderingService::extract_av_tags(
            &mut self.collection,
            anki_proto::card_rendering::ExtractAvTagsRequest {
                text: html.to_owned(),
                question_side,
            },
        )?;
        Ok(response
            .av_tags
            .into_iter()
            .filter_map(|tag| match tag.value {
                Some(anki_proto::card_rendering::av_tag::Value::SoundOrVideo(filename)) => {
                    let filename = clean_media_ref(&filename)?;
                    let kind = media_kind_for_filename(&filename);
                    Some(MediaRef {
                        audio: kind == MediaKind::Audio,
                        kind,
                        filename,
                    })
                }
                _ => None,
            })
            .collect())
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

    pub fn answer(&mut self, card: OfferedCard, rating: i32) -> Result<AnswerOutcome> {
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
        let before = if rating == 0 {
            self.card_status(id).ok()
        } else {
            None
        };
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
        let leech = match (before, rating == 0) {
            (Some(before), true) => match self.card_status(id) {
                Ok(after) => {
                    if !before.leech && after.leech {
                        Some(if after.suspended {
                            LeechEvent::Suspended
                        } else {
                            LeechEvent::Tagged
                        })
                    } else if !before.suspended && after.suspended {
                        Some(LeechEvent::Suspended)
                    } else {
                        None
                    }
                }
                Err(_) => None,
            },
            _ => None,
        };
        Ok(AnswerOutcome { card_id: id, leech })
    }

    pub fn search(&mut self, query: &str) -> Result<Vec<i64>> {
        Ok(self
            .collection
            .search_cards(query.trim(), anki::search::SortMode::NoOrder)?
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
        let l = deck.limits.clone().unwrap_or_default();
        let today_new = l.new_today_active.then_some(l.new_today).flatten();
        let today_reviews = l.review_today_active.then_some(l.review_today).flatten();
        let limit_scope = if today_new.is_some() || today_reviews.is_some() {
            LimitScope::TodayOnly
        } else if l.new.is_some() || l.review.is_some() {
            LimitScope::ThisDeck
        } else {
            LimitScope::Preset
        };
        let new = today_new.or(l.new).unwrap_or(inner.new_per_day);
        let reviews = today_reviews.or(l.review).unwrap_or(inner.reviews_per_day);
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
            limit_scope,
            preset_new: inner.new_per_day,
            preset_reviews: inner.reviews_per_day,
            deck_new: l.new,
            deck_reviews: l.review,
            today_new,
            today_reviews,
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
        inner.new_per_day = options.preset_new;
        inner.reviews_per_day = options.preset_reviews;
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
        match options.limit_scope {
            LimitScope::Preset => {
                inner.new_per_day = options.new;
                inner.reviews_per_day = options.reviews;
                limits.new = None;
                limits.review = None;
                limits.new_today = None;
                limits.review_today = None;
            }
            LimitScope::ThisDeck => {
                limits.new = Some(options.new);
                limits.review = Some(options.reviews);
                limits.new_today = None;
                limits.review_today = None;
            }
            LimitScope::TodayOnly => {
                limits.new = options.deck_new;
                limits.review = options.deck_reviews;
                limits.new_today = Some(options.new);
                limits.review_today = Some(options.reviews);
            }
        }
        limits.new_today_active = limits.new_today.is_some();
        limits.review_today_active = limits.review_today.is_some();
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

    pub fn checkpoint(&self) -> Result<()> {
        self.collection
            .storage
            .db()
            .execute_batch("pragma wal_checkpoint(truncate)")?;
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

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum LimitScope {
    Preset,
    ThisDeck,
    TodayOnly,
}

impl LimitScope {
    pub fn cycle(self, delta: i32) -> Self {
        let order = [Self::Preset, Self::ThisDeck, Self::TodayOnly];
        let index = order.iter().position(|scope| *scope == self).unwrap_or(0);
        let count = order.len() as i32;
        order[((index as i32 + delta).rem_euclid(count)) as usize]
    }
}

impl DeckOptions {
    pub fn set_scope(&mut self, scope: LimitScope) {
        self.limit_scope = scope;
        let (n, r) = match scope {
            LimitScope::Preset => (Some(self.preset_new), Some(self.preset_reviews)),
            LimitScope::ThisDeck => (self.deck_new, self.deck_reviews),
            LimitScope::TodayOnly => (self.today_new, self.today_reviews),
        };
        self.new = n.or(self.deck_new).unwrap_or(self.preset_new);
        self.reviews = r.or(self.deck_reviews).unwrap_or(self.preset_reviews);
    }
}

pub struct DeckOptions {
    pub deck_id: i64,
    pub new: u32,
    pub reviews: u32,
    pub limit_scope: LimitScope,
    pub preset_new: u32,
    pub preset_reviews: u32,
    pub deck_new: Option<u32>,
    pub deck_reviews: Option<u32>,
    pub today_new: Option<u32>,
    pub today_reviews: Option<u32>,
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
