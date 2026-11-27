// Copyright: Anki Software, LLC and contributors
// License: GNU AGPL, version 3 or later; http://www.gnu.org/licenses/agpl.html

use std::collections::HashSet;
use std::sync::atomic::AtomicBool;
use std::sync::atomic::AtomicU32;
use std::sync::atomic::Ordering;
use std::sync::mpsc;
use std::sync::Arc;
use std::time::Duration;

use tracing::debug;
use version::sync_client_version;

use crate::error::AnkiError;
use crate::error::Result;
use crate::error::SyncErrorKind;
use crate::media::files::combined_folder_mtime;
use crate::media::files::locate_existing;
use crate::media::files::normalize_filename;
use crate::media::files::save_media_name_index;
use crate::media::files::AddedFile;
use crate::media::files::MediaNameIndex;
use crate::media::MediaManager;
use crate::prelude::*;
use crate::progress::ThrottlingProgressHandler;
use crate::sync::media::begin::SyncBeginRequest;
use crate::sync::media::begin::SyncBeginResponse;
use crate::sync::media::changes;
use crate::sync::media::changes::MediaChangesRequest;
use crate::sync::media::database::client::changetracker::ChangeTracker;
use crate::sync::media::database::client::MediaDatabaseMetadata;
use crate::sync::media::database::client::MediaEntry;
use crate::sync::media::download;
use crate::sync::media::download::DownloadFilesRequest;
use crate::sync::media::progress::MediaSyncProgress;
use crate::sync::media::protocol::MediaDownloadEvent;
use crate::sync::media::protocol::MediaPriority;
use crate::sync::media::protocol::MediaSyncProtocol;
use crate::sync::media::protocol::ParallelDownloadSession;
use crate::sync::media::sanity::MediaSanityCheckResponse;
use crate::sync::media::sanity::SanityCheckRequest;
use crate::sync::media::upload::gather_zip_data_for_upload;
use crate::sync::media::zip::zip_files_for_upload;
use crate::sync::media::MAX_MEDIA_CONNECTIONS;
use crate::sync::media::MAX_MEDIA_FILES_IN_ZIP;
use crate::sync::request::IntoSyncRequest;
use crate::version;

const DOWNLOAD_COMMIT_FILES: usize = 100;
const DOWNLOAD_BATCH_FILES: usize = if cfg!(target_os = "horizon") {
    10
} else {
    MAX_MEDIA_FILES_IN_ZIP
};
const INDEX_SAVE_INTERVAL_FILES: usize = 100;
const PRIORITY_CACHE_LIMIT: usize = 256;
const BATCH_RETRIES: u32 = 3;

fn is_network_error(e: &AnkiError) -> bool {
    matches!(e, AnkiError::NetworkError { .. })
}

pub struct MediaSyncer<T> {
    mgr: MediaManager,
    client: T,
    progress: ThrottlingProgressHandler<MediaSyncProgress>,
    download_connections: usize,
    priority: Option<Arc<MediaPriority>>,
    changed: Option<Arc<AtomicU32>>,
    paused: Option<Arc<AtomicBool>>,
    focus: Option<Arc<MediaPriority>>,
    index: Option<Arc<MediaNameIndex>>,
    index_dirty: bool,
    index_dirty_count: usize,
    parallel_downloads: bool,
    last_focus_version: u32,
    last_priority_version: u32,
    priority_attempted: HashSet<String>,
    priority_fetched: HashSet<String>,
}

impl<T: MediaSyncProtocol> MediaSyncer<T> {
    const SCAN_LOCAL_CHANGES: bool = !cfg!(target_os = "horizon");

    pub fn new(
        mgr: MediaManager,
        progress: ThrottlingProgressHandler<MediaSyncProgress>,
        client: T,
    ) -> Result<MediaSyncer<T>> {
        Ok(MediaSyncer {
            mgr,
            client,
            progress,
            download_connections: 1,
            priority: None,
            changed: None,
            paused: None,
            focus: None,
            index: None,
            index_dirty: false,
            index_dirty_count: 0,
            parallel_downloads: true,
            last_focus_version: u32::MAX,
            last_priority_version: u32::MAX,
            priority_attempted: HashSet::new(),
            priority_fetched: HashSet::new(),
        })
    }

