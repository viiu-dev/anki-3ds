// Copyright: Anki 3DS contributors
// License: GNU AGPL, version 3 or later; http://www.gnu.org/licenses/agpl.html

use std::fs;
use std::fs::File;
use std::io;
use std::io::Read;
use std::io::Write;
use std::path::Path;
use std::path::PathBuf;

pub const DIR: &str = "sdmc:/3ds/anki-3ds/backups";
pub const MAX_BACKUPS: usize = 8;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Entry {
    pub filename: String,
    pub path: PathBuf,
    pub label: String,
    pub size: u64,
    pub timestamp: i64,
}

impl Entry {
    pub fn row_label(&self) -> String {
        format!("{} ({})", self.label, format_size(self.size))
    }
}

fn format_size(size: u64) -> String {
    if size < 1024 * 1024 {
        format!("{} KB", ((size + 512) / 1024).max(1))
    } else {
        format!("{:.1} MB", size as f64 / (1024.0 * 1024.0))
    }
}

fn civil_from_days(days: i64) -> (i64, u32, u32) {
    let z = days + 719_468;
    let era = if z >= 0 { z } else { z - 146_096 } / 146_097;
    let doe = (z - era * 146_097) as u64;
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146_096) / 365;
    let y = yoe as i64 + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    (y + i64::from(m <= 2), m as u32, d as u32)
}

fn days_from_civil(year: i64, month: u32, day: u32) -> i64 {
    let y = year - i64::from(month <= 2);
    let era = if y >= 0 { y } else { y - 399 } / 400;
    let yoe = (y - era * 400) as u64;
    let m = u64::from(month);
    let d = u64::from(day);
    let doy = (153 * if m > 2 { m - 3 } else { m + 9 } + 2) / 5 + d - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    era * 146_097 + doe as i64 - 719_468
}

fn format_timestamp(secs: i64) -> (String, String) {
    let days = secs.div_euclid(86_400);
    let sod = secs.rem_euclid(86_400);
    let hour = sod / 3600;
    let min = (sod % 3600) / 60;
    let sec = sod % 60;
    let (year, month, day) = civil_from_days(days);
    (
        format!("backup-{year:04}-{month:02}-{day:02}-{hour:02}-{min:02}-{sec:02}.anki2"),
        format!("{year:04}-{month:02}-{day:02} {hour:02}:{min:02}:{sec:02}"),
    )
}

fn parse_filename(name: &str) -> Option<(String, i64)> {
    let stem = name.strip_prefix("backup-")?.strip_suffix(".anki2")?;
    let bytes = stem.as_bytes();
    if bytes.len() != 19
        || bytes[4] != b'-'
        || bytes[7] != b'-'
        || bytes[10] != b'-'
        || bytes[13] != b'-'
        || bytes[16] != b'-'
    {
        return None;
    }
    let year: i64 = stem[0..4].parse().ok()?;
    let month: u32 = stem[5..7].parse().ok()?;
    let day: u32 = stem[8..10].parse().ok()?;
    let hour: u32 = stem[11..13].parse().ok()?;
    let min: u32 = stem[14..16].parse().ok()?;
    let sec: u32 = stem[17..19].parse().ok()?;
    if !(1..=12).contains(&month) || !(1..=31).contains(&day) || hour >= 24 || min >= 60 || sec >= 60 {
        return None;
    }
    let timestamp = days_from_civil(year, month, day) * 86_400
        + i64::from(hour) * 3600
        + i64::from(min) * 60
        + i64::from(sec);
    Some((
        format!("{year:04}-{month:02}-{day:02} {hour:02}:{min:02}:{sec:02}"),
        timestamp,
    ))
}

pub fn list_in(dir: &Path) -> Vec<Entry> {
    let Ok(read) = fs::read_dir(dir) else {
        return Vec::new();
    };
    let mut entries = Vec::new();
    for item in read.flatten() {
        let filename = item.file_name().to_string_lossy().into_owned();
        let Some((label, timestamp)) = parse_filename(&filename) else {
            continue;
        };
        let Ok(meta) = item.metadata() else {
            continue;
        };
        if !meta.is_file() || meta.len() == 0 {
            continue;
        }
        entries.push(Entry {
            filename,
            path: item.path(),
            label,
            size: meta.len(),
            timestamp,
        });
    }
    entries.sort_by(|a, b| b.filename.cmp(&a.filename));
    entries
}

pub fn list() -> Vec<Entry> {
    list_in(Path::new(DIR))
}

pub fn prune_in(dir: &Path) {
    let entries = list_in(dir);
    for entry in entries.into_iter().skip(MAX_BACKUPS) {
        let _ = fs::remove_file(&entry.path);
    }
}

pub fn create_in(source: &Path, dir: &Path, now_secs: i64) -> io::Result<Entry> {
    fs::create_dir_all(dir)?;
    let tmp = dir.join(".backup-tmp.anki2");
    let _ = fs::remove_file(&tmp);
    let existing = list_in(dir);
    let secs = existing
        .first()
        .map_or(now_secs, |latest| now_secs.max(latest.timestamp + 1));
    let (filename, label) = format_timestamp(secs);
    let dest = dir.join(&filename);
    {
        let mut input = File::open(source)?;
        let mut output = File::create(&tmp)?;
        io::copy(&mut input, &mut output)?;
        output.sync_all()?;
    }
    if let Err(error) = fs::rename(&tmp, &dest) {
        let _ = fs::remove_file(&tmp);
        return Err(error);
    }
    let size = fs::metadata(&dest)?.len();
    prune_in(dir);
    Ok(Entry {
        filename,
        path: dest,
        label,
        size,
        timestamp: secs,
    })
}

pub fn create_from_backend(
    backend: &mut super::backend::Backend,
    modified_ms: i64,
    startup_day: Option<i64>,
) -> super::backend::Result<Entry> {
    backend.checkpoint()?;
    let entry = create_in(
        Path::new(super::backend::PATH),
        Path::new(DIR),
        super::clock::local_time(),
    )?;
    if modified_ms > 0 {
        let _ = super::i18n::save_backup_stamp(modified_ms);
    }
    if let Some(day) = startup_day {
        let _ = super::i18n::save_backup_day(day);
    }
    Ok(entry)
}

pub fn restore_file(backup_path: &Path, target: &Path) -> io::Result<()> {
    let mut input = File::open(backup_path)?;
    let mut header = [0u8; 16];
    if input.read_exact(&mut header).is_err() || &header != b"SQLite format 3\0" {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "invalid backup file",
        ));
    }
    let parent = target.parent().unwrap_or_else(|| Path::new("."));
    let mut temp = anki_3ds_sync_transport::storage::tempfile(parent, ".restore-")?;
    temp.write_all(&header)?;
    io::copy(&mut input, &mut temp)?;
    temp.as_file().sync_all()?;
    drop(input);
    let path = temp.into_temp_path();
    let wal = PathBuf::from(format!("{}-wal", target.display()));
    let shm = PathBuf::from(format!("{}-shm", target.display()));
    let _ = fs::remove_file(wal);
    let _ = fs::remove_file(shm);
    if target.exists() {
        anki_3ds_sync_transport::storage::replace(&path, target)?;
    } else {
        fs::rename(&path, target)?;
    }
    Ok(())
}
