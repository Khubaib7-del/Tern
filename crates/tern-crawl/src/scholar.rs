//! HTTP for the scholarly APIs (Semantic Scholar, OpenAlex, arXiv,
//! Crossref, Unpaywall, CORE): one shared client that keeps each
//! service's pace (its own gap between requests), retries a rate limit or
//! a server error with backoff (a `Retry-After` is honoured, up to 30 s),
//! and caches answers for a day in memory and, with a cache folder, on
//! disk. What the answers mean is read in `tern_research::scholar`.
//!
//! The services are fixed public APIs, so requests go to their own
//! addresses (or a test server's), not through the reader's page guard.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use url::Url;

use crate::search::USER_AGENT;

const TIMEOUT: Duration = Duration::from_secs(25);
/// The largest answer kept (a page of results is a few hundred KB).
const MAX_BYTES: usize = 6 * 1024 * 1024;
/// How long an answer is reused.
pub const TTL: Duration = Duration::from_secs(24 * 60 * 60);
const MEMORY_CAP: usize = 512;
/// Retries after the first try.
const RETRIES: u32 = 3;
const MAX_WAIT: Duration = Duration::from_secs(30);

#[derive(Clone, Debug, PartialEq, Eq, thiserror::Error)]
pub enum ScholarError {
    #[error("{0} refused the key (HTTP {1}). Check it in Settings › Search & Crawlers")]
    BadKey(String, u16),
    #[error("{0} is rate limiting Tern; tried {1} times. Wait a minute, or add a key")]
    RateLimited(String, u32),
    #[error("{0} has nothing at that address (HTTP 404)")]
    NotFound(String),
    #[error("{0} answered HTTP {1}: {2}")]
    Http(String, u16, String),
    #[error("could not reach {0}: {1}")]
    Network(String, String),
    #[error("{0} sent more than {max} MB", max = MAX_BYTES / 1_048_576)]
    TooBig(String),
    #[error("`{0}` is not a valid address")]
    BadUrl(String),
}

impl ScholarError {
    /// Whether trying again later could help.
    pub fn transient(&self) -> bool {
        matches!(self, Self::RateLimited(..) | Self::Network(..))
            || matches!(self, Self::Http(_, c, _) if *c >= 500)
    }
}

/// One request: which service (for its pace and its name in errors),
/// the address, headers, and the gap the service asks for.
#[derive(Clone, Debug)]
pub struct ApiRequest {
    pub service: String,
    pub url: String,
    pub headers: Vec<(String, String)>,
    pub gap: Duration,
    /// Use a cached answer when there is one.
    pub cache: bool,
}

impl ApiRequest {
    pub fn get(service: &str, url: impl Into<String>, gap: Duration) -> Self {
        Self {
            service: service.to_owned(),
            url: url.into(),
            headers: Vec::new(),
            gap,
            cache: true,
        }
    }

    pub fn header(mut self, name: &str, value: impl Into<String>) -> Self {
        self.headers.push((name.to_owned(), value.into()));
        self
    }
}

/// An answer and whether it came from the cache.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ApiAnswer {
    pub body: String,
    pub cached: bool,
    /// Tries it took (1 when it worked first time, 0 from the cache).
    pub tries: u32,
}

#[derive(Debug, Default)]
struct Shared {
    next: Mutex<HashMap<String, Instant>>,
    memory: Mutex<HashMap<String, (Instant, String)>>,
}

/// The shared scholarly HTTP client.
#[derive(Clone, Debug)]
pub struct ScholarHttp {
    client: reqwest::Client,
    shared: Arc<Shared>,
    cache_dir: Option<PathBuf>,
    /// Backoff unit; shortened in tests.
    unit: Duration,
}

impl Default for ScholarHttp {
    fn default() -> Self {
        Self::new()
    }
}

impl ScholarHttp {
    pub fn new() -> Self {
        let client = reqwest::Client::builder()
            .timeout(TIMEOUT)
            .user_agent(USER_AGENT)
            .build()
            .unwrap_or_default();
        Self {
            client,
            shared: Arc::default(),
            cache_dir: None,
            unit: Duration::from_secs(1),
        }
    }

