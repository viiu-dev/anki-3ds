// Copyright: Anki 3DS contributors
// License: GNU AGPL, version 3 or later; http://www.gnu.org/licenses/agpl.html

use std::sync::atomic::AtomicBool;
use std::sync::atomic::Ordering;
use std::sync::mpsc;
use std::sync::Arc;
use std::time::Duration;
use std::time::Instant;

#[cfg(target_os = "horizon")]
use anki_3ds_sync_transport::soc::NativeSoc;
#[cfg(target_os = "horizon")]
use anki_3ds_sync_transport::NativeSyncClient;
#[cfg(target_os = "horizon")]
use ctru::services::ps::Ps;
use serde::Deserialize;

pub const CURRENT_VERSION: &str = env!("CARGO_PKG_VERSION");

#[cfg(target_os = "horizon")]
const GITHUB_HOST: &str = "api.github.com";
#[cfg(target_os = "horizon")]
const GITHUB_PATH: &str = "/repos/viiu-dev/anki-3ds/releases/latest";
#[cfg(target_os = "horizon")]
const GITHUB_ACCEPT: &str = "application/vnd.github+json";
#[cfg(target_os = "horizon")]
const MAX_RELEASE_JSON_BYTES: usize = 256 * 1024;

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Outcome {
    Available(String),
    UpToDate,
}

#[derive(Deserialize)]
struct ReleasePayload {
    tag_name: String,
    #[serde(default)]
    draft: bool,
    #[serde(default)]
    prerelease: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct ParsedVersion {
    major: u32,
    minor: u32,
    patch: u32,
    is_prerelease: bool,
}

fn parse_version(raw: &str) -> Option<ParsedVersion> {
    let trimmed = raw.trim();
    let stripped = trimmed
        .strip_prefix('v')
        .or_else(|| trimmed.strip_prefix('V'))
        .unwrap_or(trimmed);
    let no_build = stripped.split('+').next().unwrap_or(stripped);
    let (core, pre) = match no_build.split_once('-') {
        Some((core, pre)) => (core, Some(pre)),
        None => (no_build, None),
    };
    let mut parts = core.split('.');
    let major = parts.next()?.parse::<u32>().ok()?;
    let minor = parts.next().unwrap_or("0").parse::<u32>().ok()?;
    let patch = parts.next().unwrap_or("0").parse::<u32>().ok()?;
    if parts.next().is_some() {
        return None;
    }
    Some(ParsedVersion {
        major,
        minor,
        patch,
        is_prerelease: pre.is_some_and(|p| !p.is_empty()),
    })
}

pub fn is_newer_version(latest_tag: &str, current: &str) -> bool {
    let Some(latest) = parse_version(latest_tag) else {
        return false;
    };
    let Some(curr) = parse_version(current) else {
        return false;
    };
    match (latest.major, latest.minor, latest.patch).cmp(&(curr.major, curr.minor, curr.patch)) {
        std::cmp::Ordering::Greater => true,
        std::cmp::Ordering::Less => false,
        std::cmp::Ordering::Equal => !latest.is_prerelease && curr.is_prerelease,
    }
}

pub fn parse_release_response(status: u16, body: &[u8], current: &str) -> Result<Outcome, String> {
    if status == 404 {
        return Ok(Outcome::UpToDate);
    }
    if status != 200 {
        return Err(format!("HTTP {status}"));
    }
    let payload: ReleasePayload =
        serde_json::from_slice(body).map_err(|_| "Invalid release response".to_owned())?;
    if payload.draft || payload.prerelease {
        return Ok(Outcome::UpToDate);
    }
    let tag = payload.tag_name.trim();
    if is_newer_version(tag, current) {
        let display = if tag.starts_with('v') || tag.starts_with('V') {
            format!("v{}", &tag[1..])
        } else {
            format!("v{tag}")
        };
        Ok(Outcome::Available(display))
    } else {
        Ok(Outcome::UpToDate)
    }
}

pub struct Job {
    receiver: mpsc::Receiver<Result<Outcome, String>>,
    worker: Option<std::thread::JoinHandle<()>>,
    cancel: Arc<AtomicBool>,
}

impl Job {
    #[cfg(target_os = "horizon")]
    pub fn start() -> Result<Self, String> {
        let mut client = NativeSyncClient::default();
        client.timeout = Duration::from_secs(10);
        client.connect_timeout = Duration::from_secs(8);
        let cancel = client.cancel.clone();
        let (sender, receiver) = mpsc::channel();
        let worker = std::thread::Builder::new()
            .name("update-check".into())
            .stack_size(256 * 1024)
            .spawn(move || {
                let result = (|| {
                    let ps = Ps::new().map_err(|e| e.to_string())?;
                    let mut soc = NativeSoc::new().map_err(|e| e.to_string())?;
                    let response = client
                        .https_get(
                            GITHUB_HOST,
                            GITHUB_PATH,
                            GITHUB_ACCEPT,
                            MAX_RELEASE_JSON_BYTES,
                        )
                        .map_err(|e| e.to_string());
                    anki_3ds_sync_transport::cleanup_https_session();
                    let cleanup = soc.close().map_err(|e| e.to_string());
                    drop(ps);
                    cleanup?;
                    let resp = response?;
                    parse_release_response(resp.status, &resp.body, CURRENT_VERSION)
                })();
                let _ = sender.send(result);
            })
            .map_err(|e| e.to_string())?;
        Ok(Self {
            receiver,
            worker: Some(worker),
            cancel,
        })
    }

