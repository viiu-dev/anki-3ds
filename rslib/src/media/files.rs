// Copyright: Anki Software, LLC and contributors
// License: GNU AGPL, version 3 or later; http://www.gnu.org/licenses/agpl.html

use std::borrow::Cow;
use std::collections::HashSet;
use std::fs;
use std::fs::FileTimes;
use std::io;
use std::io::Read;
use std::io::Write;
use std::path::Path;
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::LazyLock;
use std::sync::Mutex;
use std::time;

use anki_io::create_dir;
use anki_io::create_dir_all;
use anki_io::open_file;
use anki_io::set_file_times;
use anki_io::write_file;
use anki_io::FileIoError;
use anki_io::FileIoSnafu;
use anki_io::FileOp;
use regex::Regex;
use sha1::Digest;
use sha1::Sha1;
use tracing::debug;
use unic_ucd_category::GeneralCategory;
use unicode_normalization::is_nfc;
use unicode_normalization::UnicodeNormalization;

use crate::prelude::*;
use crate::sync::media::MAX_MEDIA_FILENAME_LENGTH;

static WINDOWS_DEVICE_NAME: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(
        r"(?xi)
            # starting with one of the following names
            ^
            (
                CON | PRN | AUX | NUL | COM[1-9] | LPT[1-9]
            )
            # either followed by a dot, or no extension
            (
                \. | $
            )
        ",
    )
    .unwrap()
});
static WINDOWS_TRAILING_CHAR: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(
        r"(?x)
            # filenames can't end with a space or period
            (
                \x20 | \.
            )    
            $
            ",
    )
    .unwrap()
});
pub(crate) static NONSYNCABLE_FILENAME: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(
        r#"(?xi)
            ^
            (:?
                thumbs.db | .ds_store
            )
            $
            "#,
    )
    .unwrap()
});

/// Shard subdirectory for a media file, from a hash of its filename.
pub fn shard_dir_name(fname: &str) -> String {
    let mut hash: u32 = 0x811c_9dc5;
    for byte in fname.as_bytes() {
        hash ^= u32::from(*byte);
        hash = hash.wrapping_mul(0x0100_0193);
    }
    format!("{:02x}", (hash >> 24) as u8)
}

/// True when media files are stored in shard subdirectories. Only the 3DS
/// build shards; host builds keep the traditional flat layout.
pub fn sharding_enabled() -> bool {
    cfg!(target_os = "horizon")
}

/// Full path of a media file inside its shard subdirectory.
pub fn sharded_path(folder: &Path, fname: &str) -> PathBuf {
    if sharding_enabled() {
        folder.join(shard_dir_name(fname)).join(fname)
    } else {
        folder.join(fname)
    }
}

/// Existing media file, preferring its shard copy.
pub fn locate_existing(folder: &Path, fname: &str) -> Option<PathBuf> {
    let sharded = sharded_path(folder, fname);
    if sharded.is_file() {
        return Some(sharded);
    }
    let root = folder.join(fname);
    if root.is_file() {
        return Some(root);
    }
    None
}

fn open_media_file(folder: &Path, fname: &str) -> std::result::Result<fs::File, FileIoError> {
    match anki_io::open_file(sharded_path(folder, fname)) {
        Err(e) if e.is_not_found() => anki_io::open_file(folder.join(fname)),
        res => res,
    }
}

/// Bare filenames and full paths for every media file, including sharded
/// ones. A name present in both the root and a shard resolves to the shard.
pub fn media_file_entries(folder: &Path) -> Result<Vec<(String, PathBuf)>> {
    media_file_entries_checked(folder, &mut || true)
}

/// [media_file_entries], but polls `keep_going` between directory entries.
pub fn media_file_entries_checked(
    folder: &Path,
    keep_going: &mut impl FnMut() -> bool,
) -> Result<Vec<(String, PathBuf)>> {
    let root_entries: Vec<fs::DirEntry> = fs::read_dir(folder)?.collect::<io::Result<_>>()?;
    let mut seen = HashSet::new();
    let mut out = Vec::new();
    for entry in &root_entries {
        if !keep_going() {
            return Err(AnkiError::Interrupted);
        }
        if !entry.path().is_dir() {
            continue;
        }
        let Ok(sub) = fs::read_dir(entry.path()) else {
            continue;
        };
        for entry in sub.flatten() {
            if !keep_going() {
                return Err(AnkiError::Interrupted);
            }
            let path = entry.path();
            if path.is_dir() {
                continue;
            }
            if let Some(name) = path.file_name().and_then(|name| name.to_str()) {
                if seen.insert(name.to_owned()) {
                    out.push((name.to_owned(), path));
                }
            }
        }
    }
    for entry in &root_entries {
        let path = entry.path();
        if path.is_dir() {
            continue;
        }
        if let Some(name) = path.file_name().and_then(|name| name.to_str()) {
            if seen.insert(name.to_owned()) {
                out.push((name.to_owned(), path));
            }
        }
    }
    Ok(out)
}

/// Count of media files, including sharded ones.
pub fn media_file_count(folder: &Path) -> Result<usize> {
    Ok(media_file_entries(folder)?.len())
}

