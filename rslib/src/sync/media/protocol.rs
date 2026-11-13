// Copyright: Anki Software, LLC and contributors
// License: GNU AGPL, version 3 or later; http://www.gnu.org/licenses/agpl.html

use std::collections::HashSet;
use std::collections::VecDeque;
use std::sync::atomic::AtomicBool;
use std::sync::atomic::Ordering;
use std::sync::mpsc::Receiver;
use std::sync::Arc;
use std::sync::Mutex;
use std::thread::JoinHandle;

use ammonia::Url;
use async_trait::async_trait;
use serde::de::DeserializeOwned;
use serde::Deserialize;
use serde::Serialize;
use strum::IntoStaticStr;

use crate::error;
use crate::error::AnkiError;
use crate::sync::collection::protocol::AsSyncEndpoint;
use crate::sync::error::HttpResult;
use crate::sync::media::begin::SyncBeginRequest;
use crate::sync::media::begin::SyncBeginResponse;
use crate::sync::media::changes::MediaChangesRequest;
use crate::sync::media::changes::MediaChangesResponse;
use crate::sync::media::download::DownloadFilesRequest;
use crate::sync::media::sanity::MediaSanityCheckResponse;
use crate::sync::media::sanity::SanityCheckRequest;
use crate::sync::media::upload::MediaUploadResponse;
use crate::sync::request::SyncRequest;
use crate::sync::response::SyncResponse;

#[derive(IntoStaticStr, Deserialize, PartialEq, Eq, Debug)]
#[serde(rename_all = "camelCase")]
#[strum(serialize_all = "camelCase")]
pub enum MediaSyncMethod {
    Begin,
    MediaChanges,
    UploadChanges,
    DownloadFiles,
    MediaSanity,
}

impl AsSyncEndpoint for MediaSyncMethod {
    fn as_sync_endpoint(&self, base: &Url) -> Url {
        base.join("msync/").unwrap().join(self.into()).unwrap()
    }
}

pub enum MediaDownloadEvent {
    Downloaded { data: Vec<u8> },
    Failed { error: String, files: Vec<String> },
}

/// Shared, ordered set of media filenames that should be downloaded before
/// the rest of the queue. The app pushes the files referenced by today's
/// cards, and by the card the user is currently looking at.
#[derive(Default)]
pub struct MediaPriority {
    inner: Mutex<MediaPriorityInner>,
}

#[derive(Default)]
struct MediaPriorityInner {
    order: VecDeque<String>,
    queued: HashSet<String>,
}

impl MediaPriority {
    pub fn new() -> Arc<Self> {
        Arc::new(Self::default())
    }

    /// Add filenames, either behind everything already queued (the day's
    /// cards) or at the very front (the card currently on screen).
    pub fn add(&self, files: impl IntoIterator<Item = String>, front: bool) {
        let mut inner = self
            .inner
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let files: Vec<String> = files.into_iter().filter(|file| !file.is_empty()).collect();
        if front {
            for file in files.into_iter().rev() {
                if !inner.queued.insert(file.clone()) {
                    if let Some(index) = inner.order.iter().position(|queued| queued == &file) {
                        inner.order.remove(index);
                    }
                }
                inner.order.push_front(file);
            }
        } else {
            for file in files {
                if inner.queued.insert(file.clone()) {
                    inner.order.push_back(file);
                }
            }
        }
    }

    pub fn snapshot(&self) -> Vec<String> {
        self.inner
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .order
            .iter()
            .cloned()
            .collect()
    }

    pub fn replace(&self, files: &[String]) {
        let mut inner = self
            .inner
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        inner.order.clear();
        inner.queued.clear();
        for file in files {
            if !file.is_empty() && inner.queued.insert(file.clone()) {
                inner.order.push_back(file.clone());
            }
        }
    }

    pub fn remove(&self, files: &[String]) {
        let mut inner = self
            .inner
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        for file in files {
            if inner.queued.remove(file) {
                if let Some(index) = inner.order.iter().position(|queued| queued == file) {
                    inner.order.remove(index);
                }
            }
        }
    }
}

/// A running pool of download workers. Dropping the session stops the workers
/// and waits for them to finish.
pub struct ParallelDownloadSession {
    receiver: Option<Receiver<MediaDownloadEvent>>,
    stop: Arc<AtomicBool>,
    cancel: Arc<AtomicBool>,
    handles: Vec<JoinHandle<()>>,
}

