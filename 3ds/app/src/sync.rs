// Copyright: Anki 3DS contributors
// License: GNU AGPL, version 3 or later; http://www.gnu.org/licenses/agpl.html

use std::sync::atomic::AtomicBool;
use std::sync::atomic::Ordering;
use std::sync::mpsc;
use std::sync::Arc;
use std::sync::Mutex;
use std::thread::JoinHandle;

use anki::progress::Progress;
use anki::progress::ProgressState;
use anki::sync::login::sync_login_with_client;
use anki::sync::login::SyncAuth;
use anki_3ds_sync_transport::soc::NativeSoc;
use anki_3ds_sync_transport::NativeSyncClient;
use ctru::services::ps::Ps;
use futures::executor::block_on;

use super::backend::Backend;
use super::i18n::tr;
use super::media_sync::MediaSync;

#[derive(Clone, Copy)]
pub enum Mode {
    Login,
    Normal,
    Upload,
    Download,
}
pub enum Outcome {
    LoggedIn,
    Complete,
    FullRequired { upload_ok: bool, download_ok: bool },
}
pub struct Completed {
    pub backend: Option<Backend>,
    pub auth: Option<SyncAuth>,
    pub media: Option<MediaSync>,
    pub media_error: Option<String>,
    pub result: Result<Outcome, String>,
}
#[derive(Default, serde::Deserialize)]
pub struct Login {
    pub username: String,
    pub password: String,
    #[cfg(feature = "emulator")]
    pub endpoint: Option<String>,
    #[cfg(feature = "emulator")]
    pub ca_file: Option<String>,
}
impl Login {
    pub fn is_test(&self) -> bool {
        #[cfg(feature = "emulator")]
        {
            self.endpoint.is_some()
        }
        #[cfg(not(feature = "emulator"))]
        {
            false
        }
    }
    pub fn new() -> Self {
        #[cfg(feature = "emulator")]
        if let Ok(data) = std::fs::read("sdmc:/3ds/anki-3ds/sync-test.json") {
            match serde_json::from_slice(&data) {
                Ok(login) => return login,
                Err(error) => {
                    super::event(&format!("SYNC_TEST_CONFIG_ERROR {error}"));
                }
            }
        }
        Self::default()
    }
}
pub struct Job {
    receiver: mpsc::Receiver<Completed>,
    thread: Option<JoinHandle<()>>,
    cancel: Arc<AtomicBool>,
    pub progress: Arc<Mutex<ProgressState>>,
    login_only: bool,
    speed: Mutex<RateTracker>,
    unpack_speed: Mutex<RateTracker>,
}

#[derive(Default)]
struct RateTracker {
    last: Option<(std::time::Instant, u64)>,
    rate: f64,
}

impl RateTracker {
    fn sample(&mut self, bytes: u64) -> f64 {
        let now = std::time::Instant::now();
        match self.last {
            Some((previous_time, previous_bytes)) => {
                let elapsed = now.duration_since(previous_time).as_secs_f64();
                if elapsed >= 0.25 {
                    let delta = bytes.saturating_sub(previous_bytes) as f64;
                    let instant = delta / elapsed;
                    self.rate = if self.rate <= 0.0 {
                        instant
                    } else {
                        self.rate * 0.5 + instant * 0.5
                    };
                    self.last = Some((now, bytes));
                }
            }
            None => self.last = Some((now, bytes)),
        }
        self.rate
    }
}

fn lock_progress(progress: &Arc<Mutex<ProgressState>>) -> std::sync::MutexGuard<'_, ProgressState> {
    progress
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

fn format_rate(bytes_per_second: f64) -> String {
    if bytes_per_second >= 1024.0 * 1024.0 {
        format!("{:.1} MB/s", bytes_per_second / (1024.0 * 1024.0))
    } else if bytes_per_second >= 1024.0 {
        format!("{:.1} KB/s", bytes_per_second / 1024.0)
    } else {
        format!("{:.0} B/s", bytes_per_second)
    }
}