    pub fn set_download_connections(&mut self, connections: usize) {
        self.download_connections = connections.clamp(1, MAX_MEDIA_CONNECTIONS);
    }

    pub fn set_priority(&mut self, priority: Option<Arc<MediaPriority>>) {
        self.priority = priority;
    }

    pub fn set_changed(&mut self, changed: Option<Arc<AtomicU32>>) {
        self.changed = changed;
    }

    pub fn set_paused(&mut self, paused: Option<Arc<AtomicBool>>) {
        self.paused = paused;
    }

    pub fn set_focus(&mut self, focus: Option<Arc<MediaPriority>>) {
        self.focus = focus;
    }

    pub fn set_index(&mut self, index: Option<Arc<MediaNameIndex>>) {
        self.index = index;
    }

    pub fn set_parallel_downloads(&mut self, parallel: bool) {
        self.parallel_downloads = parallel;
    }

    fn notify_changed(&self) {
        if let Some(changed) = &self.changed {
            changed.fetch_add(1, Ordering::Release);
        }
    }

    fn bulk_paused(&self) -> bool {
        self.paused
            .as_ref()
            .is_some_and(|paused| paused.load(Ordering::Acquire))
    }

    fn on_demand_priority(&self) -> Option<(Arc<MediaPriority>, bool)> {
        if self.bulk_paused() {
            self.focus.clone().map(|p| (p, true))
        } else {
            self.priority.clone().map(|p| (p, false))
        }
    }

    async fn pause_point(&mut self) -> Result<()> {
        if self.bulk_paused() && self.index_dirty_count >= INDEX_SAVE_INTERVAL_FILES {
            self.save_index();
        }
        while self.bulk_paused() {
            if let Err(e) = self.fetch_priority_now().await {
                if matches!(e, AnkiError::Interrupted) {
                    return Err(e);
                }
            }
            if self.client.is_cancelled() || self.progress.check_cancelled().is_err() {
                return Err(AnkiError::Interrupted);
            }
            std::thread::sleep(Duration::from_millis(50));
        }
        Ok(())
    }

    pub async fn sync(&mut self, server_usn: Option<Usn>) -> Result<()> {
        let result = self.sync_inner(server_usn).await.map_err(|e| {
            debug!("sync error: {:?}", e);
            e
        });
        let _ = std::fs::remove_file(self.download_scratch_path());
        self.save_index();
        result
    }

    fn download_scratch_path(&self) -> std::path::PathBuf {
        self.mgr.media_folder.with_file_name(".media-batch.zip")
    }

    fn save_index(&mut self) {
        if !self.index_dirty || self.client.is_cancelled() {
            return;
        }
        self.index_dirty = false;
        self.index_dirty_count = 0;
        if let Some(index) = &self.index {
            let _ = save_media_name_index(index, &self.mgr.media_folder);
        }
    }

    #[allow(clippy::useless_let_if_seq)]
    async fn sync_inner(&mut self, server_usn: Option<Usn>) -> Result<()> {
        self.fetch_priority_now().await?;
        self.progress.check_cancelled()?;

        let meta = self.mgr.db.get_meta()?;
        let fresh_db = meta.last_sync_usn.0 == 0;
        if fresh_db {
            self.pause_point().await?;
            if Self::SCAN_LOCAL_CHANGES {
                self.register_changes()?;
            }
            self.progress.check_cancelled()?;
        }
        let meta = self.mgr.db.get_meta()?;
        let client_usn = meta.last_sync_usn;
        let server_usn = if let Some(usn) = server_usn {
            usn
        } else {
            self.begin_sync().await?
        };

        let mut actions_performed = false;

        // need to fetch changes from server?
        if client_usn != server_usn {
            self.progress.check_cancelled()?;
            debug!("differs from local usn {}, fetching changes", client_usn);
            self.fetch_changes(meta).await?;
            actions_performed = true;
        }

        if !fresh_db {
            self.progress.check_cancelled()?;
            self.pause_point().await?;
            if Self::SCAN_LOCAL_CHANGES {
                self.register_changes()?;
            }
        }

        // need to send changes to server?
        let changes_pending = !self.mgr.db.get_pending_uploads(1)?.is_empty();
        if changes_pending {
            self.progress.check_cancelled()?;
            self.pause_point().await?;
            self.send_changes().await?;
            actions_performed = true;
        }

        if actions_performed {
            self.progress.check_cancelled()?;
            self.finalize_sync().await?;
        }

        debug!("media sync complete");

        Ok(())
    }