impl ParallelDownloadSession {
    pub fn new(
        receiver: Receiver<MediaDownloadEvent>,
        stop: Arc<AtomicBool>,
        cancel: Arc<AtomicBool>,
        handles: Vec<JoinHandle<()>>,
    ) -> Self {
        Self {
            receiver: Some(receiver),
            stop,
            cancel,
            handles,
        }
    }

    pub fn recv_timeout(
        &self,
        timeout: std::time::Duration,
    ) -> Result<MediaDownloadEvent, std::sync::mpsc::RecvTimeoutError> {
        match &self.receiver {
            Some(receiver) => receiver.recv_timeout(timeout),
            None => Err(std::sync::mpsc::RecvTimeoutError::Disconnected),
        }
    }
}

impl Drop for ParallelDownloadSession {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Release);
        // Release any worker blocked on a full channel before joining it, or
        // the join would wait forever.
        drop(self.receiver.take());
        let abort = self.handles.iter().any(|handle| !handle.is_finished());
        let previous = if abort {
            self.cancel.swap(true, Ordering::AcqRel)
        } else {
            false
        };
        for handle in self.handles.drain(..) {
            let _ = handle.join();
        }
        if abort {
            self.cancel.store(previous, Ordering::Release);
        }
    }
}

#[async_trait]
pub trait MediaSyncProtocol: Send + Sync + 'static {
    async fn begin(
        &self,
        req: SyncRequest<SyncBeginRequest>,
    ) -> HttpResult<SyncResponse<JsonResult<SyncBeginResponse>>>;
    async fn media_changes(
        &self,
        req: SyncRequest<MediaChangesRequest>,
    ) -> HttpResult<SyncResponse<JsonResult<MediaChangesResponse>>>;
    async fn upload_changes(
        &self,
        req: SyncRequest<Vec<u8>>,
    ) -> HttpResult<SyncResponse<JsonResult<MediaUploadResponse>>>;
    async fn download_files(
        &self,
        req: SyncRequest<DownloadFilesRequest>,
    ) -> HttpResult<SyncResponse<Vec<u8>>>;
    async fn media_sanity_check(
        &self,
        req: SyncRequest<SanityCheckRequest>,
    ) -> HttpResult<SyncResponse<JsonResult<MediaSanityCheckResponse>>>;

    /// Download several file batches over a pool of persistent connections.
    /// Transports that cannot run parallel downloads return `None`, and the
    /// caller falls back to sequential requests.
    fn download_files_parallel(
        &self,
        batches: &[Vec<String>],
        connections: usize,
    ) -> Option<ParallelDownloadSession> {
        let _ = (batches, connections);
        None
    }

    fn is_cancelled(&self) -> bool {
        false
    }
}

/// Media endpoints wrap their returns in a JSON result, and legacy
/// clients expect it to always have an err field, even if it's empty.
#[derive(Debug, Serialize, Deserialize)]
#[serde(untagged)]
pub enum JsonResult<T> {
    Ok {
        data: T,
        #[serde(default)]
        err: String,
    },
    Err {
        err: String,
    },
}

impl<T> JsonResult<T> {
    pub fn ok(inner: T) -> Self {
        Self::Ok {
            data: inner,
            err: String::new(),
        }
    }
}

impl<T> SyncResponse<JsonResult<T>>
where
    T: DeserializeOwned,
{
    pub fn json_result(&self) -> error::Result<T> {
        match serde_json::from_slice(&self.data)? {
            JsonResult::Ok { data, .. } => Ok(data),
            JsonResult::Err { err } => Err(AnkiError::server_message(err)),
        }
    }
}

#[cfg(test)]
mod test {
    use super::MediaPriority;

    #[test]
    fn priority_moves_front_without_duplicates() {
        let priority = MediaPriority::new();
        priority.add(vec!["b".into(), "a".into()], false);
        assert_eq!(priority.snapshot(), vec!["b", "a"]);

        priority.add(vec!["c".into()], true);
        assert_eq!(priority.snapshot(), vec!["c", "b", "a"]);

        priority.add(vec!["a".into(), "d".into()], true);
        assert_eq!(priority.snapshot(), vec!["a", "d", "c", "b"]);

        priority.remove(&["d".into()]);
        assert_eq!(priority.snapshot(), vec!["a", "c", "b"]);
    }
}
