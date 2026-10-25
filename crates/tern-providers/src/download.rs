//! Large file downloads for on-demand local models (`docs/local-models.md`).
//!
//! One file at a time, written to `<dest>.part` and renamed when complete.
//! A `.part` left by an earlier attempt is resumed with an HTTP `Range`
//! request; a server that ignores the range (200 instead of 206) starts the
//! file again. Progress is reported after every chunk, and a cancel flag is
//! checked between chunks, leaving the `.part` in place for the next try.
//! Hashing is the caller's job: this module only moves bytes.

use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

/// Why a download stopped.
#[derive(Debug, thiserror::Error)]
pub enum DownloadError {
    #[error("the download was cancelled")]
    Cancelled,
    #[error("network error: {0}")]
    Network(String),
    #[error("the server answered {0}")]
    Status(u16),
    #[error("the file is {got} bytes, expected {expected}")]
    Size { got: u64, expected: u64 },
    #[error("could not write the file: {0}")]
    Io(#[from] std::io::Error),
}

/// A cancel switch shared with the UI or CLI. Cloning shares it.
#[derive(Clone, Debug, Default)]
pub struct CancelDownload(Arc<AtomicBool>);

impl CancelDownload {
    pub fn new() -> Self {
        Self::default()
    }
    pub fn cancel(&self) {
        self.0.store(true, Ordering::Relaxed);
    }
    pub fn is_cancelled(&self) -> bool {
        self.0.load(Ordering::Relaxed)
    }
}

/// Bytes on disk so far and the file's full size when known.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Progress {
    pub done: u64,
    pub total: Option<u64>,
}

/// The `.part` file next to `dest`.
pub fn part_path(dest: &Path) -> PathBuf {
    let mut name = dest.file_name().unwrap_or_default().to_os_string();
    name.push(".part");
    dest.with_file_name(name)
}

/// The client used for model downloads: no overall timeout (files can be
/// hundreds of megabytes), a connect timeout, and Tern's user agent.
pub fn client() -> Result<reqwest::Client, DownloadError> {
    reqwest::Client::builder()
        .connect_timeout(Duration::from_secs(15))
        .user_agent(concat!("Tern/", env!("CARGO_PKG_VERSION")))
        .build()
        .map_err(|e| DownloadError::Network(e.to_string()))
}

/// Downloads `url` to `dest`, resuming a `.part` file when one is there.
/// `size` is the expected length when the manifest knows it; a finished
/// file of another length is an error (and the `.part` is removed so the
/// next attempt starts clean). Returns the file's length.
pub async fn fetch(
    client: &reqwest::Client,
    url: &str,
    dest: &Path,
    size: Option<u64>,
    cancel: &CancelDownload,
    on_progress: &mut (dyn FnMut(Progress) + Send),
) -> Result<u64, DownloadError> {
    if let Some(dir) = dest.parent() {
        std::fs::create_dir_all(dir)?;
    }
    let part = part_path(dest);
    let mut have = std::fs::metadata(&part).map(|m| m.len()).unwrap_or(0);
    // A `.part` already as long as the file only needs the rename.
    if let Some(total) = size
        && have >= total
    {
        if have == total {
            std::fs::rename(&part, dest)?;
            on_progress(Progress {
                done: total,
                total: Some(total),
            });
            return Ok(total);
        }
        std::fs::remove_file(&part)?;
        have = 0;
    }
    if cancel.is_cancelled() {
        return Err(DownloadError::Cancelled);
    }
    let mut req = client.get(url);
    if have > 0 {
        req = req.header(reqwest::header::RANGE, format!("bytes={have}-"));
    }
    let mut resp = req
        .send()
        .await
        .map_err(|e| DownloadError::Network(e.to_string()))?;
    let status = resp.status().as_u16();
    let resumed = match status {
        206 => true,
        200 => false,
        // The range starts at or past the end: the `.part` is whole or bad.
        416 if have > 0 => {
            std::fs::remove_file(&part)?;
            return Box::pin(fetch(client, url, dest, size, cancel, on_progress)).await;
        }
        s => return Err(DownloadError::Status(s)),
    };
    if !resumed {
        have = 0;
    }
    let total = size.or_else(|| resp.content_length().map(|n| n + have));
    let mut file = std::fs::OpenOptions::new()
        .create(true)
        .write(true)
        .append(resumed)
        .truncate(!resumed)
        .open(&part)?;
    on_progress(Progress { done: have, total });
    loop {
        if cancel.is_cancelled() {
            file.flush()?;
            return Err(DownloadError::Cancelled);
        }
        let chunk = resp
            .chunk()
            .await
            .map_err(|e| DownloadError::Network(e.to_string()))?;
        let Some(chunk) = chunk else { break };
        file.write_all(&chunk)?;
        have += chunk.len() as u64;
        on_progress(Progress { done: have, total });
    }
    file.flush()?;
    drop(file);
    if let Some(expected) = size
        && have != expected
    {
        if have > expected {
            std::fs::remove_file(&part)?;
        }
        return Err(DownloadError::Size {
            got: have,
            expected,
        });
    }
    std::fs::rename(&part, dest)?;
    Ok(have)
}

#[cfg(test)]
mod tests {
    use super::*;
    use wiremock::matchers::{header, header_exists, method, path};
    use wiremock::{Mock, MockServer, Request, Respond, ResponseTemplate};

    const BODY: &[u8] = b"0123456789abcdefghijklmnopqrstuvwxyz";

