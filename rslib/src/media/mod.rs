// Copyright: Ankitects Pty Ltd and contributors
// License: GNU AGPL, version 3 or later; http://www.gnu.org/licenses/agpl.html

pub mod check;
pub mod files;
mod service;

use std::borrow::Cow;
use std::path::Path;
use std::path::PathBuf;
use std::sync::atomic::AtomicBool;
use std::sync::atomic::AtomicU32;
use std::sync::Arc;

use anki_io::create_dir_all;
#[cfg(feature = "network")]
use reqwest::Client;

use crate::media::files::add_data_to_folder_uniquely;
use crate::media::files::combined_folder_mtime;
use crate::media::files::mtime_as_i64;
use crate::media::files::remove_files;
use crate::media::files::sha1_of_data;
use crate::media::files::sharded_path;
use crate::media::files::MediaNameIndex;
use crate::prelude::*;
use crate::progress::ThrottlingProgressHandler;
#[cfg(feature = "network")]
use crate::sync::http_client::HttpSyncClient;
#[cfg(feature = "network")]
use crate::sync::login::SyncAuth;
use crate::sync::media::database::client::changetracker::ChangeTracker;
pub use crate::sync::media::database::client::Checksums;
use crate::sync::media::database::client::MediaDatabase;
use crate::sync::media::database::client::MediaEntry;
use crate::sync::media::progress::MediaSyncProgress;
use crate::sync::media::protocol::MediaPriority;
use crate::sync::media::protocol::MediaSyncProtocol;
use crate::sync::media::syncer::MediaSyncer;

pub type Sha1Hash = [u8; 20];

/// Options for a caller-provided media sync: how many connections to use,
/// which files to prioritize, and a counter the caller can watch to notice
/// newly written files.
pub struct MediaSyncConfig {
    pub max_connections: usize,
    pub priority: Option<Arc<MediaPriority>>,
    pub changed: Option<Arc<AtomicU32>>,
    pub paused: Option<Arc<AtomicBool>>,
    pub focus: Option<Arc<MediaPriority>>,
    pub index: Option<Arc<MediaNameIndex>>,
    pub parallel_downloads: bool,
}

impl Default for MediaSyncConfig {
    fn default() -> Self {
        Self {
            max_connections: 1,
            priority: None,
            changed: None,
            paused: None,
            focus: None,
            index: None,
            parallel_downloads: true,
        }
    }
}

impl Collection {
    pub fn media(&self) -> Result<MediaManager> {
        MediaManager::new(&self.media_folder, &self.media_db)
    }
}

pub struct MediaManager {
    pub(crate) db: MediaDatabase,
    pub(crate) media_folder: PathBuf,
}

impl MediaManager {
    pub fn new<P, P2>(media_folder: P, media_db: P2) -> Result<Self>
    where
        P: Into<PathBuf>,
        P2: AsRef<Path>,
    {
        let media_folder = media_folder.into();
        if media_folder.as_os_str().is_empty() {
            invalid_input!("attempted media operation without media folder set");
        }
        create_dir_all(&media_folder)?;
        Ok(MediaManager {
            db: MediaDatabase::new(media_db.as_ref())?,
            media_folder,
        })
    }