    async fn begin_sync(&mut self) -> Result<Usn> {
        debug!("begin media sync");
        let SyncBeginResponse {
            host_key: _,
            usn: server_usn,
        } = self
            .client
            .begin(
                SyncBeginRequest {
                    client_version: sync_client_version().into(),
                }
                .try_into_sync_request()?,
            )
            .await?
            .json_result()?;

        debug!("server usn was {}", server_usn);
        Ok(server_usn)
    }

    /// Make sure media DB is up to date.
    fn register_changes(&mut self) -> Result<(Vec<String>, Vec<String>)> {
        let progress_cb = |checked| self.progress.update(true, |p| p.checked = checked).is_ok();
        let (changed, removed) = ChangeTracker::new(self.mgr.media_folder.as_path(), progress_cb)
            .register_changes(&self.mgr.db)?;
        if let Some(index) = &self.index {
            for fname in &changed {
                index.insert(fname, false);
            }
            for fname in &removed {
                index.remove(fname);
            }
        }
        if !changed.is_empty() || !removed.is_empty() {
            self.index_dirty = true;
        }
        Ok((changed, removed))
    }

    async fn fetch_priority_now(&mut self) -> Result<()> {
        let Some((priority, is_focus)) = self.on_demand_priority() else {
            return Ok(());
        };
        let version = priority.version();
        let last = if is_focus {
            &mut self.last_focus_version
        } else {
            &mut self.last_priority_version
        };
        if *last == version {
            return Ok(());
        }
        *last = version;
        let snapshot = priority.snapshot();
        if snapshot.is_empty() {
            return Ok(());
        }
        if self.priority_attempted.len() > PRIORITY_CACHE_LIMIT {
            self.priority_attempted.clear();
        }
        let mut missing = Vec::new();
        for name in snapshot {
            let name = normalize_filename(&name).into_owned();
            if name.is_empty() || !self.priority_attempted.insert(name.clone()) {
                continue;
            }
            if let Some(existing) = locate_existing(&self.mgr.media_folder, &name) {
                if let Some(index) = &self.index {
                    let unmigrated = existing.parent() == Some(self.mgr.media_folder.as_path());
                    index.insert(&name, unmigrated);
                }
                priority.remove(&[name]);
            } else {
                missing.push(name);
            }
        }
        if missing.is_empty() {
            return Ok(());
        }
        debug!(count = missing.len(), "fetching prioritized files directly");
        let mut pending: Vec<AddedFile> = Vec::new();
        let mut remaining = missing.as_slice();
        let mut chunk = 4;
        let mut result = Ok(());
        while !remaining.is_empty() {
            if self.client.is_cancelled() || self.progress.check_cancelled().is_err() {
                result = Err(AnkiError::Interrupted);
                break;
            }
            let (files, rest) = remaining.split_at(chunk.min(remaining.len()));
            remaining = rest;
            chunk = DOWNLOAD_BATCH_FILES;
            let request = DownloadFilesRequest {
                files: files.to_vec(),
            }
            .try_into_sync_request()?;
            let scratch = self.download_scratch_path();
            let downloaded = match self.client.download_files_to_file(request, &scratch).await {
                Some(Ok(bytes)) => {
                    self.progress
                        .update(false, |p| p.downloaded_bytes += bytes)?;
                    let interrupted = self.write_zip_file(&scratch, &mut pending);
                    let _ = std::fs::remove_file(&scratch);
                    match interrupted {
                        Ok(cancelled) => Ok(cancelled),
                        Err(e) => Err(e),
                    }
                }
                Some(Err(e)) => {
                    let _ = std::fs::remove_file(&scratch);
                    Err(e.into())
                }
                None => {
                    let request = DownloadFilesRequest {
                        files: files.to_vec(),
                    }
                    .try_into_sync_request()?;
                    match self.client.download_files(request).await {
                        Ok(response) => {
                            let data = response.data;
                            self.progress
                                .update(false, |p| p.downloaded_bytes += data.len() as u64)?;
                            self.write_zip(data, &mut pending)
                        }
                        Err(e) => Err(e.into()),
                    }
                }
            };
            match downloaded {
                Ok(true) => {
                    result = Err(AnkiError::Interrupted);
                    break;
                }
                Ok(false) => {
                    for name in files {
                        if locate_existing(&self.mgr.media_folder, name).is_some() {
                            if self.priority_fetched.len() > PRIORITY_CACHE_LIMIT {
                                self.priority_fetched.clear();
                            }
                            self.priority_fetched.insert(name.clone());
                        }
                    }
                    priority.remove(files);
                }
                Err(error) => {
                    if self.client.is_cancelled() || matches!(error, AnkiError::Interrupted) {
                        result = Err(AnkiError::Interrupted);
                        break;
                    }
                    debug!(?error, "direct priority download failed");
                    for name in files {
                        self.priority_attempted.remove(name);
                    }
                }
            }
        }
        let flush = self.flush_additions(&mut pending);
        result.and(flush)
    }