    /// Serves `BODY`, honouring `Range: bytes=N-` with a 206.
    struct Ranged;
    impl Respond for Ranged {
        fn respond(&self, req: &Request) -> ResponseTemplate {
            match req
                .headers
                .get("range")
                .and_then(|v| v.to_str().ok())
                .and_then(|v| v.strip_prefix("bytes="))
                .and_then(|v| v.trim_end_matches('-').parse::<usize>().ok())
            {
                Some(from) if from >= BODY.len() => ResponseTemplate::new(416),
                Some(from) => ResponseTemplate::new(206).set_body_bytes(&BODY[from..]),
                None => ResponseTemplate::new(200).set_body_bytes(BODY),
            }
        }
    }

    async fn server() -> MockServer {
        let s = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/m.bin"))
            .respond_with(Ranged)
            .mount(&s)
            .await;
        s
    }

    #[tokio::test]
    async fn downloads_whole_file_with_progress() {
        let s = server().await;
        let dir = tempfile_dir();
        let dest = dir.join("m.bin");
        let mut seen = Vec::new();
        let n = fetch(
            &client().unwrap(),
            &format!("{}/m.bin", s.uri()),
            &dest,
            Some(BODY.len() as u64),
            &CancelDownload::new(),
            &mut |p| seen.push(p),
        )
        .await
        .unwrap();
        assert_eq!(n, BODY.len() as u64);
        assert_eq!(std::fs::read(&dest).unwrap(), BODY);
        assert!(!part_path(&dest).exists());
        assert_eq!(seen.last().unwrap().done, BODY.len() as u64);
        assert_eq!(seen.last().unwrap().total, Some(BODY.len() as u64));
    }

    #[tokio::test]
    async fn resumes_a_part_file_with_a_range_request() {
        let s = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/m.bin"))
            .and(header("range", "bytes=10-"))
            .respond_with(Ranged)
            .expect(1)
            .mount(&s)
            .await;
        let dir = tempfile_dir();
        let dest = dir.join("m.bin");
        std::fs::write(part_path(&dest), &BODY[..10]).unwrap();
        let mut first = None;
        fetch(
            &client().unwrap(),
            &format!("{}/m.bin", s.uri()),
            &dest,
            Some(BODY.len() as u64),
            &CancelDownload::new(),
            &mut |p| {
                first.get_or_insert(p);
            },
        )
        .await
        .unwrap();
        assert_eq!(std::fs::read(&dest).unwrap(), BODY);
        assert_eq!(first.unwrap().done, 10);
    }

    #[tokio::test]
    async fn a_server_ignoring_the_range_restarts_the_file() {
        let s = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/m.bin"))
            .respond_with(ResponseTemplate::new(200).set_body_bytes(BODY))
            .mount(&s)
            .await;
        let dir = tempfile_dir();
        let dest = dir.join("m.bin");
        std::fs::write(part_path(&dest), b"garbage").unwrap();
        fetch(
            &client().unwrap(),
            &format!("{}/m.bin", s.uri()),
            &dest,
            None,
            &CancelDownload::new(),
            &mut |_| {},
        )
        .await
        .unwrap();
        assert_eq!(std::fs::read(&dest).unwrap(), BODY);
    }

    #[tokio::test]
    async fn cancel_keeps_the_part_and_stops() {
        let s = server().await;
        let dir = tempfile_dir();
        let dest = dir.join("m.bin");
        let cancel = CancelDownload::new();
        let c2 = cancel.clone();
        let err = fetch(
            &client().unwrap(),
            &format!("{}/m.bin", s.uri()),
            &dest,
            Some(BODY.len() as u64),
            &cancel,
            &mut move |_| c2.cancel(),
        )
        .await
        .unwrap_err();
        assert!(matches!(err, DownloadError::Cancelled));
        assert!(!dest.exists());
        assert!(part_path(&dest).exists());
    }

    #[tokio::test]
    async fn wrong_size_and_bad_status_are_errors() {
        let s = server().await;
        let dir = tempfile_dir();
        let dest = dir.join("m.bin");
        let err = fetch(
            &client().unwrap(),
            &format!("{}/m.bin", s.uri()),
            &dest,
            Some(5),
            &CancelDownload::new(),
            &mut |_| {},
        )
        .await
        .unwrap_err();
        assert!(matches!(err, DownloadError::Size { .. }), "{err}");
        assert!(!dest.exists());
        let err = fetch(
            &client().unwrap(),
            &format!("{}/missing", s.uri()),
            &dest,
            None,
            &CancelDownload::new(),
            &mut |_| {},
        )
        .await
        .unwrap_err();
        assert!(matches!(err, DownloadError::Status(404)), "{err}");
    }

    #[tokio::test]
    async fn no_range_header_on_a_fresh_start() {
        let s = MockServer::start().await;
        Mock::given(method("GET"))
            .and(header_exists("range"))
            .respond_with(ResponseTemplate::new(500))
            .mount(&s)
            .await;
        Mock::given(method("GET"))
            .respond_with(Ranged)
            .mount(&s)
            .await;
        let dir = tempfile_dir();
        let dest = dir.join("m.bin");
        fetch(
            &client().unwrap(),
            &format!("{}/m.bin", s.uri()),
            &dest,
            None,
            &CancelDownload::new(),
            &mut |_| {},
        )
        .await
        .unwrap();
        assert_eq!(std::fs::read(&dest).unwrap(), BODY);
    }

    /// A fresh folder under the system temp dir (no tempfile dev-dep here).
    fn tempfile_dir() -> PathBuf {
        use std::sync::atomic::AtomicU32;
        static N: AtomicU32 = AtomicU32::new(0);
        let d = std::env::temp_dir().join(format!(
            "tern-dl-{}-{}",
            std::process::id(),
            N.fetch_add(1, Ordering::Relaxed)
        ));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        d
    }
}