    /// Add a file to the media folder.
    ///
    /// If a file with differing contents already exists, a hash will be
    /// appended to the name.
    ///
    /// Also notes the file in the media database.
    pub fn add_file<'a>(&self, desired_name: &'a str, data: &[u8]) -> Result<Cow<'a, str>> {
        let data_hash = sha1_of_data(data);

        self.transact(|db| {
            let chosen_fname =
                add_data_to_folder_uniquely(&self.media_folder, desired_name, data, data_hash)?;
            let file_mtime = mtime_as_i64(sharded_path(&self.media_folder, chosen_fname.as_ref()))?;

            let existing_entry = db.get_entry(&chosen_fname)?;
            let new_sha1 = Some(data_hash);

            let entry_update_required = existing_entry.map(|e| e.sha1 != new_sha1).unwrap_or(true);

            if entry_update_required {
                db.set_entry(&MediaEntry {
                    fname: chosen_fname.to_string(),
                    sha1: new_sha1,
                    mtime: file_mtime,
                    sync_required: true,
                })?;
            }

            Ok(chosen_fname)
        })
    }

    pub fn remove_files<S>(&self, filenames: &[S]) -> Result<()>
    where
        S: AsRef<str> + std::fmt::Debug,
    {
        self.transact(|db| {
            remove_files(&self.media_folder, filenames)?;
            for fname in filenames {
                if let Some(mut entry) = db.get_entry(fname.as_ref())? {
                    entry.sha1 = None;
                    entry.mtime = 0;
                    entry.sync_required = true;
                    db.set_entry(&entry)?;
                }
            }
            Ok(())
        })
    }

    /// Opens a transaction and manages folder mtime, so user should perform not
    /// only db ops, but also all file ops inside the closure.
    pub(crate) fn transact<T>(&self, func: impl FnOnce(&MediaDatabase) -> Result<T>) -> Result<T> {
        let start_folder_mtime = combined_folder_mtime(&self.media_folder)?;
        self.db.transact(|db| {
            let out = func(db)?;

            let mut meta = db.get_meta()?;
            if meta.folder_mtime == start_folder_mtime {
                // if media db was in sync with folder prior to this add,
                // we can keep it in sync
                meta.folder_mtime = combined_folder_mtime(&self.media_folder)?;
                db.set_meta(&meta)?;
            } else {
                // otherwise, leave it alone so that other pending changes
                // get picked up later
            }

            Ok(out)
        })
    }

    /// Set entry for a newly added file. Caller must ensure transaction.
    pub(crate) fn add_entry(&self, fname: impl Into<String>, sha1: [u8; 20]) -> Result<()> {
        let fname = fname.into();
        let mtime = mtime_as_i64(sharded_path(&self.media_folder, &fname))?;
        self.db.set_entry(&MediaEntry {
            fname,
            mtime,
            sha1: Some(sha1),
            sync_required: true,
        })
    }

    #[cfg(feature = "network")]
    /// Sync media.
    pub async fn sync_media(
        self,
        progress: ThrottlingProgressHandler<MediaSyncProgress>,
        auth: SyncAuth,
        client: Client,
        server_usn: Option<Usn>,
    ) -> Result<()> {
        let client = HttpSyncClient::new(auth, client);
        let mut syncer = MediaSyncer::new(self, progress, client)?;
        syncer.sync(server_usn).await
    }

    /// Sync media over a caller-provided transport.
    pub async fn sync_media_with<T: crate::sync::media::protocol::MediaSyncProtocol>(
        self,
        progress: ThrottlingProgressHandler<MediaSyncProgress>,
        client: T,
        server_usn: Option<Usn>,
    ) -> Result<()> {
        let mut syncer = MediaSyncer::new(self, progress, client)?;
        syncer.sync(server_usn).await
    }

    /// Sync media over a caller-provided transport, using up to
    /// `max_connections` parallel download connections.
    pub async fn sync_media_with_options<T: crate::sync::media::protocol::MediaSyncProtocol>(
        self,
        progress: ThrottlingProgressHandler<MediaSyncProgress>,
        client: T,
        server_usn: Option<Usn>,
        max_connections: usize,
    ) -> Result<()> {
        let mut syncer = MediaSyncer::new(self, progress, client)?;
        syncer.set_download_connections(max_connections);
        syncer.sync(server_usn).await
    }

    /// Sync media with connection, priority and change-notification hooks.
    pub async fn sync_media_with_config<T: MediaSyncProtocol>(
        self,
        progress: ThrottlingProgressHandler<MediaSyncProgress>,
        client: T,
        server_usn: Option<Usn>,
        config: MediaSyncConfig,
    ) -> Result<()> {
        let mut syncer = MediaSyncer::new(self, progress, client)?;
        syncer.set_download_connections(config.max_connections);
        syncer.set_priority(config.priority);
        syncer.set_changed(config.changed);
        syncer.set_paused(config.paused);
        syncer.set_focus(config.focus);
        syncer.set_index(config.index);
        syncer.set_parallel_downloads(config.parallel_downloads);
        syncer.sync(server_usn).await
    }

    /// Move unsharded root files into shard subdirectories. The caller
    /// decides when bulk I/O is acceptable via `keep_going`.
    pub fn migrate_to_shards(
        &self,
        keep_going: &mut impl FnMut() -> bool,
        max_files: usize,
    ) -> Result<Vec<String>> {
        crate::media::files::migrate_media_to_shards(&self.media_folder, keep_going, max_files)
    }

    /// Persist the shared media index next to the media folder.
    pub fn save_media_index(&self, index: &MediaNameIndex) -> Result<()> {
        crate::media::files::save_media_name_index(index, &self.media_folder)
    }

    /// Filenames recorded in the media database, read without scanning the
    /// media folder.
    pub fn known_file_names(&self) -> Result<Vec<String>> {
        self.db.file_names()
    }

    pub fn all_checksums_after_checking(
        &self,
        progress: impl FnMut(usize) -> bool,
    ) -> Result<Checksums> {
        ChangeTracker::new(&self.media_folder, progress).register_changes(&self.db)?;
        self.db.all_registered_checksums()
    }

    pub fn checksum_getter(&self) -> impl FnMut(&str) -> Result<Option<Sha1Hash>> + '_ {
        |fname: &str| {
            self.db
                .get_entry(fname)
                .map(|opt| opt.and_then(|entry| entry.sha1))
        }
    }

    pub fn register_changes(
        &self,
        progress: &mut impl FnMut(usize) -> bool,
    ) -> Result<(Vec<String>, Vec<String>)> {
        ChangeTracker::new(&self.media_folder, progress).register_changes(&self.db)
    }

    /// All checksums without registering changes first.
    #[cfg(test)]
    pub(crate) fn all_checksums_as_is(&self) -> Checksums {
        self.db.all_registered_checksums().unwrap()
    }
}
