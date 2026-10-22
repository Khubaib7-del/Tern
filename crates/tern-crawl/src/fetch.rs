//! The reader: fetches one URL through the guard and returns readable text.

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use reqwest::header::{CACHE_CONTROL, CONTENT_TYPE, LOCATION};
use serde::Serialize;
use url::Url;

use crate::cache;
use crate::extract::{extract_html, tidy};
use crate::guard::{self, GuardError};
use crate::icon::{SourceIcon, site_of};
use crate::robots::Robots;

/// The product token Tern's automated reads match in robots.txt.
pub const ROBOTS_AGENT: &str = "Tern";
/// The gap between two automated reads of one host when its robots.txt
/// names no Crawl-delay.
pub const DEFAULT_PACE: Duration = Duration::from_millis(500);
/// The longest Crawl-delay Tern honours; a longer one is capped here.
pub const MAX_CRAWL_DELAY: Duration = Duration::from_secs(10);
/// How many pages are read at once across all hosts.
pub const MAX_PARALLEL: usize = 8;
const ROBOTS_TTL: Duration = Duration::from_secs(24 * 60 * 60);
const ROBOTS_FAIL_TTL: Duration = Duration::from_secs(10 * 60);
const MAX_ROBOTS_BYTES: usize = 512 * 1024;

/// Pages larger than this are cut off and marked truncated.
pub const MAX_BYTES: usize = 2 * 1024 * 1024;
const MAX_REDIRECTS: usize = 5;
/// Favicons larger than this are skipped.
pub const MAX_ICON_BYTES: usize = 256 * 1024;
const TIMEOUT: Duration = Duration::from_secs(20);

/// One page as Tern read it.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct Page {
    /// The URL that was asked for.
    pub url: String,
    /// Where it ended up after redirects. Citations link here.
    pub final_url: String,
    pub site: String,
    pub title: String,
    pub description: Option<String>,
    pub icon: SourceIcon,
    pub content_type: String,
    pub text: String,
    pub bytes: usize,
    pub truncated: bool,
    /// The text as Markdown with links (HTML pages; plain text otherwise).
    pub markdown: String,
    /// Links in the main text (HTML pages only).
    pub links: Vec<crate::extract::Anchor>,
}

#[derive(Clone, Debug, PartialEq, Eq, thiserror::Error)]
pub enum FetchError {
    #[error("`{0}` is not a valid link")]
    BadUrl(String),
    #[error(transparent)]
    Blocked(#[from] GuardError),
    #[error("{site} answered HTTP {status}")]
    Status { site: String, status: u16 },
    #[error("{site} redirected more than {MAX_REDIRECTS} times")]
    TooManyRedirects { site: String },
    #[error("{site} took too long to answer")]
    Timeout { site: String },
    #[error("could not reach {site}: {reason}")]
    Network { site: String, reason: String },
    #[error("{site} sent {content_type}, which Tern cannot read yet{hint}")]
    Unsupported {
        site: String,
        content_type: String,
        hint: &'static str,
    },
    #[error("{site} sent an empty page")]
    Empty { site: String },
    #[error("{site} sent a PDF Tern could not read: {reason}")]
    Pdf { site: String, reason: String },
    #[error("{site}'s robots.txt asks automated readers not to open this page")]
    Robots { site: String },
}

/// What every clone of a [`Fetcher`] shares: robots files, pacing, the
/// page cache, and the parallel-read limit.
#[derive(Debug)]
struct Shared {
    robots: Mutex<HashMap<String, (Instant, Arc<Robots>)>>,
    next_slot: Mutex<HashMap<String, Instant>>,
    memory: Mutex<cache::Memory>,
    permits: tokio::sync::Semaphore,
}

impl Default for Shared {
    fn default() -> Self {
        Self {
            robots: Mutex::default(),
            next_slot: Mutex::default(),
            memory: Mutex::default(),
            permits: tokio::sync::Semaphore::new(MAX_PARALLEL),
        }
    }
}

/// Reads web pages. Cheap to clone.
#[derive(Clone, Debug)]
pub struct Fetcher {
    http: reqwest::Client,
    allow_private: bool,
    shared: Arc<Shared>,
    pace: Duration,
    cache_dir: Option<Arc<PathBuf>>,
}

impl Fetcher {
    /// The reader for the open web: private and local addresses are refused.
    pub fn new() -> Self {
        Self::build(false)
    }