pub fn client_for(auth: Option<SyncAuth>, login: &Login) -> NativeSyncClient {
    let client = NativeSyncClient::new(auth.clone().unwrap_or_default());
    #[cfg(not(feature = "emulator"))]
    let _ = login;
    #[cfg(feature = "emulator")]
    let client = {
        let mut client = client;
        if let Some(endpoint) = &login.endpoint {
            if auth
                .as_ref()
                .and_then(|auth| auth.endpoint.as_ref())
                .is_none()
            {
                if let Ok(url) = ammonia::Url::parse(endpoint) {
                    client.endpoint = url;
                } else {
                    super::event("SYNC_TEST_ENDPOINT_INVALID");
                }
            }
        }
        if let Some(ca) = &login.ca_file {
            client.ca_file = ca.clone();
        }
        client
    };
    client
}

pub fn acquire_network(
    cancel: &AtomicBool,
    timeout: std::time::Duration,
) -> Result<(Ps, NativeSoc), String> {
    let deadline = std::time::Instant::now() + timeout;
    loop {
        let error = match Ps::new() {
            Ok(ps) => match NativeSoc::new() {
                Ok(soc) => return Ok((ps, soc)),
                Err(error) => error.to_string(),
            },
            Err(error) => error.to_string(),
        };
        if cancel.load(Ordering::Acquire) {
            return Err("Cancelled".into());
        }
        if std::time::Instant::now() >= deadline {
            return Err(format!("Network initialization failed: {error}"));
        }
        std::thread::sleep(std::time::Duration::from_millis(50));
    }
}

pub fn start_media(
    auth: Option<SyncAuth>,
    login: &Login,
    priority: Vec<String>,
    index: Arc<anki::media::files::MediaNameIndex>,
) -> Result<MediaSync, String> {
    let auth = auth.ok_or_else(|| "Not logged in".to_string())?;
    let client = client_for(Some(auth), login);
    MediaSync::start(
        client,
        super::i18n::download_connections(),
        priority,
        None,
        index,
    )
}