/// Cache file holding the media index, stored next to the media folder.
pub fn media_index_cache_path(folder: &Path) -> PathBuf {
    let stem = folder
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or("media");
    folder.with_file_name(format!("{stem}.idx"))
}

const MEDIA_INDEX_VERSION: &str = "1";
const MEDIA_INDEX_SHARDED: char = 's';
const MEDIA_INDEX_UNMIGRATED: char = 'u';

/// Load a previously saved media index without touching the media folder.
/// Returns None when the cache is missing or invalid.
pub fn load_media_name_index(folder: &Path) -> Option<Arc<MediaNameIndex>> {
    let data = String::from_utf8(std::fs::read(media_index_cache_path(folder)).ok()?).ok()?;
    let mut lines = data.lines();
    if lines.next() != Some(MEDIA_INDEX_VERSION) {
        return None;
    }
    let index = MediaNameIndex::new();
    {
        let mut inner = index
            .inner
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        for line in lines {
            let Some((flag, name)) = line.split_once('\t') else {
                continue;
            };
            if name.is_empty() {
                continue;
            }
            inner.files.insert(name.to_owned());
            if flag.starts_with(MEDIA_INDEX_UNMIGRATED) {
                inner.unmigrated.insert(name.to_owned());
            }
        }
    }
    Some(index)
}

/// Persist the media index to a single compact newline-delimited file.
pub fn save_media_name_index(index: &MediaNameIndex, folder: &Path) -> Result<()> {
    let data = {
        let inner = index
            .inner
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let mut text = String::with_capacity(inner.files.len() * 48 + 4);
        text.push_str(MEDIA_INDEX_VERSION);
        text.push('\n');
        for name in &inner.files {
            let flag = if inner.unmigrated.contains(name) {
                MEDIA_INDEX_UNMIGRATED
            } else {
                MEDIA_INDEX_SHARDED
            };
            text.push(flag);
            text.push('\t');
            text.push_str(name);
            text.push('\n');
        }
        text
    };
    let cache_path = media_index_cache_path(folder);
    let temp_path = cache_path.with_extension("idx.tmp");
    fs::write(&temp_path, data.as_bytes())?;
    let _ = fs::remove_file(&cache_path);
    fs::rename(&temp_path, &cache_path)?;
    Ok(())
}

#[derive(Default)]
struct MediaNameIndexInner {
    files: HashSet<String>,
    unmigrated: HashSet<String>,
}

/// Concurrent set of media filenames present on disk, shared between the
/// background sync worker and the UI thread. The worker pushes additions,
/// removals and migration updates; readers only do in-memory lookups.
#[derive(Default)]
pub struct MediaNameIndex {
    inner: Mutex<MediaNameIndexInner>,
}

impl MediaNameIndex {
    pub fn new() -> Arc<Self> {
        Arc::new(Self::default())
    }

    pub fn insert(&self, fname: &str, unmigrated: bool) {
        let mut inner = self
            .inner
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        inner.files.insert(fname.to_owned());
        if unmigrated {
            inner.unmigrated.insert(fname.to_owned());
        } else {
            inner.unmigrated.remove(fname);
        }
    }

    pub fn remove(&self, fname: &str) {
        let mut inner = self
            .inner
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        inner.files.remove(fname);
        inner.unmigrated.remove(fname);
    }

    pub fn mark_migrated(&self, fname: &str) {
        self.inner
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .unmigrated
            .remove(fname);
    }

    pub fn contains(&self, fname: &str) -> bool {
        self.inner
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .files
            .contains(fname)
    }

    pub fn is_unmigrated(&self, fname: &str) -> bool {
        self.inner
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .unmigrated
            .contains(fname)
    }

    pub fn len(&self) -> usize {
        self.inner
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .files
            .len()
    }
}

/// Latest modification time of the folder or any shard subdirectory.
pub(crate) fn combined_folder_mtime(folder: &Path) -> Result<i64> {
    combined_folder_mtime_checked(folder, &mut || true)
}

/// [combined_folder_mtime], aborting the shard walk when `keep_going` is
/// false. The 3DS FAT layer reports no usable directory mtime for shards, so
/// it only stats the media root instead of walking every shard folder.
pub(crate) fn combined_folder_mtime_checked(
    folder: &Path,
    keep_going: &mut impl FnMut() -> bool,
) -> Result<i64> {
    let mut latest = mtime_as_i64(folder)?;
    if cfg!(target_os = "horizon") {
        return Ok(latest);
    }
    if let Ok(entries) = fs::read_dir(folder) {
        for entry in entries.flatten() {
            if !keep_going() {
                return Err(AnkiError::Interrupted);
            }
            let path = entry.path();
            if path.is_dir() {
                if let Ok(mtime) = mtime_as_i64(&path) {
                    latest = latest.max(mtime);
                }
            }
        }
    }
    Ok(latest)
}

fn remove_superseded_root_copy(folder: &Path, fname: &str) {
    let root = folder.join(fname);
    if root != sharded_path(folder, fname) {
        let _ = fs::remove_file(root);
    }
}