    /// A reader that may reach local addresses (tests, a self-hosted service the user named).
    pub fn allowing_private() -> Self {
        Self::build(true)
    }

    fn build(allow_private: bool) -> Self {
        let mut b = reqwest::Client::builder()
            .timeout(TIMEOUT)
            .redirect(reqwest::redirect::Policy::none())
            .user_agent(concat!(
                "Mozilla/5.0 (compatible; Tern/",
                env!("CARGO_PKG_VERSION"),
                "; research reader)"
            ));
        if allow_private {
            b = b.no_proxy();
        }
        Self {
            http: b.build().expect("http client"),
            allow_private,
            shared: Arc::default(),
            // Local services are the user's own; tests run fast.
            pace: if allow_private {
                Duration::ZERO
            } else {
                DEFAULT_PACE
            },
            cache_dir: None,
        }
    }

    /// Keeps read pages in `dir` for a day, across restarts.
    pub fn with_cache_dir(mut self, dir: impl Into<PathBuf>) -> Self {
        self.cache_dir = Some(Arc::new(dir.into()));
        self
    }

    /// The gap between automated reads of one host when robots.txt names
    /// none.
    pub fn with_pace(mut self, pace: Duration) -> Self {
        self.pace = pace;
        self
    }

    /// Waits for this host's next slot: its Crawl-delay or the default
    /// pace after the previous read.
    async fn pace_host(&self, host: &str, delay: Option<f32>) {
        let gap = delay
            .map(|d| Duration::from_secs_f32(d).min(MAX_CRAWL_DELAY))
            .unwrap_or(self.pace);
        if gap.is_zero() {
            return;
        }
        let wait = {
            let mut slots = self.shared.next_slot.lock().expect("pace lock");
            let now = Instant::now();
            let at = slots.get(host).copied().unwrap_or(now).max(now);
            slots.insert(host.to_owned(), at + gap);
            at - now
        };
        if !wait.is_zero() {
            tokio::time::sleep(wait).await;
        }
    }

    /// The robots.txt rules for `url`'s origin, read once a day.
    pub async fn robots_for(&self, url: &Url) -> Arc<Robots> {
        let origin = url.origin().ascii_serialization();
        if let Some((until, r)) = self.shared.robots.lock().expect("robots lock").get(&origin) {
            if Instant::now() < *until {
                return r.clone();
            }
        }
        let (robots, ttl) = self.read_robots(url).await;
        let robots = Arc::new(robots);
        self.shared
            .robots
            .lock()
            .expect("robots lock")
            .insert(origin, (Instant::now() + ttl, robots.clone()));
        robots
    }

    async fn read_robots(&self, url: &Url) -> (Robots, Duration) {
        let Ok(mut at) = url.join("/robots.txt") else {
            return (Robots::allow_all(), ROBOTS_TTL);
        };
        for _ in 0..=MAX_REDIRECTS {
            if guard::check_resolved(&at, self.allow_private)
                .await
                .is_err()
            {
                return (Robots::allow_all(), ROBOTS_TTL);
            }
            let resp = match self
                .http
                .get(at.clone())
                .header("Accept", "text/plain")
                .send()
                .await
            {
                Ok(r) => r,
                // Unreachable: RFC 9309 says assume a full disallow, but
                // only briefly, so a flaky network does not lock a site out.
                Err(_) => return (Robots::unreachable(), ROBOTS_FAIL_TTL),
            };
            let status = resp.status();
            if status.is_redirection() {
                match resp
                    .headers()
                    .get(LOCATION)
                    .and_then(|v| v.to_str().ok())
                    .and_then(|l| at.join(l).ok())
                {
                    Some(next) => {
                        at = next;
                        continue;
                    }
                    None => return (Robots::allow_all(), ROBOTS_TTL),
                }
            }
            if status.as_u16() == 429 || status.is_server_error() {
                return (Robots::unreachable(), ROBOTS_FAIL_TTL);
            }
            if !status.is_success() {
                return (Robots::allow_all(), ROBOTS_TTL);
            }
            let site = site_of(&at);
            let Ok((body, _)) = read_capped_to(resp, &site, MAX_ROBOTS_BYTES).await else {
                return (Robots::unreachable(), ROBOTS_FAIL_TTL);
            };
            return (
                Robots::parse(&String::from_utf8_lossy(&body), ROBOTS_AGENT),
                ROBOTS_TTL,
            );
        }
        (Robots::allow_all(), ROBOTS_TTL)
    }

