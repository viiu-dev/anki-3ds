// Copyright: Anki 3DS contributors
// License: GNU AGPL, version 3 or later; http://www.gnu.org/licenses/agpl.html

//! Background media synchronization.
//!
//! Collection sync finishes first and hands the app back to the user; this
//! worker keeps downloading and unpacking media without touching the
//! collection database.

use std::sync::atomic::AtomicBool;
use std::sync::atomic::AtomicU32;
use std::sync::atomic::Ordering;
use std::sync::Arc;
use std::sync::Mutex;
use std::sync::TryLockError;
use std::thread::JoinHandle;
use std::time::Duration;
use std::time::Instant;

use anki::media::MediaSyncConfig;
use anki::progress::ProgressState;
use anki::progress::ThrottlingProgressHandler;
use anki::sync::media::progress::MediaSyncProgress;
use anki::sync::media::protocol::MediaPriority;
use anki_3ds_sync_transport::soc::NativeSoc;
use anki_3ds_sync_transport::NativeSyncClient;
use ctru::services::ps::Ps;
use futures::executor::block_on;

#[derive(Clone, Debug)]
pub enum State {
    Running,
    Done,
    Cancelled,
    Failed(String),
}

pub struct MediaSync {
    pub progress: Arc<Mutex<ProgressState>>,
    priority: Arc<MediaPriority>,
    focus: Arc<MediaPriority>,
    paused: Arc<AtomicBool>,
    changed: Arc<AtomicU32>,
    state: Arc<Mutex<State>>,
    cancel: Arc<AtomicBool>,
    thread: Option<JoinHandle<()>>,
}

impl MediaSync {
    pub fn start(
        client: NativeSyncClient,
        connections: usize,
        priority_files: Vec<String>,
        net: Option<(Ps, NativeSoc)>,
        index: Arc<anki::media::files::MediaNameIndex>,
    ) -> Result<Self, String> {
        let progress = Arc::new(Mutex::new(ProgressState::default()));
        let priority = MediaPriority::new();
        priority.add(priority_files, false);
        let focus = MediaPriority::new();
        let paused = Arc::new(AtomicBool::new(false));
        let changed = Arc::new(AtomicU32::new(0));
        let state = Arc::new(Mutex::new(State::Running));
        let cancel = client.cancel.clone();

        let thread_progress = progress.clone();
        let thread_priority = priority.clone();
        let thread_focus = focus.clone();
        let thread_paused = paused.clone();
        let thread_index = index.clone();
        let thread_changed = changed.clone();
        let thread_state = state.clone();
        let thread_cancel = cancel.clone();
        let thread = std::thread::Builder::new()
            .name("media-sync".into())
            .stack_size(1024 * 1024)
            .spawn(move || {
                let config = move || MediaSyncConfig {
                    max_connections: connections,
                    priority: Some(thread_priority.clone()),
                    changed: Some(thread_changed.clone()),
                    paused: Some(thread_paused.clone()),
                    focus: Some(thread_focus.clone()),
                    index: Some(thread_index.clone()),
                    parallel_downloads: false,
                };
                let outcome = run(thread_progress, client, config, net, &thread_cancel);
                super::event(&format!("MEDIA_SYNC {}", state_label(&outcome)));
                *thread_state
                    .lock()
                    .unwrap_or_else(|poisoned| poisoned.into_inner()) = outcome;
            })
            .map_err(|error| error.to_string())?;
        Ok(Self {
            progress,
            priority,
            focus,
            paused,
            changed,
            state,
            cancel,
            thread: Some(thread),
        })
    }

    /// Put the currently displayed card's files at the front of the queue.
    pub fn prioritize(&self, files: &[String]) {
        self.priority.add(files.iter().cloned(), true);
    }

    pub fn set_focus(&self, files: &[String]) {
        self.focus.replace(files);
    }

    pub fn set_paused(&self, paused: bool) {
        self.paused.store(paused, Ordering::Release);
    }

    pub fn changed(&self) -> u32 {
        self.changed.load(Ordering::Acquire)
    }

    pub fn state(&self) -> State {
        self.state
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .clone()
    }