    async fn fetch_changes(&mut self, mut meta: MediaDatabaseMetadata) -> Result<()> {
        let mut last_usn = meta.last_sync_usn;
        loop {
            self.progress.check_cancelled()?;
            self.pause_point().await?;
            self.fetch_priority_now().await?;
            debug!(start_usn = ?last_usn, "fetching record batch");

            let batch = self
                .client
                .media_changes(MediaChangesRequest { last_usn }.try_into_sync_request()?)
                .await?
                .json_result()?;
            if batch.is_empty() {
                debug!("empty batch, done");
                break;
            }
            last_usn = batch.last().unwrap().usn;

            self.progress.update(false, |p| p.checked += batch.len())?;

            let (to_download, to_delete, to_remove_pending) =
                changes::determine_required_changes(&self.mgr.db, batch)?;
            let batch_start_dirmod = if Self::SCAN_LOCAL_CHANGES {
                combined_folder_mtime(&self.mgr.media_folder)?
            } else {
                0
            };

            // file removal
            self.mgr.remove_files(to_delete.as_slice())?;
            if let Some(index) = &self.index {
                for fname in &to_delete {
                    index.remove(fname);
                }
            }
            if !to_delete.is_empty() {
                self.index_dirty = true;
            }
            self.progress
                .update(false, |p| p.downloaded_deletions += to_delete.len())?;

            // file download
            if !to_download.is_empty() {
                self.download_files(to_download).await?;
            }

            // then update the DB
            let dirmod = if Self::SCAN_LOCAL_CHANGES {
                combined_folder_mtime(&self.mgr.media_folder)?
            } else {
                0
            };
            if self.progress.check_cancelled().is_err() {
                return Err(AnkiError::Interrupted);
            }
            self.mgr.db.transact(|ctx| {
                ctx.record_clean(&to_remove_pending)?;
                ctx.record_removals(&to_delete)?;

                // update usn
                let mut batch_meta = ctx.get_meta()?;
                batch_meta.last_sync_usn = last_usn;
                if batch_start_dirmod != 0 && batch_meta.folder_mtime == batch_start_dirmod {
                    batch_meta.folder_mtime = dirmod;
                }
                ctx.set_meta(&batch_meta)?;
                meta.last_sync_usn = batch_meta.last_sync_usn;
                meta.folder_mtime = batch_meta.folder_mtime;

                Ok(())
            })?;
        }
        Ok(())
    }

    async fn download_files(&mut self, mut to_download: Vec<String>) -> Result<()> {
        let mut pending: Vec<AddedFile> = Vec::new();
        let result = self
            .download_files_inner(&mut to_download, &mut pending)
            .await;
        let flush = self.flush_additions(&mut pending);
        result.and(flush)
    }

