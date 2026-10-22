//! The reader: fetches one URL through the guard and returns readable text.

use std::time::Duration;

use reqwest::header::{CONTENT_TYPE, LOCATION};
use serde::Serialize;
use url::Url;

use crate::extract::{extract_html, tidy};
use crate::guard::{self, GuardError};
use crate::icon::{SourceIcon, site_of};

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
}

/// Reads web pages. Cheap to clone.
#[derive(Clone, Debug)]
pub struct Fetcher {
    http: reqwest::Client,
    allow_private: bool,
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
        }
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

    pub async fn fetch(&self, url: &str) -> Result<Page, FetchError> {
        let start = Url::parse(url.trim()).map_err(|_| FetchError::BadUrl(url.to_owned()))?;
        let mut current = start.clone();
        for _ in 0..=MAX_REDIRECTS {
            guard::check_resolved(&current, self.allow_private).await?;
            let site = site_of(&current);
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
            if content_type == "application/pdf" {
                return Err(FetchError::Unsupported {
                    site,
                    content_type,
                    hint: " (PDF reading arrives with the reader in phase 4)",
                });
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
            let (title, description, text) = if html {
                let e = extract_html(&raw, &current);
                if let Some(i) = e.icon {
                    icon.favicon = Some(i);
                }
                (e.title, e.description, e.text)
            } else {
                (None, None, tidy(&raw))
            };
            if text.trim().is_empty() {
                return Err(FetchError::Empty { site });
            }
            return Ok(Page {
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
            });
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

async fn read_capped(
    mut resp: reqwest::Response,
    site: &str,
) -> Result<(Vec<u8>, bool), FetchError> {
    let mut body = Vec::new();
    while let Some(chunk) = resp.chunk().await.map_err(|e| net_error(site, &e))? {
        let room = MAX_BYTES - body.len();
        if chunk.len() >= room {
            body.extend_from_slice(&chunk[..room]);
            return Ok((body, true));
        }
        body.extend_from_slice(&chunk);
    }
    Ok((body, false))
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