impl Job {
    pub fn start(
        backend: Backend,
        auth: Option<SyncAuth>,
        login: &Login,
        mode: Mode,
        progress: Arc<Mutex<ProgressState>>,
    ) -> Result<Self, std::io::Error> {
        progress
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .reset();
        let client = client_for(auth.clone(), login);
        let cancel = client.cancel.clone();
        let shared = progress.clone();
        let username = login.username.clone();
        let password = login.password.clone();
        let (sender, receiver) = mpsc::channel();
        let thread = std::thread::Builder::new()
            .name("collection-sync".into())
            .stack_size(1024 * 1024)
            .spawn(move || {
                let completed = run_job(backend, client, auth, (username, password), mode, shared);
                let _ = sender.send(completed);
            })?;
        Ok(Self {
            receiver,
            thread: Some(thread),
            cancel,
            progress,
            login_only: matches!(mode, Mode::Login),
            speed: Mutex::new(RateTracker::default()),
            unpack_speed: Mutex::new(RateTracker::default()),
        })
    }
    pub fn finish_timeout(mut self, timeout: std::time::Duration) -> Option<Completed> {
        self.cancel();
        let deadline = std::time::Instant::now() + timeout;
        loop {
            match self.receiver.try_recv() {
                Ok(done) => {
                    if let Some(thread) = self.thread.take() {
                        let _ = thread.join();
                    }
                    return Some(done);
                }
                Err(mpsc::TryRecvError::Disconnected) => {
                    if let Some(thread) = self.thread.take() {
                        let _ = thread.join();
                    }
                    return Some(Completed {
                        backend: None,
                        auth: None,
                        media: None,
                        media_error: None,
                        result: Err("Sync worker terminated".into()),
                    });
                }
                Err(mpsc::TryRecvError::Empty) => {
                    if std::time::Instant::now() >= deadline {
                        super::event("SYNC_EXIT_TIMEOUT detached");
                        drop(self.thread.take());
                        return None;
                    }
                    std::thread::sleep(std::time::Duration::from_millis(10));
                }
            }
        }
    }
    pub fn is_syncing(&self) -> bool {
        match lock_progress(&self.progress).last_progress {
            Some(Progress::NormalSync(p)) => {
                p.stage == anki::sync::collection::progress::SyncStage::Syncing
            }
            Some(Progress::MediaSync(_)) => true,
            _ => false,
        }
    }
    pub fn cancel(&self) {
        self.cancel.store(true, Ordering::Release);
        lock_progress(&self.progress).want_abort = true;
    }
    pub fn poll(&mut self) -> Option<Completed> {
        match self.receiver.try_recv() {
            Ok(result) => {
                if let Some(thread) = self.thread.take() {
                    let _ = thread.join();
                }
                Some(result)
            }
            Err(mpsc::TryRecvError::Disconnected) => {
                if let Some(thread) = self.thread.take() {
                    let _ = thread.join();
                }
                Some(Completed {
                    backend: None,
                    auth: None,
                    media: None,
                    media_error: None,
                    result: Err("Sync worker terminated".into()),
                })
            }
            Err(mpsc::TryRecvError::Empty) => None,
        }
    }
    pub fn label(&self) -> String {
        if self.login_only {
            return tr("Checking login...").into();
        }
        match lock_progress(&self.progress).last_progress {
            Some(Progress::NormalSync(p)) => {
                use anki::sync::collection::progress::SyncStage;
                let stage = match p.stage {
                    SyncStage::Connecting => tr("Connecting"),
                    SyncStage::Syncing => tr("Syncing changes"),
                    SyncStage::Finalizing => tr("Finalizing"),
                };
                format!(
                    "{stage}\n{}: {}\n{}: {}",
                    tr("Uploaded"),
                    p.local_update,
                    tr("Downloaded"),
                    p.remote_update
                )
            }
            Some(Progress::MediaSync(p)) => {
                let bytes = p.downloaded_bytes + p.uploaded_bytes;
                let rate = self
                    .speed
                    .lock()
                    .unwrap_or_else(|poisoned| poisoned.into_inner())
                    .sample(bytes);
                let unpack_rate = self
                    .unpack_speed
                    .lock()
                    .unwrap_or_else(|poisoned| poisoned.into_inner())
                    .sample(p.unpacked_bytes);
                format!(
                    "{}\n{}: {}\n{}: {}\n{}: {}\n{}: {}",
                    tr("Media"),
                    tr("Downloaded"),
                    p.downloaded_files + p.downloaded_deletions,
                    tr("Uploaded"),
                    p.uploaded_files + p.uploaded_deletions,
                    tr("Speed"),
                    format_rate(rate),
                    tr("Unpacking"),
                    format_rate(unpack_rate)
                )
            }
            Some(Progress::FullSync(p)) => super::i18n::format(
                "Collection transfer: {0} / {1} bytes",
                &[&p.transferred_bytes.to_string(), &p.total_bytes.to_string()],
            ),
            _ => tr("Connecting...").into(),
        }
    }
    pub fn title(&self) -> &'static str {
        if self.login_only {
            tr("Log in")
        } else {
            tr("Syncing")
        }
    }
}
impl Drop for Job {
    fn drop(&mut self) {
        if let Some(thread) = self.thread.take() {
            self.cancel();
            let deadline = std::time::Instant::now() + std::time::Duration::from_millis(1000);
            while !thread.is_finished() && std::time::Instant::now() < deadline {
                std::thread::sleep(std::time::Duration::from_millis(10));
            }
            if thread.is_finished() {
                let _ = thread.join();
            } else {
                super::event("SYNC_DETACHED");
            }
        }
    }
}