    /// Reads a page for an agent run: keeps to robots.txt, paced per host,
    /// cached.
    pub async fn fetch(&self, url: &str) -> Result<Page, FetchError> {
        self.fetch_cached(url, true).await
    }

    /// Reads a page the user opened themselves (the built-in browser): a
    /// person reading, not a crawl, so robots.txt and pacing do not apply.
    /// Still guarded and cached.
    pub async fn fetch_for_user(&self, url: &str) -> Result<Page, FetchError> {
        self.fetch_cached(url, false).await
    }

    async fn fetch_cached(&self, url: &str, automated: bool) -> Result<Page, FetchError> {
        let key = cache::key(url);
        if let Some(p) = self.cached(&key) {
            return Ok(p);
        }
        let _permit = self.shared.permits.acquire().await.expect("semaphore open");
        // Another read may have filled it while this one waited.
        if let Some(p) = self.cached(&key) {
            return Ok(p);
        }
        let (page, store) = self.fetch_live(url, automated).await?;
        if store {
            let now = Instant::now();
            let mut mem = self.shared.memory.lock().expect("cache lock");
            mem.put(key.clone(), page.clone(), now);
            let final_key = cache::key(&page.final_url);
            if final_key != key {
                mem.put(final_key, page.clone(), now);
            }
            drop(mem);
            if let Some(dir) = &self.cache_dir {
                cache::disk_put(dir, &key, &page);
            }
        }
        Ok(page)
    }

    fn cached(&self, key: &str) -> Option<Page> {
        let now = Instant::now();
        if let Some(p) = self.shared.memory.lock().expect("cache lock").get(key, now) {
            return Some(p);
        }
        let dir = self.cache_dir.as_ref()?;
        let p = cache::disk_get(dir, key)?;
        self.shared
            .memory
            .lock()
            .expect("cache lock")
            .put(key.to_owned(), p.clone(), now);
        Some(p)
    }

    /// Downloads a site icon through the same network guard as pages.
    /// Returns the raw bytes when they look like an image (PNG, ICO, GIF,
    /// JPEG, WebP or SVG) and fit in [`MAX_ICON_BYTES`].
    pub async fn fetch_icon(&self, url: &str) -> Result<Vec<u8>, FetchError> {
        let mut current = Url::parse(url.trim()).map_err(|_| FetchError::BadUrl(url.to_owned()))?;
        for _ in 0..=MAX_REDIRECTS {
            guard::check_resolved(&current, self.allow_private).await?;
            let site = site_of(&current);
            let resp = self
                .http
                .get(current.clone())
                .header("Accept", "image/*")
                .send()
                .await
                .map_err(|e| net_error(&site, &e))?;
            let status = resp.status();
            if status.is_redirection() {
                current = resp
                    .headers()
                    .get(LOCATION)
                    .and_then(|v| v.to_str().ok())
                    .and_then(|l| current.join(l).ok())
                    .ok_or(FetchError::Status {
                        site: site.clone(),
                        status: status.as_u16(),
                    })?;
                continue;
            }
            if !status.is_success() {
                return Err(FetchError::Status {
                    site,
                    status: status.as_u16(),
                });
            }
            let mut resp = resp;
            let mut body = Vec::new();
            while let Some(chunk) = resp.chunk().await.map_err(|e| net_error(&site, &e))? {
                body.extend_from_slice(&chunk);
                if body.len() > MAX_ICON_BYTES {
                    return Err(FetchError::Unsupported {
                        site,
                        content_type: "an icon over 256 KB".into(),
                        hint: "",
                    });
                }
            }
            if !looks_like_image(&body) {
                return Err(FetchError::Unsupported {
                    site,
                    content_type: "an icon that is not an image".into(),
                    hint: "",
                });
            }
            return Ok(body);
        }
        Err(FetchError::TooManyRedirects {
            site: site_of(&current),
        })
    }