fn files_have_same_contents(first: &Path, second: &Path) -> Result<bool> {
    let first_meta = fs::metadata(first)?;
    let second_meta = fs::metadata(second)?;
    if first_meta.len() != second_meta.len() {
        return Ok(false);
    }
    Ok(sha1_of_file(first)? == sha1_of_file(second)?)
}

/// Move root files into shard subdirectories, returning the moved names.
/// Moves at most `max_files`, and stops with [AnkiError::Interrupted] when
/// `keep_going` reports false.
pub fn migrate_media_to_shards(
    folder: &Path,
    keep_going: &mut impl FnMut() -> bool,
    max_files: usize,
) -> Result<Vec<String>> {
    if !sharding_enabled() {
        return Ok(Vec::new());
    }
    let mut moved = Vec::new();
    let mut created: HashSet<String> = HashSet::new();
    let root_entries: Vec<fs::DirEntry> = fs::read_dir(folder)?.collect::<io::Result<_>>()?;
    for entry in &root_entries {
        if !keep_going() {
            return Err(AnkiError::Interrupted);
        }
        if moved.len() >= max_files {
            break;
        }
        let path = entry.path();
        if path.is_dir() {
            continue;
        }
        let Some(name) = path.file_name().and_then(|name| name.to_str()) else {
            continue;
        };
        if NONSYNCABLE_FILENAME.is_match(name) {
            continue;
        }
        let shard = shard_dir_name(name);
        let dest = folder.join(&shard).join(name);
        if dest == path {
            continue;
        }
        if !keep_going() {
            return Err(AnkiError::Interrupted);
        }
        if dest.exists() {
            if files_have_same_contents(&path, &dest)? {
                fs::remove_file(&path)?;
                moved.push(name.to_owned());
            }
            continue;
        }
        if created.insert(shard.clone()) {
            create_dir_all(folder.join(&shard))?;
        }
        match fs::rename(&path, &dest) {
            Ok(()) => moved.push(name.to_owned()),
            Err(e) if e.kind() == io::ErrorKind::NotFound => continue,
            Err(e) => return Err(e.into()),
        }
    }
    Ok(moved)
}

/// True if character may cause problems on one or more platforms.
fn disallowed_char(char: char) -> bool {
    match char {
        '[' | ']' | '<' | '>' | ':' | '"' | '/' | '?' | '*' | '^' | '\\' | '|' => true,
        c if c.is_ascii_control() => true,
        // Macs do not allow invalid Unicode characters like 05F8 to be in a filename.
        c if GeneralCategory::of(c) == GeneralCategory::Unassigned => true,
        _ => false,
    }
}

fn nonbreaking_space(char: char) -> bool {
    char == '\u{a0}'
}

/// Adjust filename into the format Anki expects.
///
/// - The filename is normalized to NFC.
/// - Any problem characters are removed.
/// - Windows device names like CON and PRN have '_' appended
/// - The filename is limited to 120 bytes.
pub fn normalize_filename(fname: &str) -> Cow<'_, str> {
    let mut output = Cow::Borrowed(fname);

    if !is_nfc(output.as_ref()) {
        output = output.chars().nfc().collect::<String>().into();
    }

    normalize_nfc_filename(output)
}

/// See normalize_filename(). This function expects NFC-normalized input.
pub(crate) fn normalize_nfc_filename(mut fname: Cow<'_, str>) -> Cow<'_, str> {
    if fname.contains(disallowed_char) {
        fname = fname.replace(disallowed_char, "").into()
    }

    // convert nonbreaking spaces to regular ones, as the filename extraction
    // code treats nonbreaking spaces as regular ones
    if fname.contains(nonbreaking_space) {
        fname = fname.replace(nonbreaking_space, " ").into()
    }

    if let Cow::Owned(o) = WINDOWS_DEVICE_NAME.replace_all(fname.as_ref(), "${1}_${2}") {
        fname = o.into();
    }

    if WINDOWS_TRAILING_CHAR.is_match(fname.as_ref()) {
        fname = format!("{}_", fname.as_ref()).into();
    }

    if let Cow::Owned(o) = truncate_filename(fname.as_ref(), MAX_MEDIA_FILENAME_LENGTH) {
        fname = o.into();
    }

    fname
}

/// Return the filename in NFC form if the filename is valid.
///
/// Returns None if the filename is not normalized
/// (NFD, invalid chars, etc)
///
/// On Apple devices, the filename may be stored on disk in NFD encoding,
/// but can be accessed as NFC. On these devices, if the filename
/// is otherwise valid, the filename is returned as NFC.
#[allow(clippy::collapsible_else_if)]
pub(crate) fn filename_if_normalized(fname: &str) -> Option<Cow<'_, str>> {
    if cfg!(target_vendor = "apple") {
        if !is_nfc(fname) {
            let as_nfc = fname.chars().nfc().collect::<String>();
            if let Cow::Borrowed(_) = normalize_nfc_filename(as_nfc.as_str().into()) {
                Some(as_nfc.into())
            } else {
                None
            }
        } else {
            if let Cow::Borrowed(_) = normalize_nfc_filename(fname.into()) {
                Some(fname.into())
            } else {
                None
            }
        }
    } else {
        if let Cow::Borrowed(_) = normalize_filename(fname) {
            Some(fname.into())
        } else {
            None
        }
    }
}