    /// Downloads are handled in small groups so newly-prioritized files (for
    /// the card the user just opened) can jump ahead of the remaining queue.
    async fn download_files_inner(
        &mut self,
        to_download: &mut Vec<String>,
        pending: &mut Vec<AddedFile>,
    ) -> Result<()> {
        let mut consecutive_failures = 0u32;
        while !to_download.is_empty() {
            self.pause_point().await?;
            self.fetch_priority_now().await?;
            if !self.priority_fetched.is_empty() {
                let fetched = &self.priority_fetched;
                to_download.retain(|name| !fetched.contains(name));
                if to_download.is_empty() {
                    break;
                }
            }
            self.apply_priority(to_download);
            let group: Vec<String> = to_download
                .drain(..DOWNLOAD_BATCH_FILES.min(to_download.len()))
                .collect();
            let batches: Vec<Vec<String>> = group
                .chunks(DOWNLOAD_BATCH_FILES)
                .map(|chunk| chunk.to_vec())
                .collect();
            match self.download_group(batches, pending).await {
                Ok(()) => {
                    consecutive_failures = 0;
                }
                Err(e) if is_network_error(&e) && consecutive_failures < 5 => {
                    consecutive_failures += 1;
                    to_download.splice(0..0, group.clone());
                    std::thread::sleep(Duration::from_secs(2));
                    self.pause_point().await?;
                    continue;
                }
                Err(e) => return Err(e),
            }
            if let Some(priority) = &self.priority {
                priority.remove(&group);
            }
            std::thread::yield_now();
            std::thread::sleep(Duration::from_millis(12));
        }
        Ok(())
    }

    fn apply_priority(&self, to_download: &mut [String]) {
        let Some(priority) = &self.priority else {
            return;
        };
        if priority.is_empty() {
            return;
        }
        let wanted = priority.snapshot();
        if wanted.is_empty() {
            return;
        }
        let rank: std::collections::HashMap<String, usize> = wanted
            .iter()
            .enumerate()
            .map(|(index, name)| (normalize_filename(name).into_owned(), index))
            .collect();
        let mut front = 0usize;
        for i in 0..to_download.len() {
            if rank.contains_key(to_download[i].as_str()) {
                to_download.swap(front, i);
                front += 1;
            }
        }
        if front > 1 {
            to_download[..front].sort_by_key(|name| rank.get(name.as_str()).copied().unwrap_or(usize::MAX));
        }
    }

    async fn download_group(
        &mut self,
        batches: Vec<Vec<String>>,
        pending: &mut Vec<AddedFile>,
    ) -> Result<()> {
        let total = batches.len();
        // A download worker keeps fetching the next batch while this thread
        // unpacks the previous zip onto the SD card.
        if self.parallel_downloads {
            if let Some(session) = self
                .client
                .download_files_parallel(&batches, self.download_connections)
            {
                return self.consume_parallel(session, total, pending).await;
            }
        }

        self.download_group_serial(&batches, pending).await
    }

    async fn download_group_serial(
        &mut self,
        batches: &[Vec<String>],
        pending: &mut Vec<AddedFile>,
    ) -> Result<()> {
        for files in batches {
            if self.client.is_cancelled() || self.progress.check_cancelled().is_err() {
                return Err(AnkiError::Interrupted);
            }
            let scratch = self.download_scratch_path();
            let mut attempt = 0;
            enum BatchPayload {
                File(u64),
                Memory(Vec<u8>),
            }
            let payload = loop {
                if self.client.is_cancelled() || self.progress.check_cancelled().is_err() {
                    return Err(AnkiError::Interrupted);
                }
                let req = DownloadFilesRequest {
                    files: files.clone(),
                }
                .try_into_sync_request()?;
                let outcome = match self.client.download_files_to_file(req, &scratch).await {
                    Some(Ok(bytes)) => Ok(BatchPayload::File(bytes)),
                    Some(Err(e)) => {
                        let _ = std::fs::remove_file(&scratch);
                        Err(e)
                    }
                    None => self
                        .client
                        .download_files(
                            DownloadFilesRequest {
                                files: files.clone(),
                            }
                            .try_into_sync_request()?,
                        )
                        .await
                        .map(|r| BatchPayload::Memory(r.data)),
                };
                match outcome {
                    Ok(payload) => break payload,
                    Err(e) => {
                        let net_err: AnkiError = e.into();
                        if attempt < BATCH_RETRIES && is_network_error(&net_err) {
                            attempt += 1;
                            debug!(?net_err, attempt, "media batch failed, retrying");
                            std::thread::sleep(Duration::from_millis(500 * attempt as u64));
                            self.pause_point().await?;
                        } else {
                            return Err(net_err);
                        }
                    }
                }
            };
            match payload {
                BatchPayload::File(bytes) => {
                    self.progress
                        .update(false, |p| p.downloaded_bytes += bytes)?;
                    let extracted = self.write_zip_file(&scratch, pending);
                    let _ = std::fs::remove_file(&scratch);
                    if extracted? {
                        return Err(AnkiError::Interrupted);
                    }
                }
                BatchPayload::Memory(zip_data) => {
                    self.progress
                        .update(false, |p| p.downloaded_bytes += zip_data.len() as u64)?;
                    if self.write_zip(zip_data, pending)? {
                        return Err(AnkiError::Interrupted);
                    }
                }
            }
        }
        Ok(())
    }

