// Copyright: Ankitects Pty Ltd and contributors
// License: GNU AGPL, version 3 or later; http://www.gnu.org/licenses/agpl.html

use std::collections::HashMap;
use std::io;
use std::path::Path;

use serde::Deserialize;
use serde::Serialize;
use tracing::warn;

use crate::error;
use crate::media::files::add_file_from_ankiweb_streaming;
use crate::media::files::AddedFile;

const MAX_DOWNLOADED_FILE_BYTES: u64 = 32 * 1024 * 1024;

#[derive(Debug, Serialize, Deserialize)]
pub struct DownloadFilesRequest {
    pub files: Vec<String>,
}

pub(crate) struct ExtractedDownload {
    pub added: Vec<AddedFile>,
    pub cancelled: bool,
    pub bytes: u64,
}

pub(crate) fn extract_into_media_folder(
    media_folder: &Path,
    zip: Vec<u8>,
    mut cancelled: impl FnMut() -> bool,
) -> error::Result<ExtractedDownload> {
    let reader = io::Cursor::new(zip);
    let mut zip = zip::ZipArchive::new(reader)?;

    let meta_file = zip.by_name("_meta")?;
    let fmap: HashMap<String, String> = serde_json::from_reader(meta_file)?;
    let mut output = Vec::with_capacity(fmap.len());
    let mut bytes = 0u64;

    for i in 0..zip.len() {
        if cancelled() {
            return Ok(ExtractedDownload {
                added: output,
                cancelled: true,
                bytes,
            });
        }
        let mut file = zip.by_index(i)?;
        let name = file.name()?;
        if name == "_meta" {
            continue;
        }

        let Some(real_name) = fmap.get(name.as_ref()) else {
            warn!(
                name = name.as_ref(),
                "skipping media file missing from zip metadata"
            );
            continue;
        };
        let real_name = real_name.as_str();

        if file.size() > MAX_DOWNLOADED_FILE_BYTES {
            warn!(
                name = real_name,
                bytes = file.size(),
                "skipping media file that is too large to store safely"
            );
            continue;
        }

        let size = file.size();
        let added = add_file_from_ankiweb_streaming(media_folder, real_name, &mut file)?;
        bytes += size;

        output.push(added);
    }

    Ok(ExtractedDownload {
        added: output,
        cancelled: false,
        bytes,
    })
}