fn run_job(
    backend: Backend,
    mut client: NativeSyncClient,
    mut auth: Option<SyncAuth>,
    (username, mut password): (String, String),
    mode: Mode,
    progress: Arc<Mutex<ProgressState>>,
) -> Completed {
    let mut net = match acquire_network(&client.cancel, std::time::Duration::from_secs(10)) {
        Ok(net) => Some(net),
        Err(error) => {
            return Completed {
                backend: Some(backend),
                auth: None,
                media: None,
                media_error: None,
                result: Err(error),
            };
        }
    };
    let mut backend = Some(backend);
    let mut media = None;
    let mut media_error = None;
    let mut result: std::result::Result<Outcome, Box<dyn std::error::Error>> = {
        let login_result = if auth.is_none() {
            super::event("LOGIN_START");
            let started = std::time::Instant::now();
            let result = block_on(sync_login_with_client(&username, &password, &client)).map(|a| {
                client.sync_key = a.hkey.clone();
                auth = Some(a);
            });
            super::event(&format!(
                "LOGIN_RESULT ok={} elapsed_ms={}",
                result.is_ok(),
                started.elapsed().as_millis()
            ));
            result
        } else {
            Ok(())
        };
        unsafe {
            password.as_bytes_mut().fill(0);
        }
        match login_result {
            Ok(()) => {
                // Save immediately: a later sync failure must not force
                // another login.
                let saved = match &auth {
                    Some(value) => super::auth_store::save(
                        std::path::Path::new(super::auth_store::DIRECTORY),
                        value,
                    ),
                    None => Ok(()),
                };
                if let Err(error) = saved {
                    Err(Box::new(error) as Box<dyn std::error::Error>)
                } else {
                    let media_client = client.clone();
                    let cancel = client.cancel.clone();
                    let (reopened, result) =
                        backend
                            .take()
                            .unwrap()
                            .sync(client, mode, &mut auth, progress.clone());
                    backend = reopened;
                    super::event(&format!("SYNC_COLLECTION_DONE ok={}", result.is_ok()));
                    if matches!(result, Ok(Outcome::Complete)) && !cancel.load(Ordering::Acquire) {
                        if let Some(active) = backend.as_mut() {
                            let priority = active.priority_media(20);
                            let index = active.media_index_handle();
                            match MediaSync::start(
                                media_client,
                                super::i18n::download_connections(),
                                priority,
                                net.take(),
                                index,
                            ) {
                                Ok(job) => {
                                    super::event("MEDIA_SYNC_START");
                                    media = Some(job);
                                }
                                Err(error) => {
                                    super::event(&format!("MEDIA_SYNC_START_ERROR {error}"));
                                    media_error = Some(error);
                                }
                            }
                        }
                    }
                    result
                }
            }
            Err(error) => Err(Box::new(error) as Box<dyn std::error::Error>),
        }
    };
    if let Some((ps, mut soc)) = net.take() {
        anki_3ds_sync_transport::cleanup_https_session();
        let cleanup = soc.close();
        drop(ps);
        if let Err(error) = cleanup {
            if result.is_ok() {
                result = Err(Box::new(error) as Box<dyn std::error::Error>);
            }
        }
    }
    unsafe {
        password.as_bytes_mut().fill(0);
    }
    if let Err(error) = &result {
        super::event(&format!("SYNC_ERROR {}", super::error_text(error.as_ref())));
        if matches!(error.downcast_ref::<anki::error::AnkiError>(),Some(anki::error::AnkiError::SyncError {source}) if source.kind==anki::error::SyncErrorKind::AuthFailed)
        {
            auth = None;
        }
    }
    let persistence = match &auth {
        Some(value) => {
            super::auth_store::save(std::path::Path::new(super::auth_store::DIRECTORY), value)
        }
        None => super::auth_store::clear(std::path::Path::new(super::auth_store::DIRECTORY)),
    };
    let result = result.map_err(|error| super::user_error(error.as_ref()));
    let result = match persistence {
        Ok(()) => result,
        Err(error) => {
            super::event(&format!("AUTH_SAVE_ERROR {error}"));
            Err(super::i18n::format(
                "Could not save login: {0}",
                &[&error.to_string()],
            ))
        }
    };
    progress
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
        .want_abort = false;
    Completed {
        backend,
        auth,
        media,
        media_error,
        result,
    }
}