    async fn consume_parallel(
        &mut self,
        session: ParallelDownloadSession,
        total: usize,
        pending: &mut Vec<AddedFile>,
    ) -> Result<()> {
        let mut received = 0usize;
        let mut cancelled = false;
        let mut retry: Vec<String> = Vec::new();
        while received < total {
            if self.client.is_cancelled() || self.progress.check_cancelled().is_err() {
                cancelled = true;
                break;
            }
            if self.bulk_paused() {
                if let Err(e) = self.fetch_priority_now().await {
                    if matches!(e, AnkiError::Interrupted) {
                        cancelled = true;
                        break;
                    }
                }
                std::thread::sleep(Duration::from_millis(20));
                continue;
            }
            let event = session.recv_timeout(Duration::from_millis(100));
            match event {
                Ok(MediaDownloadEvent::Downloaded { data }) => {
                    received += 1;
                    self.progress
                        .update(false, |p| p.downloaded_bytes += data.len() as u64)?;
                    if self.write_zip(data, pending)? {
                        cancelled = true;
                        break;
                    }
                }
                Ok(MediaDownloadEvent::Failed { error, files }) => {
                    received += 1;
                    debug!(error, "media batch failed, queueing serial retry");
                    retry.extend(files);
                }
                Err(mpsc::RecvTimeoutError::Timeout) => {}
                Err(mpsc::RecvTimeoutError::Disconnected) => {
                    if received >= total {
                        break;
                    }
                    return Err(AnkiError::sync_error(
                        "media downloader stopped before all batches arrived",
                        SyncErrorKind::Other,
                    ));
                }
            }
        }
        drop(session);
        if cancelled {
            return Err(AnkiError::Interrupted);
        }
        if !retry.is_empty()
            && !self.client.is_cancelled()
            && self.progress.check_cancelled().is_ok()
        {
            let batches: Vec<Vec<String>> = retry
                .chunks(MAX_MEDIA_FILES_IN_ZIP)
                .map(<[_]>::to_vec)
                .collect();
            // Serial fallback for the failed batches; inlined instead of
            // recursing through download_group to guarantee a serial path.
            self.download_group_serial(&batches, pending).await?;
        }
        Ok(())
    }

    fn write_zip(&mut self, zip_data: Vec<u8>, pending: &mut Vec<AddedFile>) -> Result<bool> {
        let folder = self.mgr.media_folder.clone();
        let client = &self.client;
        let progress = &mut self.progress;
        let extracted = download::extract_into_media_folder(&folder, zip_data, &mut || {
            client.is_cancelled() || progress.check_cancelled().is_err()
        })?;
        self.finish_extracted(extracted, pending)
    }

    fn write_zip_file(
        &mut self,
        zip_path: &std::path::Path,
        pending: &mut Vec<AddedFile>,
    ) -> Result<bool> {
        let folder = self.mgr.media_folder.clone();
        let client = &self.client;
        let progress = &mut self.progress;
        let extracted = download::extract_file_into_media_folder(&folder, zip_path, &mut || {
            client.is_cancelled() || progress.check_cancelled().is_err()
        })?;
        self.finish_extracted(extracted, pending)
    }

