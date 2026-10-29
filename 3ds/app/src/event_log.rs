// Copyright: Anki 3DS contributors
// License: GNU AGPL, version 3 or later; http://www.gnu.org/licenses/agpl.html

use std::fs;
use std::fs::OpenOptions;
use std::io;
use std::io::Write;
use std::sync::atomic::AtomicU32;
use std::sync::atomic::Ordering;

pub const PATH: &str = "sdmc:/3ds/anki-3ds/prototype-events.txt";
const PREVIOUS: &str = "sdmc:/3ds/anki-3ds/prototype-events.prev.txt";
const MAX_BYTES: u64 = 128 * 1024;
const TRUNCATED: &str = "-- log truncated --";

static BYTES: AtomicU32 = AtomicU32::new(0);

pub fn begin_session() {
    let current = std::path::Path::new(PATH);
    let length = fs::metadata(current).map(|meta| meta.len()).unwrap_or(0);
    if length > 0 {
        let _ = fs::remove_file(PREVIOUS);
        if fs::rename(current, PREVIOUS).is_err() {
            let _ = fs::copy(current, PREVIOUS);
            let _ = fs::remove_file(current);
        }
    }
    let _ = fs::File::create(current);
    BYTES.store(0, Ordering::Relaxed);
}

pub fn append(text: &str) {
    let Ok(mut file) = OpenOptions::new()
        .create(true)
        .append(true)
        .open(PATH)
    else {
        return;
    };
    let mut line = text.to_owned();
    line.push('\n');
    truncate_if_full(&mut file, line.len() as u64);
    if file.write_all(line.as_bytes()).is_ok() {
        let _ = file.sync_all();
    }
}

pub fn trace_writer() -> Box<dyn Write + Send> {
    match OpenOptions::new().create(true).append(true).open(PATH) {
        Ok(file) => Box::new(TraceWriter {
            file,
            pending: 0,
        }),
        Err(_) => Box::new(io::sink()),
    }
}

fn truncate_if_full(file: &mut fs::File, incoming: u64) {
    let written = u64::from(BYTES.load(Ordering::Relaxed));
    if written + incoming <= MAX_BYTES {
        return;
    }
    if file.set_len(0).is_ok() {
        BYTES.store(0, Ordering::Relaxed);
        let marker = format!("{TRUNCATED}\n");
        if file.write_all(marker.as_bytes()).is_ok() {
            BYTES.store(marker.len() as u32, Ordering::Relaxed);
        }
    }
}

struct TraceWriter {
    file: fs::File,
    pending: u64,
}

impl Write for TraceWriter {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        truncate_if_full(&mut self.file, buf.len() as u64);
        let count = self.file.write(buf)?;
        BYTES.fetch_add(count as u32, Ordering::Relaxed);
        self.pending += count as u64;
        if self.pending >= 4096 {
            let _ = self.file.flush();
            self.pending = 0;
        }
        Ok(count)
    }

    fn flush(&mut self) -> io::Result<()> {
        self.pending = 0;
        self.file.flush()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn truncation_keeps_the_file_bounded() {
        let root = std::env::var("ANKI_3DS_EVENT_TEST_ROOT").expect("isolated test path");
        let folder = std::path::Path::new(&root).join(format!("events-{}", std::process::id()));
        fs::create_dir_all(&folder).unwrap();
        let path = folder.join("events.txt");
        let mut file = OpenOptions::new()
            .create(true)
            .append(true)
            .open(&path)
            .unwrap();
        BYTES.store(MAX_BYTES as u32 - 1, Ordering::Relaxed);
        truncate_if_full(&mut file, 16);
        file.write_all(b"next line\n").unwrap();
        assert!(fs::metadata(&path).unwrap().len() < MAX_BYTES);
        assert!(fs::read_to_string(&path).unwrap().contains(TRUNCATED));
        let _ = fs::remove_dir_all(folder);
    }
}