/// Write desired_name into folder, renaming if existing file has different
/// content. Returns the used filename.
pub fn add_data_to_folder_uniquely<'a, P>(
    folder: P,
    desired_name: &'a str,
    data: &[u8],
    sha1: Sha1Hash,
) -> Result<Cow<'a, str>, FileIoError>
where
    P: AsRef<Path>,
{
    let normalized_name = normalize_filename(desired_name);

    if let Some(existing) = locate_existing(folder.as_ref(), normalized_name.as_ref()) {
        let existing_file_hash = existing_file_sha1(&existing)?;
        if matches!(existing_file_hash, Some(hash) if hash == sha1) {
            #[cfg(target_os = "horizon")]
            let canonical = anki_io::canonical_sd_file(&existing);
            #[cfg(not(target_os = "horizon"))]
            let canonical = fs::canonicalize(&existing);
            if let Some(normalised_existing_name) = canonical.ok().and_then(|p| {
                p.file_name()?
                    .to_str()
                    .and_then(filename_if_normalized)
                    .map(Cow::into_owned)
            }) {
                // existing file has same checksum and normalised name, nothing
                // to do
                return Ok(normalised_existing_name.into());
            }
        }
    }

    let lowercased_name = normalized_name.to_lowercase();
    if lowercased_name != normalized_name {
        // try again with the lowercased name (to_lowercase is idempotent)
        return Ok(
            add_data_to_folder_uniquely(folder, &lowercased_name, data, sha1)?
                .to_string()
                .into(),
        );
    }

    if locate_existing(folder.as_ref(), normalized_name.as_ref()).is_none() {
        // no file with that name that's also in lowercase exists yet
        let target_path = sharded_path(folder.as_ref(), normalized_name.as_ref());
        if let Some(parent) = target_path.parent() {
            ensure_dir(parent)?;
        }
        write_file(&target_path, data)?;
        remove_superseded_root_copy(folder.as_ref(), normalized_name.as_ref());
        return Ok(normalized_name);
    }

    // give it a unique name based on its hash
    let hashed_name = add_hash_suffix_to_file_stem(normalized_name.as_ref(), &sha1);
    let target_path = sharded_path(folder.as_ref(), &hashed_name);
    if let Some(parent) = target_path.parent() {
        ensure_dir(parent)?;
    }
    write_file(&target_path, data)?;
    remove_superseded_root_copy(folder.as_ref(), &hashed_name);
    Ok(hashed_name.into())
}

/// Convert foo.jpg into foo-abcde12345679.jpg
pub(crate) fn add_hash_suffix_to_file_stem(fname: &str, hash: &Sha1Hash) -> String {
    // when appending a hash to make unique, it will be 40 bytes plus the
    // hyphen.
    let max_len = MAX_MEDIA_FILENAME_LENGTH - 40 - 1;

    let (stem, ext) = split_and_truncate_filename(fname, max_len);

    format!("{}-{}.{}", stem, hex::encode(hash), ext)
}

/// If filename is longer than max_bytes, truncate it.
fn truncate_filename(fname: &str, max_bytes: usize) -> Cow<'_, str> {
    if fname.len() <= max_bytes {
        return Cow::Borrowed(fname);
    }

    let (stem, ext) = split_and_truncate_filename(fname, max_bytes);

    let mut new_name = if ext.is_empty() {
        stem.to_string()
    } else {
        format!("{stem}.{ext}")
    };

    // make sure we don't break Windows by ending with a space or dot
    if WINDOWS_TRAILING_CHAR.is_match(&new_name) {
        new_name.push('_');
    }

    new_name.into()
}

/// Split filename into stem and extension, and trim both so the
/// resulting filename would be under max_bytes.
/// Returns (stem, extension)
fn split_and_truncate_filename(fname: &str, max_bytes: usize) -> (&str, &str) {
    // the code assumes max_bytes will be at least 11
    debug_assert!(max_bytes > 10);

    let mut iter = fname.rsplitn(2, '.');
    let mut ext = iter.next().unwrap();
    let mut stem = if let Some(s) = iter.next() {
        s
    } else {
        // no extension, so ext holds the full filename
        let ext_tmp = ext;
        ext = "";
        ext_tmp
    };

    // cap extension to 10 bytes so stem_len can't be negative
    ext = truncated_to_char_boundary(ext, 10);

    // cap stem, allowing for the . and a trailing _
    let stem_len = max_bytes - ext.len() - 2;
    stem = truncated_to_char_boundary(stem, stem_len);

    (stem, ext)
}

/// Return a substring on a valid UTF8 boundary.
/// Based on a function in the Rust stdlib.
fn truncated_to_char_boundary(s: &str, mut max: usize) -> &str {
    if max >= s.len() {
        s
    } else {
        while !s.is_char_boundary(max) {
            max -= 1;
        }
        &s[..max]
    }
}