    /// Keeps answers on disk under `dir` for a day.
    pub fn with_cache_dir(mut self, dir: impl Into<PathBuf>) -> Self {
        self.cache_dir = Some(dir.into());
        self
    }

    /// The backoff unit (1 s; tests use a few milliseconds).
    pub fn with_backoff_unit(mut self, unit: Duration) -> Self {
        self.unit = unit;
        self
    }

    /// Whether an answer for `url` is cached and fresh.
    pub fn has_cached(&self, url: &str) -> bool {
        self.memory_get(url).is_some()
            || self
                .cache_dir
                .as_deref()
                .and_then(|d| disk_get(d, url))
                .is_some()
    }

    fn memory_get(&self, url: &str) -> Option<String> {
        let m = self.shared.memory.lock().unwrap_or_else(|e| e.into_inner());
        m.get(url)
            .filter(|(at, _)| at.elapsed() < TTL)
            .map(|(_, b)| b.clone())
    }

    fn remember(&self, url: &str, body: &str) {
        let mut m = self.shared.memory.lock().unwrap_or_else(|e| e.into_inner());
        if m.len() >= MEMORY_CAP
            && !m.contains_key(url)
            && let Some(oldest) = m
                .iter()
                .min_by_key(|(_, (at, _))| *at)
                .map(|(k, _)| k.clone())
        {
            m.remove(&oldest);
        }
        m.insert(url.to_owned(), (Instant::now(), body.to_owned()));
        drop(m);
        if let Some(d) = &self.cache_dir {
            disk_put(d, url, body);
        }
    }

    /// Waits for the service's next slot.
    async fn pace(&self, service: &str, gap: Duration) {
        if gap.is_zero() {
            return;
        }
        let wait = {
            let mut slots = self.shared.next.lock().unwrap_or_else(|e| e.into_inner());
            let now = Instant::now();
            let at = slots.get(service).copied().unwrap_or(now).max(now);
            slots.insert(service.to_owned(), at + gap);
            at - now
        };
        if !wait.is_zero() {
            tokio::time::sleep(wait).await;
        }
    }

    /// Sends `req`: from the cache when it can, else paced, with retries.
    pub async fn get(&self, req: &ApiRequest) -> Result<ApiAnswer, ScholarError> {
        let url = Url::parse(&req.url).map_err(|_| ScholarError::BadUrl(req.url.clone()))?;
        if !matches!(url.scheme(), "http" | "https") {
            return Err(ScholarError::BadUrl(req.url.clone()));
        }
        if req.cache {
            if let Some(body) = self.memory_get(&req.url) {
                return Ok(ApiAnswer {
                    body,
                    cached: true,
                    tries: 0,
                });
            }
            if let Some(body) = self
                .cache_dir
                .as_deref()
                .and_then(|d| disk_get(d, &req.url))
            {
                self.remember(&req.url, &body);
                return Ok(ApiAnswer {
                    body,
                    cached: true,
                    tries: 0,
                });
            }
        }
        let name = req.service.clone();
        let mut tries = 0;
        loop {
            tries += 1;
            self.pace(&req.service, req.gap).await;
            let mut r = self.client.get(url.clone());
            for (k, v) in &req.headers {
                r = r.header(k.as_str(), v.as_str());
            }
            let outcome = match r.send().await {
                Err(e) => Err((ScholarError::Network(name.clone(), short(&e)), None)),
                Ok(resp) => {
                    let status = resp.status().as_u16();
                    let after = resp
                        .headers()
                        .get("retry-after")
                        .and_then(|v| v.to_str().ok())
                        .and_then(|v| v.trim().parse::<u64>().ok())
                        .map(Duration::from_secs);
                    match status {
                        200..=299 => match read_capped(resp).await {
                            Ok(body) => Ok(body),
                            Err(true) => return Err(ScholarError::TooBig(name)),
                            Err(false) => Err((
                                ScholarError::Network(name.clone(), "the answer broke off".into()),
                                None,
                            )),
                        },
                        401 | 403 => return Err(ScholarError::BadKey(name, status)),
                        404 => return Err(ScholarError::NotFound(name)),
                        429 => Err((ScholarError::RateLimited(name.clone(), tries), after)),
                        500..=599 => Err((
                            ScholarError::Http(name.clone(), status, "server error".into()),
                            after,
                        )),
                        _ => {
                            let text = resp.text().await.unwrap_or_default();
                            let text: String = text.chars().take(200).collect();
                            return Err(ScholarError::Http(name, status, text.trim().to_owned()));
                        }
                    }
                }
            };
            match outcome {
                Ok(body) => {
                    if req.cache {
                        self.remember(&req.url, &body);
                    }
                    return Ok(ApiAnswer {
                        body,
                        cached: false,
                        tries,
                    });
                }
                Err((e, after)) => {
                    if tries > RETRIES {
                        return Err(match e {
                            ScholarError::RateLimited(n, _) => ScholarError::RateLimited(n, tries),
                            other => other,
                        });
                    }
                    let backoff = self.unit * 2u32.pow(tries - 1);
                    let wait = after
                        .map(|a| a.max(backoff))
                        .unwrap_or(backoff)
                        .min(MAX_WAIT);
                    tokio::time::sleep(wait).await;
                }
            }
        }
    }
}