    pub fn poll(&mut self) -> Option<Result<Outcome, String>> {
        let result = match self.receiver.try_recv() {
            Ok(result) => result,
            Err(mpsc::TryRecvError::Empty) => return None,
            Err(mpsc::TryRecvError::Disconnected) => Err("Update check terminated".into()),
        };
        if let Some(worker) = self.worker.take() {
            if worker.join().is_err() {
                return Some(Err("Update check terminated".into()));
            }
        }
        Some(result)
    }

    pub fn cancel(&self) {
        self.cancel.store(true, Ordering::Release);
    }
}

impl Drop for Job {
    fn drop(&mut self) {
        self.cancel.store(true, Ordering::Release);
        let Some(worker) = self.worker.take() else {
            return;
        };
        let deadline = Instant::now() + Duration::from_secs(2);
        loop {
            match self.receiver.try_recv() {
                Ok(_) | Err(mpsc::TryRecvError::Disconnected) => {
                    let _ = worker.join();
                    return;
                }
                Err(mpsc::TryRecvError::Empty) => {
                    if Instant::now() >= deadline {
                        let _ = self.receiver.recv();
                        let _ = worker.join();
                        return;
                    }
                    std::thread::sleep(Duration::from_millis(10));
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn semver_comparison_handles_prefixes_and_prereleases() {
        assert!(is_newer_version("v1.0.1", "1.0.0"));
        assert!(is_newer_version("v1.1.0", "1.0.9"));
        assert!(is_newer_version("2.0.0", "1.9.9"));
        assert!(is_newer_version("v1.0.0", "1.0.0-rc.1"));
        assert!(!is_newer_version("v1.0.0", "1.0.0"));
        assert!(!is_newer_version("v0.9.9", "1.0.0"));
        assert!(!is_newer_version("v1.0.0-beta.1", "1.0.0"));
        assert!(!is_newer_version("invalid", "1.0.0"));
    }

    #[test]
    fn release_json_parsing_handles_newer_current_draft_and_404() {
        let newer = br#"{"tag_name":"v1.2.0","draft":false,"prerelease":false}"#;
        assert_eq!(
            parse_release_response(200, newer, "1.0.0"),
            Ok(Outcome::Available("v1.2.0".into()))
        );

        let same = br#"{"tag_name":"v1.0.0","draft":false,"prerelease":false}"#;
        assert_eq!(
            parse_release_response(200, same, "1.0.0"),
            Ok(Outcome::UpToDate)
        );

        let pre = br#"{"tag_name":"v2.0.0-beta.1","draft":false,"prerelease":true}"#;
        assert_eq!(
            parse_release_response(200, pre, "1.0.0"),
            Ok(Outcome::UpToDate)
        );

        assert_eq!(
            parse_release_response(404, b"{}", "1.0.0"),
            Ok(Outcome::UpToDate)
        );
        assert!(parse_release_response(500, b"{}", "1.0.0").is_err());
    }
}