/// Return the SHA1 of a file if it exists, or None.
fn existing_file_sha1(path: &Path) -> Result<Option<Sha1Hash>, FileIoError> {
    match sha1_of_file(path) {
        Ok(o) => Ok(Some(o)),
        Err(e) if e.is_not_found() => Ok(None),
        Err(e) => Err(e),
    }
}

/// Return the SHA1 of a file, failing if it doesn't exist.
pub(crate) fn sha1_of_file(path: &Path) -> Result<Sha1Hash, FileIoError> {
    let mut file = open_file(path)?;
    sha1_of_reader(&mut file).context(FileIoSnafu {
        path,
        op: FileOp::Read,
    })
}

/// [sha1_of_file], but aborts with [AnkiError::Interrupted] as soon as
/// `keep_going` returns false.
pub(crate) fn sha1_of_file_checked(
    path: &Path,
    keep_going: &mut impl FnMut() -> bool,
) -> Result<Sha1Hash> {
    let mut file = anki_io::open_file(path)?;
    let mut hasher = Sha1::new();
    let mut buf = [0; 64 * 1024];
    loop {
        if !keep_going() {
            return Err(AnkiError::Interrupted);
        }
        match file.read(&mut buf) {
            Ok(0) => break,
            Ok(n) => hasher.update(&buf[0..n]),
            Err(e) if e.kind() == io::ErrorKind::Interrupted => continue,
            Err(e) => {
                return Err(e).context(FileIoSnafu {
                    path,
                    op: FileOp::Read,
                })?
            }
        }
    }
    Ok(hasher.finalize().into())
}

/// Return the SHA1 of a stream.
pub(crate) fn sha1_of_reader(reader: &mut impl Read) -> io::Result<Sha1Hash> {
    let mut hasher = Sha1::new();
    let mut buf = [0; 64 * 1024];
    loop {
        match reader.read(&mut buf) {
            Ok(0) => break,
            Ok(n) => hasher.update(&buf[0..n]),
            Err(e) if e.kind() == io::ErrorKind::Interrupted => continue,
            Err(e) => return Err(e),
        };
    }
    Ok(hasher.finalize().into())
}

/// Return the SHA1 of provided data.
pub(crate) fn sha1_of_data(data: &[u8]) -> Sha1Hash {
    let mut hasher = Sha1::new();
    hasher.update(data);
    hasher.finalize().into()
}

pub(crate) fn mtime_as_i64<P: AsRef<Path>>(path: P) -> io::Result<i64> {
    Ok(path
        .as_ref()
        .metadata()?
        .modified()?
        .duration_since(time::UNIX_EPOCH)
        .unwrap()
        .as_millis() as i64)
}

pub fn remove_files<S>(media_folder: &Path, files: &[S]) -> Result<()>
where
    S: AsRef<str> + std::fmt::Debug,
{
    if files.is_empty() {
        return Ok(());
    }

    let trash_folder = trash_folder(media_folder)?;

    for file in files {
        let src_path = match locate_existing(media_folder, file.as_ref()) {
            Some(path) => path,
            None => return Ok(()),
        };
        let dst_path = trash_folder.join(file.as_ref());

        // move file to trash, clobbering any existing file with the same name
        fs::rename(&src_path, &dst_path)?;

        // mark it as modified, so we can expire it in the future
        let secs = time::SystemTime::now();
        let times = FileTimes::new().set_accessed(secs).set_modified(secs);
        if let Err(err) = set_file_times(&dst_path, times) {
            // The libc utimes() call fails on (some? all?) Android devices.
            // Since we don't do automatic expiry yet, we can safely
            // ignore the error.
            if !cfg!(target_os = "android") {
                return Err(err.into());
            }
        }
    }

    Ok(())
}

pub(super) fn trash_folder(media_folder: &Path) -> Result<PathBuf> {
    let trash_folder = media_folder.with_file_name("media.trash");
    match create_dir(&trash_folder) {
        Ok(()) => Ok(trash_folder),
        Err(e) => {
            if e.source.kind() == io::ErrorKind::AlreadyExists {
                Ok(trash_folder)
            } else {
                Err(e.into())
            }
        }
    }
}

pub struct AddedFile {
    pub fname: String,
    pub sha1: Sha1Hash,
    pub mtime: i64,
    pub renamed_from: Option<String>,
}

/// Largest single media file accepted from a sync download. The zip header
/// size is untrusted, so the streaming writer below enforces this on the
/// bytes it actually writes.
pub(crate) const MAX_DOWNLOADED_FILE_BYTES: u64 = 32 * 1024 * 1024;

fn ensure_dir(path: &Path) -> std::result::Result<(), anki_io::FileIoError> {
    if path.as_os_str().is_empty() || path.is_dir() {
        return Ok(());
    }
    create_dir_all(path)
}