fn short(e: &reqwest::Error) -> String {
    if e.is_timeout() {
        "timed out".into()
    } else if e.is_connect() {
        "could not connect".into()
    } else {
        e.to_string().chars().take(160).collect()
    }
}

/// The body as text, or `Err(true)` when it passes the cap, `Err(false)`
/// when it broke off.
async fn read_capped(mut resp: reqwest::Response) -> Result<String, bool> {
    let mut buf: Vec<u8> = Vec::new();
    loop {
        match resp.chunk().await {
            Ok(Some(c)) => {
                if buf.len() + c.len() > MAX_BYTES {
                    return Err(true);
                }
                buf.extend_from_slice(&c);
            }
            Ok(None) => break,
            Err(_) => return Err(false),
        }
    }
    Ok(String::from_utf8_lossy(&buf).into_owned())
}

fn file_for(dir: &Path, url: &str) -> PathBuf {
    // FNV-1a, stable across runs and platforms.
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    for b in url.bytes() {
        h ^= u64::from(b);
        h = h.wrapping_mul(0x0100_0000_01b3);
    }
    dir.join(format!("{h:016x}.txt"))
}

fn now_secs() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

/// A cached answer younger than a day. The file starts with the time it
/// was kept and the address on two lines, so a hash clash is caught.
fn disk_get(dir: &Path, url: &str) -> Option<String> {
    let text = std::fs::read_to_string(file_for(dir, url)).ok()?;
    let (at, rest) = text.split_once('\n')?;
    let (kept_url, body) = rest.split_once('\n')?;
    let at: u64 = at.trim().parse().ok()?;
    (kept_url == url && now_secs().saturating_sub(at) < TTL.as_secs()).then(|| body.to_owned())
}

fn disk_put(dir: &Path, url: &str, body: &str) {
    if std::fs::create_dir_all(dir).is_err() {
        return;
    }
    let _ = std::fs::write(
        file_for(dir, url),
        format!("{}\n{}\n{}", now_secs(), url.replace('\n', " "), body),
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn disk_cache_round_trips_and_checks_the_address() {
        let dir = std::env::temp_dir().join(format!("tern-scholar-cache-{}", std::process::id()));
        disk_put(
            &dir,
            "https://api.openalex.org/works?search=x",
            "{\"results\":[]}",
        );
        assert_eq!(
            disk_get(&dir, "https://api.openalex.org/works?search=x").as_deref(),
            Some("{\"results\":[]}")
        );
        assert_eq!(
            disk_get(&dir, "https://api.openalex.org/works?search=y"),
            None
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn transient_errors() {
        assert!(ScholarError::RateLimited("S2".into(), 4).transient());
        assert!(ScholarError::Http("S2".into(), 503, String::new()).transient());
        assert!(!ScholarError::BadKey("S2".into(), 403).transient());
    }
}