    fn finish_extracted(
        &mut self,
        extracted: download::ExtractedDownload,
        pending: &mut Vec<AddedFile>,
    ) -> Result<bool> {
        let downloaded = extracted.added.len();
        let cancelled = extracted.cancelled;
        let unpacked_bytes = extracted.bytes;
        if let Some(index) = &self.index {
            for added in &extracted.added {
                index.insert(&added.fname, false);
            }
        }
        if downloaded > 0 {
            self.index_dirty = true;
            self.index_dirty_count += downloaded;
        }
        pending.extend(extracted.added);
        self.progress.update(false, |p| {
            p.downloaded_files += downloaded;
            p.unpacked_bytes += unpacked_bytes;
        })?;
        self.notify_changed();
        if pending.len() >= DOWNLOAD_COMMIT_FILES {
            self.flush_additions(pending)?;
        }
        if self.index_dirty_count >= INDEX_SAVE_INTERVAL_FILES {
            self.save_index();
        }
        std::thread::yield_now();
        std::thread::sleep(Duration::from_millis(8));
        Ok(cancelled)
    }

    fn flush_additions(&mut self, pending: &mut Vec<AddedFile>) -> Result<()> {
        if pending.is_empty() {
            return Ok(());
        }
        let additions = std::mem::take(pending);
        self.mgr.db.transact(|ctx| {
            ctx.record_additions(additions)?;
            Ok(())
        })?;
        Ok(())
    }

    async fn send_changes(&mut self) -> Result<()> {
        loop {
            self.progress.check_cancelled()?;
            self.pause_point().await?;
            let pending: Vec<MediaEntry> = self
                .mgr
                .db
                .get_pending_uploads(MAX_MEDIA_FILES_IN_ZIP as u32)?;
            if pending.is_empty() {
                break;
            }

            let data_for_zip = gather_zip_data_for_upload(
                &self.mgr.db,
                &self.mgr.media_folder,
                &pending,
                &mut || !self.client.is_cancelled() && self.progress.check_cancelled().is_ok(),
            )?;
            let zip_bytes = match data_for_zip {
                None => {
                    // discard zip info and retry batch - not particularly
                    // efficient, but this is a corner case
                    self.progress
                        .update(false, |p| p.checked += pending.len())?;
                    continue;
                }
                Some(data) => zip_files_for_upload(data)?,
            };

            self.progress
                .update(false, |p| p.uploaded_bytes += zip_bytes.len() as u64)?;
            let reply = self
                .client
                .upload_changes(zip_bytes.try_into_sync_request()?)
                .await?
                .json_result()?;

            let (processed_files, processed_deletions): (Vec<_>, Vec<_>) = pending
                .into_iter()
                .take(reply.processed)
                .partition(|e| e.sha1.is_some());

            self.progress.update(false, |p| {
                p.uploaded_files += processed_files.len();
                p.uploaded_deletions += processed_deletions.len();
            })?;

            let fnames: Vec<_> = processed_files
                .into_iter()
                .chain(processed_deletions)
                .map(|e| e.fname)
                .collect();
            let fname_cnt = fnames.len() as i32;
            self.mgr.db.transact(|ctx| {
                ctx.record_clean(fnames.as_slice())?;
                let mut meta = ctx.get_meta()?;
                if meta.last_sync_usn.0 + fname_cnt == reply.current_usn.0 {
                    meta.last_sync_usn = reply.current_usn;
                    ctx.set_meta(&meta)?;
                } else {
                    debug!(
                        "server usn {} is not {}, skipping usn update",
                        reply.current_usn,
                        meta.last_sync_usn.0 + fname_cnt
                    );
                }
                Ok(())
            })?;
        }

        Ok(())
    }

    async fn finalize_sync(&mut self) -> Result<()> {
        let local = self.mgr.db.count()?;
        let msg = self
            .client
            .media_sanity_check(SanityCheckRequest { local }.try_into_sync_request()?)
            .await?
            .json_result()?;
        if msg == MediaSanityCheckResponse::Ok {
            Ok(())
        } else {
            self.mgr.db.transact(|ctx| ctx.force_resync())?;
            Err(AnkiError::sync_error("", SyncErrorKind::ResyncRequired))
        }
    }
}