/// Add a file received from AnkiWeb into the media folder.
///
/// Because AnkiWeb did not previously enforce file name limits and invalid
/// characters, we'll need to rename the file if it is not valid.
pub(crate) fn add_file_from_ankiweb(
    media_folder: &Path,
    fname: &str,
    data: &[u8],
) -> Result<AddedFile> {
    let sha1 = sha1_of_data(data);
    let normalized = normalize_filename(fname);

    // if the filename is already valid, we can write the file directly
    let (renamed_from, path) = if let Cow::Borrowed(_) = normalized {
        let path = sharded_path(media_folder, normalized.as_ref());
        if let Some(parent) = path.parent() {
            ensure_dir(parent)?;
        }
        debug!(fname = normalized.as_ref(), "write");
        write_file(&path, data)?;
        remove_superseded_root_copy(media_folder, normalized.as_ref());
        (None, path)
    } else {
        // ankiweb sent us a non-normalized filename, so we'll rename it
        let new_name = add_data_to_folder_uniquely(media_folder, fname, data, sha1)?;
        debug!(
            fname,
            rename_to = new_name.as_ref(),
            "non-normalized filename received"
        );
        (
            Some(fname.to_string()),
            sharded_path(media_folder, new_name.as_ref()),
        )
    };

    let mtime = mtime_as_i64(path)?;

    Ok(AddedFile {
        fname: normalized.to_string(),
        sha1,
        mtime,
        renamed_from,
    })
}

struct HashingWriter<W: Write> {
    inner: W,
    hasher: Sha1,
}

impl<W: Write> Write for HashingWriter<W> {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        let written = self.inner.write(buf)?;
        self.hasher.update(&buf[..written]);
        Ok(written)
    }

    fn flush(&mut self) -> io::Result<()> {
        self.inner.flush()
    }
}

/// Like [add_file_from_ankiweb], but streams the file straight to its final
/// path while checksumming, so a large download is never held in memory.
pub(crate) fn add_file_from_ankiweb_streaming(
    media_folder: &Path,
    fname: &str,
    reader: &mut impl Read,
    cancelled: &mut impl FnMut() -> bool,
) -> Result<AddedFile> {
    let normalized = normalize_filename(fname);
    if let Cow::Borrowed(_) = normalized {
        let path = sharded_path(media_folder, normalized.as_ref());
        if let Some(parent) = path.parent() {
            ensure_dir(parent)?;
        }
        let (sha1, mtime) = write_file_hashing(&path, reader, cancelled)?;
        remove_superseded_root_copy(media_folder, normalized.as_ref());
        debug!(fname = normalized.as_ref(), "write");
        Ok(AddedFile {
            fname: normalized.to_string(),
            sha1,
            mtime,
            renamed_from: None,
        })
    } else {
        if cancelled() {
            return Err(AnkiError::Interrupted);
        }
        let mut data = Vec::new();
        // the .take() is because we don't trust the reader to be bounded
        reader
            .take(MAX_DOWNLOADED_FILE_BYTES + 1)
            .read_to_end(&mut data)?;
        if data.len() as u64 > MAX_DOWNLOADED_FILE_BYTES {
            invalid_input!("media file exceeds download limit");
        }
        add_file_from_ankiweb(media_folder, fname, &data)
    }
}

fn write_file_hashing(
    path: &Path,
    reader: &mut impl Read,
    cancelled: &mut impl FnMut() -> bool,
) -> Result<(Sha1Hash, i64)> {
    const STREAM_BUF_SIZE: usize = if cfg!(target_os = "horizon") {
        16 * 1024
    } else {
        64 * 1024
    };
    let file = fs::File::create(path)?;
    let mut writer = HashingWriter {
        inner: io::BufWriter::with_capacity(STREAM_BUF_SIZE, file),
        hasher: Sha1::new(),
    };
    let mut buf = [0u8; STREAM_BUF_SIZE];
    let mut interrupted = false;
    let mut total: u64 = 0;
    // the .take() is because we don't trust the zip header to be correct
    let mut limited = reader.by_ref().take(MAX_DOWNLOADED_FILE_BYTES + 1);
    loop {
        if cancelled() {
            interrupted = true;
            break;
        }
        match limited.read(&mut buf) {
            Ok(0) => break,
            Ok(n) => {
                total += n as u64;
                writer.write_all(&buf[..n])?
            }
            Err(e) if e.kind() == io::ErrorKind::Interrupted => continue,
            Err(e) => return Err(e.into()),
        }
    }
    let HashingWriter { mut inner, hasher } = writer;
    if interrupted {
        drop(inner);
        let _ = fs::remove_file(path);
        return Err(AnkiError::Interrupted);
    }
    if total > MAX_DOWNLOADED_FILE_BYTES {
        drop(inner);
        let _ = fs::remove_file(path);
        invalid_input!("media file exceeds download limit");
    }
    inner.flush()?;
    drop(inner);
    let sha1: Sha1Hash = hasher.finalize().into();
    let mtime = mtime_as_i64(path)?;
    Ok((sha1, mtime))
}

pub(crate) fn data_for_file(media_folder: &Path, fname: &str) -> Result<Option<Vec<u8>>> {
    let mut file = match open_media_file(media_folder, fname) {
        Err(e) if e.is_not_found() => return Ok(None),
        res => res?,
    };
    let mut buf = vec![];
    file.read_to_end(&mut buf)?;
    Ok(Some(buf))
}

#[cfg(test)]
mod test {
    use std::borrow::Cow;

    use tempfile::tempdir;