    /// One live read. Returns the page and whether it may be cached.
    async fn fetch_live(&self, url: &str, automated: bool) -> Result<(Page, bool), FetchError> {
        let start = Url::parse(url.trim()).map_err(|_| FetchError::BadUrl(url.to_owned()))?;
        let mut current = start.clone();
        for _ in 0..=MAX_REDIRECTS {
            guard::check_resolved(&current, self.allow_private).await?;
            let site = site_of(&current);
            if automated {
                let robots = self.robots_for(&current).await;
                let path = match current.query() {
                    Some(q) => format!("{}?{q}", current.path()),
                    None => current.path().to_owned(),
                };
                if !robots.allows(&path) {
                    return Err(FetchError::Robots { site });
                }
                self.pace_host(current.host_str().unwrap_or_default(), robots.crawl_delay)
                    .await;
            }
            let resp = self
                .http
                .get(current.clone())
                .header("Accept", "text/html,application/xhtml+xml,text/plain;q=0.9,application/json;q=0.5,*/*;q=0.1")
                .send()
                .await
                .map_err(|e| net_error(&site, &e))?;
            let status = resp.status();
            if status.is_redirection() {
                let next = resp
                    .headers()
                    .get(LOCATION)
                    .and_then(|v| v.to_str().ok())
                    .and_then(|l| current.join(l).ok())
                    .ok_or(FetchError::Status {
                        site: site.clone(),
                        status: status.as_u16(),
                    })?;
                current = next;
                continue;
            }
            if !status.is_success() {
                return Err(FetchError::Status {
                    site,
                    status: status.as_u16(),
                });
            }
            let content_type = resp
                .headers()
                .get(CONTENT_TYPE)
                .and_then(|v| v.to_str().ok())
                .unwrap_or("text/html")
                .split(';')
                .next()
                .unwrap_or_default()
                .trim()
                .to_ascii_lowercase();
            if content_type == "application/pdf"
                || (content_type == "application/octet-stream"
                    && current.path().to_ascii_lowercase().ends_with(".pdf"))
            {
                let no_store = no_store(&resp);
                let (body, truncated) =
                    read_capped_to(resp, &site, crate::pdf::MAX_PDF_BYTES + 1).await?;
                if truncated || body.len() > crate::pdf::MAX_PDF_BYTES {
                    return Err(FetchError::Unsupported {
                        site,
                        content_type: "a PDF over 32 MB".into(),
                        hint: "",
                    });
                }
                let pdf = tokio::task::spawn_blocking(move || {
                    let r = crate::pdf::read(&body);
                    (r, body.len())
                })
                .await;
                let (pdf, bytes) = pdf.map_err(|_| FetchError::Pdf {
                    site: site.clone(),
                    reason: "the reader stopped".into(),
                })?;
                let pdf = pdf.map_err(|e| FetchError::Pdf {
                    site: site.clone(),
                    reason: e.to_string(),
                })?;
                let text = pdf.joined();
                let file = current
                    .path_segments()
                    .and_then(|mut s| s.next_back().map(str::to_owned))
                    .filter(|s| !s.is_empty());
                return Ok((
                    Page {
                        url: start.to_string(),
                        final_url: current.to_string(),
                        title: pdf.title.clone().or(file).unwrap_or_else(|| site.clone()),
                        icon: SourceIcon::for_url(&current),
                        site,
                        description: Some(format!("PDF, {} pages", pdf.pages.len())),
                        content_type: "application/pdf".into(),
                        markdown: text.clone(),
                        text,
                        bytes,
                        truncated: false,
                        links: Vec::new(),
                    },
                    !no_store,
                ));
            }
            let mut html = matches!(content_type.as_str(), "text/html" | "application/xhtml+xml");
            let texty = html
                || content_type.starts_with("text/")
                || content_type == "application/json"
                || content_type.ends_with("+json")
                || content_type.ends_with("+xml")
                || content_type == "application/xml";
            if !texty {
                return Err(FetchError::Unsupported {
                    site,
                    content_type,
                    hint: "",
                });
            }
            let no_store = no_store(&resp);
            let (body, truncated) = read_capped(resp, &site).await?;
            let raw = String::from_utf8_lossy(&body);
            // Some servers label HTML as plain text.
            if content_type == "text/plain" {
                let head: String = raw
                    .trim_start()
                    .chars()
                    .take(512)
                    .collect::<String>()
                    .to_ascii_lowercase();
                html |= head.starts_with("<!doctype html") || head.starts_with("<html");
            }
            let mut icon = SourceIcon::for_url(&current);
            let (title, description, text, markdown, links) = if html {
                let e = extract_html(&raw, &current);
                if let Some(i) = e.icon {
                    icon.favicon = Some(i);
                }
                (e.title, e.description, e.text, e.markdown, e.links)
            } else {
                let t = tidy(&raw);
                (None, None, t.clone(), t, Vec::new())
            };
            if text.trim().is_empty() {
                return Err(FetchError::Empty { site });
            }
            return Ok((
                Page {
                    url: start.to_string(),
                    final_url: current.to_string(),
                    title: title.unwrap_or_else(|| site.clone()),
                    site,
                    description,
                    icon,
                    content_type,
                    text,
                    bytes: body.len(),
                    truncated,
                    markdown,
                    links,
                },
                !no_store,
            ));
        }
        Err(FetchError::TooManyRedirects {
            site: site_of(&current),
        })
    }
}

impl Default for Fetcher {
    fn default() -> Self {
        Self::new()
    }
}

/// Magic bytes of the image formats the desktop app can draw.
pub fn looks_like_image(b: &[u8]) -> bool {
    let head = String::from_utf8_lossy(&b[..b.len().min(256)]).to_ascii_lowercase();
    b.starts_with(b"\x89PNG")
        || b.starts_with(&[0, 0, 1, 0])
        || b.starts_with(b"GIF8")
        || b.starts_with(&[0xFF, 0xD8, 0xFF])
        || (b.len() > 12 && &b[..4] == b"RIFF" && &b[8..12] == b"WEBP")
        || head.trim_start().starts_with("<svg")
        || (head.trim_start().starts_with("<?xml") && head.contains("<svg"))
}

async fn read_capped(resp: reqwest::Response, site: &str) -> Result<(Vec<u8>, bool), FetchError> {
    read_capped_to(resp, site, MAX_BYTES).await
}

async fn read_capped_to(
    mut resp: reqwest::Response,
    site: &str,
    max: usize,
) -> Result<(Vec<u8>, bool), FetchError> {
    let mut body = Vec::new();
    while let Some(chunk) = resp.chunk().await.map_err(|e| net_error(site, &e))? {
        let room = max - body.len();
        if chunk.len() >= room {
            body.extend_from_slice(&chunk[..room]);
            return Ok((body, true));
        }
        body.extend_from_slice(&chunk);
    }
    Ok((body, false))
}

fn no_store(resp: &reqwest::Response) -> bool {
    resp.headers()
        .get(CACHE_CONTROL)
        .and_then(|v| v.to_str().ok())
        .is_some_and(|v| v.to_ascii_lowercase().contains("no-store"))
}

fn net_error(site: &str, e: &reqwest::Error) -> FetchError {
    if e.is_timeout() {
        FetchError::Timeout {
            site: site.to_owned(),
        }
    } else {
        let mut reason = e.to_string();
        let mut src = std::error::Error::source(e);
        while let Some(s) = src {
            reason = s.to_string();
            src = s.source();
        }
        FetchError::Network {
            site: site.to_owned(),
            reason,
        }
    }
}