    pub fn is_running(&self) -> bool {
        matches!(self.state(), State::Running)
    }

    pub fn cancel(&self) {
        self.cancel.store(true, Ordering::Release);
        self.progress
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .want_abort = true;
    }

    pub fn processed_files(&self) -> usize {
        match self
            .progress
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .last_progress
        {
            Some(anki::progress::Progress::MediaSync(p)) => {
                p.downloaded_files
                    + p.downloaded_deletions
                    + p.uploaded_files
                    + p.uploaded_deletions
            }
            _ => 0,
        }
    }

    pub fn checked_files(&self) -> usize {
        match self
            .progress
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .last_progress
        {
            Some(anki::progress::Progress::MediaSync(p)) => p.checked,
            _ => 0,
        }
    }
}

impl Drop for MediaSync {
    fn drop(&mut self) {
        if let Some(thread) = self.thread.take() {
            self.cancel();
            let _ = thread.join();
        }
    }
}

static WORKER: Mutex<()> = Mutex::new(());

pub fn worker_active() -> bool {
    matches!(WORKER.try_lock(), Err(TryLockError::WouldBlock))
}

fn run(
    progress: Arc<Mutex<ProgressState>>,
    client: NativeSyncClient,
    config: impl Fn() -> MediaSyncConfig,
    net: Option<(Ps, NativeSoc)>,
    cancel: &AtomicBool,
) -> State {
    let _worker = loop {
        match WORKER.try_lock() {
            Ok(guard) => break guard,
            Err(TryLockError::Poisoned(poisoned)) => break poisoned.into_inner(),
            Err(TryLockError::WouldBlock) => {
                if cancel.load(Ordering::Acquire) {
                    return State::Cancelled;
                }
                std::thread::sleep(Duration::from_millis(20));
            }
        }
    };
    let (ps, mut soc) = match net {
        Some(net) => net,
        None => match super::sync::acquire_network(cancel, Duration::from_secs(15)) {
            Ok(net) => net,
            Err(_) if cancel.load(Ordering::Acquire) => return State::Cancelled,
            Err(error) => return State::Failed(error),
        },
    };
    let result = sync_with_recovery(&progress, &client, &config, cancel);
    anki_3ds_sync_transport::cleanup_https_session();
    let cleanup = soc.close();
    drop(ps);
    match (result, cleanup, cancel.load(Ordering::Acquire)) {
        (_, _, true) => State::Cancelled,
        (Err(error), _, _) => error,
        (Ok(()), Err(error), _) => State::Failed(error.to_string()),
        (Ok(()), Ok(()), _) => State::Done,
    }
}

fn migrate_media(
    manager: &anki::media::MediaManager,
    cancel: &AtomicBool,
    paused: &Option<Arc<AtomicBool>>,
    index: &Option<Arc<anki::media::files::MediaNameIndex>>,
) -> usize {
    let paused_now = || {
        paused
            .as_ref()
            .is_some_and(|flag| flag.load(Ordering::Acquire))
    };
    if cancel.load(Ordering::Acquire) || paused_now() {
        return 0;
    }
    let started = Instant::now();
    let mut keep_going = || !cancel.load(Ordering::Acquire) && !paused_now();
    match manager.migrate_to_shards(&mut keep_going, 20) {
        Ok(moved) if moved.is_empty() => 0,
        Ok(moved) => {
            let count = moved.len();
            if let Some(index) = index {
                for fname in &moved {
                    index.mark_migrated(fname);
                }
                let _ = manager.save_media_index(index);
            }
            super::event(&format!(
                "MEDIA_MIGRATE moved={count} elapsed_ms={}",
                started.elapsed().as_millis()
            ));
            count
        }
        Err(anki::error::AnkiError::Interrupted) => 0,
        Err(error) => {
            super::event(&format!(
                "MEDIA_MIGRATE_ERROR {}",
                super::error_text(&error)
            ));
            0
        }
    }
}