    use crate::media::files::add_data_to_folder_uniquely;
    use crate::media::files::add_hash_suffix_to_file_stem;
    use crate::media::files::normalize_filename;
    use crate::media::files::remove_files;
    use crate::media::files::sha1_of_data;
    use crate::media::files::truncate_filename;
    use crate::sync::media::MAX_MEDIA_FILENAME_LENGTH;

    #[test]
    fn normalize() {
        assert_eq!(normalize_filename("foo.jpg"), Cow::Borrowed("foo.jpg"));
        assert_eq!(
            normalize_filename("con.jpg[]><:\"/?*^\\|\0\r\n").as_ref(),
            "con_.jpg"
        );

        assert_eq!(normalize_filename("test.").as_ref(), "test._");
        assert_eq!(normalize_filename("test ").as_ref(), "test _");

        let expected_stem_len = MAX_MEDIA_FILENAME_LENGTH - ".jpg".len() - 1;
        assert_eq!(
            normalize_filename(&format!(
                "{}.jpg",
                "x".repeat(MAX_MEDIA_FILENAME_LENGTH * 2)
            )),
            "x".repeat(expected_stem_len) + ".jpg"
        );
    }

    #[test]
    fn add_hash_suffix() {
        let hash = sha1_of_data(b"hello");
        assert_eq!(
            add_hash_suffix_to_file_stem("test.jpg", &hash).as_str(),
            "test-aaf4c61ddcc5e8a2dabede0f3b482cd9aea9434d.jpg"
        );
    }

    #[test]
    fn adding_removing() {
        let dir = tempdir().unwrap();
        let dpath = dir.path();

        // no existing file case
        let h1 = sha1_of_data(b"hello");
        assert_eq!(
            add_data_to_folder_uniquely(dpath, "test.mp3", b"hello", h1).unwrap(),
            "test.mp3"
        );

        // same contents case
        assert_eq!(
            add_data_to_folder_uniquely(dpath, "test.mp3", b"hello", h1).unwrap(),
            "test.mp3"
        );

        // same contents, filenames differ only by case
        assert_eq!(
            add_data_to_folder_uniquely(dpath, "Test.mp3", b"hello", h1).unwrap(),
            "test.mp3"
        );

        // different contents, filenames differ only by case
        let h2 = sha1_of_data(b"hello1");
        assert_eq!(
            add_data_to_folder_uniquely(dpath, "Test.mp3", b"hello1", h2).unwrap(),
            "test-88fdd585121a4ccb3d1540527aee53a77c77abb8.mp3"
        );

        // same contents, filenames differ only by case
        assert_eq!(
            add_data_to_folder_uniquely(dpath, "test.mp3", b"hello1", h2).unwrap(),
            "test-88fdd585121a4ccb3d1540527aee53a77c77abb8.mp3"
        );

        // filename normalisation
        assert_eq!(normalize_filename("cafe\u{301}.mp3"), "caf\u{e9}.mp3");

        let h3 = sha1_of_data(b"hello3");
        assert_eq!(
            add_data_to_folder_uniquely(dpath, "caf\u{e9}.mp3", b"hello3", h3).unwrap(),
            "caf\u{e9}.mp3"
        );

        assert_eq!(
            add_data_to_folder_uniquely(dpath, "cafe\u{301}.mp3", b"hello3", h3).unwrap(),
            "caf\u{e9}.mp3"
        );

        assert_eq!(
            add_data_to_folder_uniquely(dpath, "Cafe\u{301}.mp3", b"hello3", h3).unwrap(),
            "caf\u{e9}.mp3"
        );

        let h4 = sha1_of_data(b"hello4");
        assert_eq!(
            add_data_to_folder_uniquely(dpath, "Cafe\u{301}.mp3", b"hello4", h4).unwrap(),
            "caf\u{e9}-8f0bc65da355c6cb184de9d17bfe1baaeefbd443.mp3"
        );

        let mut written_files = crate::media::files::media_file_entries(dpath)
            .unwrap()
            .into_iter()
            .map(|(fname, _)| fname)
            .collect::<Vec<_>>();
        written_files.sort();
        assert_eq!(
            written_files,
            vec![
                "caf\u{e9}-8f0bc65da355c6cb184de9d17bfe1baaeefbd443.mp3",
                "caf\u{e9}.mp3",
                "test-88fdd585121a4ccb3d1540527aee53a77c77abb8.mp3",
                "test.mp3",
            ]
        );

        // remove
        remove_files(dpath, written_files.as_slice()).unwrap();
    }

    #[test]
    fn sharding_roundtrip() {
        let dir = tempdir().unwrap();
        let folder = dir.path().join("media");
        std::fs::create_dir(&folder).unwrap();

        let name = "some-audio-file.mp3";
        let shard = crate::media::files::shard_dir_name(name);
        assert_eq!(shard.len(), 2);
        assert!(shard.bytes().all(|b| b.is_ascii_hexdigit()));

        let path = crate::media::files::sharded_path(&folder, name);
        if crate::media::files::sharding_enabled() {
            assert_eq!(path.parent().unwrap().file_name().unwrap(), shard.as_str());
        } else {
            assert_eq!(path, folder.join(name));
        }

        assert!(crate::media::files::locate_existing(&folder, name).is_none());

        std::fs::write(folder.join(name), "root copy").unwrap();
        assert_eq!(
            crate::media::files::locate_existing(&folder, name).unwrap(),
            folder.join(name)
        );

        let mut keep_going = || true;
        let moved =
            crate::media::files::migrate_media_to_shards(&folder, &mut keep_going, usize::MAX)
                .unwrap();
        if crate::media::files::sharding_enabled() {
            assert_eq!(moved, vec![name.to_string()]);
        } else {
            assert!(moved.is_empty());
        }
        assert_eq!(
            crate::media::files::locate_existing(&folder, name).unwrap(),
            path
        );
        assert_eq!(std::fs::read(&path).unwrap(), b"root copy");

        let entries = crate::media::files::media_file_entries(&folder).unwrap();
        assert_eq!(entries, vec![(name.to_string(), path.clone())]);
        assert_eq!(crate::media::files::media_file_count(&folder).unwrap(), 1);
        let expected_mtime = if crate::media::files::sharding_enabled() {
            crate::media::files::mtime_as_i64(folder.join(shard)).unwrap()
        } else {
            crate::media::files::mtime_as_i64(&folder).unwrap()
        };
        assert_eq!(
            crate::media::files::combined_folder_mtime(&folder).unwrap(),
            expected_mtime.max(crate::media::files::mtime_as_i64(&folder).unwrap())
        );
    }

    #[test]
    fn truncation() {
        let one_less = "x".repeat(MAX_MEDIA_FILENAME_LENGTH - 1);
        assert_eq!(
            truncate_filename(&one_less, MAX_MEDIA_FILENAME_LENGTH),
            Cow::Borrowed(&one_less)
        );
        let equal = "x".repeat(MAX_MEDIA_FILENAME_LENGTH);
        assert_eq!(
            truncate_filename(&equal, MAX_MEDIA_FILENAME_LENGTH),
            Cow::Borrowed(&equal)
        );
        let equal = format!("{}.jpg", "x".repeat(MAX_MEDIA_FILENAME_LENGTH - 4));
        assert_eq!(
            truncate_filename(&equal, MAX_MEDIA_FILENAME_LENGTH),
            Cow::Borrowed(&equal)
        );
        let one_more = "x".repeat(MAX_MEDIA_FILENAME_LENGTH + 1);
        assert_eq!(
            truncate_filename(&one_more, MAX_MEDIA_FILENAME_LENGTH),
            Cow::<str>::Owned("x".repeat(MAX_MEDIA_FILENAME_LENGTH - 2))
        );
        assert_eq!(
            truncate_filename(
                &" ".repeat(MAX_MEDIA_FILENAME_LENGTH + 1),
                MAX_MEDIA_FILENAME_LENGTH
            ),
            Cow::<str>::Owned(format!("{}_", " ".repeat(MAX_MEDIA_FILENAME_LENGTH - 2)))
        );
    }

    #[test]
    fn migration_moves_bounded_batches() {
        let dir = tempdir().unwrap();
        let folder = dir.path().join("media");
        std::fs::create_dir(&folder).unwrap();
        for name in ["a.mp3", "b.mp3", "c.mp3"] {
            std::fs::write(folder.join(name), name).unwrap();
        }
        let mut keep_going = || true;
        let moved =
            crate::media::files::migrate_media_to_shards(&folder, &mut keep_going, 2).unwrap();
        if !crate::media::files::sharding_enabled() {
            assert!(moved.is_empty());
            return;
        }
        assert_eq!(moved.len(), 2);
        let mut keep_going = || true;
        let moved =
            crate::media::files::migrate_media_to_shards(&folder, &mut keep_going, 2).unwrap();
        assert_eq!(moved.len(), 1);
        assert_eq!(crate::media::files::media_file_count(&folder).unwrap(), 3);
        let mut keep_going = || false;
        let result =
            crate::media::files::migrate_media_to_shards(&folder, &mut keep_going, usize::MAX);
        assert!(matches!(result, Err(crate::error::AnkiError::Interrupted)));
    }

    #[test]
    fn media_name_index_cache_roundtrip() {
        let dir = tempdir().unwrap();
        let folder = dir.path().join("media");
        std::fs::create_dir(&folder).unwrap();
        let index = crate::media::files::MediaNameIndex::new();
        assert!(crate::media::files::load_media_name_index(&folder).is_none());
        for name in ["x.mp3", "y.png"] {
            std::fs::write(folder.join(name), name).unwrap();
            index.insert(name, true);
        }
        index.insert("z.mp3", false);
        crate::media::files::save_media_name_index(&index, &folder).unwrap();
        let loaded = crate::media::files::load_media_name_index(&folder).unwrap();
        assert!(loaded.contains("x.mp3"));
        assert!(loaded.is_unmigrated("y.png"));
        assert!(!loaded.is_unmigrated("z.mp3"));
        assert!(!loaded.contains("missing.mp3"));
        assert_eq!(loaded.len(), 3);
    }
}