fn sync_with_recovery(
    progress: &Arc<Mutex<ProgressState>>,
    client: &NativeSyncClient,
    config: &impl Fn() -> MediaSyncConfig,
    cancel: &AtomicBool,
) -> Result<(), State> {
    let started = Instant::now();
    let manager = super::backend::open_media_manager().map_err(State::Failed)?;
    super::event(&format!(
        "MEDIA_DB_OPEN elapsed_ms={}",
        started.elapsed().as_millis()
    ));
    if cancel.load(Ordering::Acquire) {
        return Err(State::Cancelled);
    }
    if migrate_media(&manager, cancel, &config().paused, &config().index) > 0 {
        if let Some(changed) = config().changed {
            changed.fetch_add(1, Ordering::Release);
        }
    }
    if cancel.load(Ordering::Acquire) {
        return Err(State::Cancelled);
    }
    let error = match run_with_resume(manager, progress, client, config, cancel) {
        Ok(()) => return Ok(()),
        Err(error) if is_interrupted(&error) => return Err(State::Cancelled),
        Err(error) => error,
    };
    if !super::backend::media_db_recoverable(&error) || cancel.load(Ordering::Acquire) {
        return Err(State::Failed(super::error_text(&error)));
    }
    super::event(&format!(
        "MEDIA_DB_RESET during_sync {}",
        super::error_text(&error)
    ));
    super::backend::reset_media_db();
    let manager = super::backend::open_media_manager().map_err(State::Failed)?;
    if migrate_media(&manager, cancel, &config().paused, &config().index) > 0 {
        if let Some(changed) = config().changed {
            changed.fetch_add(1, Ordering::Release);
        }
    }
    match run_with_resume(manager, progress, client, config, cancel) {
        Ok(()) => Ok(()),
        Err(error) if is_interrupted(&error) => Err(State::Cancelled),
        Err(error) => Err(State::Failed(super::error_text(&error))),
    }
}

fn run_with_resume(
    initial: anki::media::MediaManager,
    progress: &Arc<Mutex<ProgressState>>,
    client: &NativeSyncClient,
    config: &impl Fn() -> MediaSyncConfig,
    cancel: &AtomicBool,
) -> Result<(), anki::error::AnkiError> {
    // sync_media_with_config consumes the manager, so each resume attempt
    // re-opens it; the first attempt reuses the already-opened one.
    let mut manager = Some(initial);
    let mut last_error: Option<anki::error::AnkiError> = None;
    let mut attempts = 0u32;
    loop {
        let mgr = match manager.take() {
            Some(m) => m,
            None => match super::backend::open_media_manager() {
                Ok(m) => m,
                Err(open_error) => {
                    super::event(&format!("MEDIA_DB_REOPEN_ERROR {open_error}"));
                    return Err(last_error.unwrap_or(anki::error::AnkiError::Interrupted));
                }
            },
        };
        let handler = ThrottlingProgressHandler::<MediaSyncProgress>::new(progress.clone());
        match block_on(mgr.sync_media_with_config(handler, client.clone(), None, config())) {
            Ok(()) => return Ok(()),
            Err(e) if is_interrupted(&e) => return Err(e),
            Err(e) if is_network(&e) && attempts < 5 && !cancel.load(Ordering::Acquire) => {
                attempts += 1;
                super::event(&format!(
                    "MEDIA_SYNC_RETRY attempt={attempts} {}",
                    super::error_text(&e)
                ));
                anki_3ds_sync_transport::cleanup_https_session();
                anki_3ds_sync_transport::init_https_session();
                for _ in 0..(20 * attempts) {
                    if cancel.load(Ordering::Acquire) {
                        return Err(anki::error::AnkiError::Interrupted);
                    }
                    std::thread::sleep(Duration::from_millis(100));
                }
                last_error = Some(e);
            }
            Err(e) => return Err(e),
        }
    }
}

fn is_network(e: &anki::error::AnkiError) -> bool {
    matches!(e, anki::error::AnkiError::NetworkError { .. })
}

fn is_interrupted(error: &anki::error::AnkiError) -> bool {
    matches!(error, anki::error::AnkiError::Interrupted)
}

fn state_label(state: &State) -> String {
    match state {
        State::Running => "running".into(),
        State::Done => "done".into(),
        State::Cancelled => "cancelled".into(),
        State::Failed(error) => format!("failed {error}"),
    }
}
