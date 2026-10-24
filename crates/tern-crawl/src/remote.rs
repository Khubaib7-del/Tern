//! Hosted readers: Firecrawl and Jina Reader turn a page into clean
//! Markdown on their own servers (they render JavaScript, which Tern Crawl
//! does not). The user picks one in Settings › Search & Crawler; Tern Crawl
//! stays the default and the fallback when a hosted reader fails.
//!
//! The page's link is checked by the network guard before it is sent, so a
//! hosted reader is never asked to open a private or local address. Only
//! the link leaves this computer (and the key, to its own service).

use std::time::Duration;

use serde_json::Value;
use tern_core::SecretString;
use url::Url;

use crate::fetch::{FetchError, Page};
use crate::guard;
use crate::icon::{SourceIcon, site_of};

/// Which hosted reader.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RemoteKind {
    Firecrawl,
    Jina,
}

impl RemoteKind {
    pub fn id(self) -> &'static str {
        match self {
            Self::Firecrawl => "firecrawl",
            Self::Jina => "jina",
        }
    }

    pub fn name(self) -> &'static str {
        match self {
            Self::Firecrawl => "Firecrawl",
            Self::Jina => "Jina Reader",
        }
    }

    pub fn default_base(self) -> &'static str {
        match self {
            Self::Firecrawl => "https://api.firecrawl.dev",
            Self::Jina => "https://r.jina.ai",
        }
    }

    /// Firecrawl needs a key; Jina Reader works without one (a key lifts
    /// its rate limit).
    pub fn needs_key(self) -> bool {
        matches!(self, Self::Firecrawl)
    }

    pub fn from_id(id: &str) -> Option<Self> {
        match id {
            "firecrawl" => Some(Self::Firecrawl),
            "jina" => Some(Self::Jina),
            _ => None,
        }
    }
}

const TIMEOUT: Duration = Duration::from_secs(45);
/// The most text kept from one page, like Tern Crawl's own cap.
const MAX_TEXT: usize = 400_000;

/// A hosted reader, ready to call. Cheap to clone.
#[derive(Clone, Debug)]
pub struct RemoteReader {
    kind: RemoteKind,
    base: String,
    key: Option<SecretString>,
    http: reqwest::Client,
    /// Tests point the guard-free path at a mock server.
    check_target: bool,
}

impl RemoteReader {
    pub fn new(kind: RemoteKind, key: Option<SecretString>) -> Result<Self, FetchError> {
        Self::at(kind, kind.default_base(), key)
    }

    /// A reader at another address (a self-hosted Firecrawl, tests).
    pub fn at(kind: RemoteKind, base: &str, key: Option<SecretString>) -> Result<Self, FetchError> {
        let key = key.filter(|k| !k.is_empty());
        if kind.needs_key() && key.is_none() {
            return Err(FetchError::Network {
                site: kind.name().to_owned(),
                reason: "no API key saved; add one in Settings › Search & Crawler".into(),
            });
        }
        let mut b = reqwest::Client::builder()
            .timeout(TIMEOUT)
            .user_agent(concat!("Tern/", env!("CARGO_PKG_VERSION")));
        // A reader on this computer (a self-hosted Firecrawl) is never
        // sent through the system proxy.
        let local = Url::parse(base)
            .ok()
            .and_then(|u| {
                u.host_str()
                    .map(|h| h == "localhost" || h.starts_with("127."))
            })
            .unwrap_or(false);
        if local {
            b = b.no_proxy();
        }
        let http = b.build().map_err(|e| FetchError::Network {
            site: kind.name().to_owned(),
            reason: e.to_string(),
        })?;
        Ok(Self {
            kind,
            base: base.trim_end_matches('/').to_owned(),
            key,
            http,
            check_target: true,
        })
    }

    /// Skips the address check on the page link (tests read from a local
    /// mock server).
    #[doc(hidden)]
    pub fn without_target_check(mut self) -> Self {
        self.check_target = false;
        self
    }

    pub fn kind(&self) -> RemoteKind {
        self.kind
    }

    /// Reads `url` through the hosted reader.
    pub async fn read(&self, url: &str) -> Result<Page, FetchError> {
        let target = Url::parse(url.trim()).map_err(|_| FetchError::BadUrl(url.to_owned()))?;
        if self.check_target {
            guard::check_resolved(&target, false).await?;
        } else {
            guard::check_form(&target)?;
        }
        let service = self.kind.name().to_owned();
        let rb = match self.kind {
            RemoteKind::Firecrawl => {
                self.http
                    .post(format!("{}/v1/scrape", self.base))
                    .json(&serde_json::json!({
                        "url": target.as_str(),
                        "formats": ["markdown"],
                        "onlyMainContent": true,
                    }))
            }
            RemoteKind::Jina => self
                .http
                .get(format!("{}/{}", self.base, target.as_str()))
                .header("accept", "application/json")
                .header("x-return-format", "markdown"),
        };
        let rb = match &self.key {
            Some(k) => rb.bearer_auth(k.expose()),
            None => rb,
        };
        let resp = rb.send().await.map_err(|e| {
            if e.is_timeout() {
                FetchError::Timeout {
                    site: service.clone(),
                }
            } else {
                FetchError::Network {
                    site: service.clone(),
                    reason: "the request did not reach it".into(),
                }
            }
        })?;
        let status = resp.status().as_u16();
        if !(200..300).contains(&status) {
            return Err(FetchError::Status {
                site: service,
                status,
            });
        }
        let body: Value = resp.json().await.map_err(|_| FetchError::Network {
            site: service.clone(),
            reason: "it sent an answer Tern could not read".into(),
        })?;
        parse_answer(self.kind, &target, &body)
    }
}

/// Turns a hosted reader's JSON answer into a [`Page`].
pub fn parse_answer(kind: RemoteKind, target: &Url, body: &Value) -> Result<Page, FetchError> {
    let service = kind.name().to_owned();
    let (markdown, title, final_url, description) = match kind {
        RemoteKind::Firecrawl => {
            if body["success"].as_bool() == Some(false) {
                return Err(FetchError::Network {
                    site: service,
                    reason: body["error"]
                        .as_str()
                        .unwrap_or("it could not read the page")
                        .chars()
                        .take(200)
                        .collect(),
                });
            }
            let d = &body["data"];
            let meta = &d["metadata"];
            (
                d["markdown"].as_str().unwrap_or_default().to_owned(),
                meta["title"].as_str().map(str::to_owned),
                meta["sourceURL"]
                    .as_str()
                    .or(meta["url"].as_str())
                    .map(str::to_owned),
                meta["description"].as_str().map(str::to_owned),
            )
        }
        RemoteKind::Jina => {
            let d = &body["data"];
            (
                d["content"].as_str().unwrap_or_default().to_owned(),
                d["title"].as_str().map(str::to_owned),
                d["url"].as_str().map(str::to_owned),
                d["description"].as_str().map(str::to_owned),
            )
        }
    };
    if markdown.trim().is_empty() {
        return Err(FetchError::Empty {
            site: site_of(target),
        });
    }
    let final_url = final_url
        .and_then(|u| Url::parse(&u).ok())
        .filter(|u| guard::check_form(u).is_ok())
        .unwrap_or_else(|| target.clone());
    let truncated = markdown.len() > MAX_TEXT;
    let markdown: String = if truncated {
        let mut cut = MAX_TEXT;
        while !markdown.is_char_boundary(cut) {
            cut -= 1;
        }
        markdown[..cut].to_owned()
    } else {
        markdown
    };
    let title = title
        .filter(|t| !t.trim().is_empty())
        .unwrap_or_else(|| site_of(&final_url));
    Ok(Page {
        url: target.to_string(),
        site: site_of(&final_url),
        icon: SourceIcon::for_url(&final_url),
        final_url: final_url.to_string(),
        title,
        description,
        content_type: "text/markdown".into(),
        bytes: markdown.len(),
        text: plain(&markdown),
        truncated,
        markdown,
        links: Vec::new(),
    })
}

/// Markdown with images and link targets dropped, for the model's text.
fn plain(md: &str) -> String {
    let mut out = String::with_capacity(md.len());
    let b = md.as_bytes();
    let mut i = 0;
    while i < md.len() {
        // ![alt](src) → nothing; [text](href) → text.
        if b[i] == b'!' && b.get(i + 1) == Some(&b'[') {
            if let Some(end) = skip_link(md, i + 1) {
                i = end;
                continue;
            }
        }
        if b[i] == b'[' {
            if let Some(close) = md[i..].find("](").map(|c| i + c) {
                if let Some(end) = md[close..].find(')').map(|c| close + c + 1) {
                    out.push_str(&md[i + 1..close]);
                    i = end;
                    continue;
                }
            }
        }
        let ch = md[i..].chars().next().unwrap_or(' ');
        out.push(ch);
        i += ch.len_utf8();
    }
    out
}

fn skip_link(md: &str, open: usize) -> Option<usize> {
    let close = md[open..].find("](")? + open;
    let end = md[close..].find(')')? + close + 1;
    Some(end)
}

#[cfg(test)]
mod tests {
    use super::*;
    use wiremock::matchers::{header, method, path};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    #[test]
    fn firecrawl_needs_a_key_jina_does_not() {
        assert!(RemoteReader::new(RemoteKind::Firecrawl, None).is_err());
        assert!(RemoteReader::new(RemoteKind::Jina, None).is_ok());
        assert_eq!(RemoteKind::from_id("jina"), Some(RemoteKind::Jina));
        assert_eq!(RemoteKind::from_id("tern"), None);
    }

    #[test]
    fn plain_text_drops_images_and_link_targets() {
        assert_eq!(
            plain("See ![logo](a.png)[the docs](https://x.y/d) now."),
            "See the docs now."
        );
        assert_eq!(plain("a [b] c"), "a [b] c");
    }

    #[test]
    fn answers_become_pages() {
        let t = Url::parse("https://example.com/a").unwrap();
        let fc = serde_json::json!({"success": true, "data": {"markdown": "# Hi\n\nBody",
            "metadata": {"title": "Hi", "sourceURL": "https://example.com/a?x=1"}}});
        let p = parse_answer(RemoteKind::Firecrawl, &t, &fc).unwrap();
        assert_eq!(p.title, "Hi");
        assert_eq!(p.final_url, "https://example.com/a?x=1");
        assert_eq!(p.site, "example.com");
        assert!(p.text.contains("Body"));
        let bad = serde_json::json!({"success": false, "error": "blocked"});
        assert!(
            parse_answer(RemoteKind::Firecrawl, &t, &bad)
                .unwrap_err()
                .to_string()
                .contains("blocked")
        );
        let empty = serde_json::json!({"data": {"content": "  "}});
        assert!(matches!(
            parse_answer(RemoteKind::Jina, &t, &empty),
            Err(FetchError::Empty { .. })
        ));
        // A final URL with credentials is not trusted.
        let j = serde_json::json!({"data": {"content": "x", "url": "https://u:p@evil.example"}});
        assert_eq!(
            parse_answer(RemoteKind::Jina, &t, &j).unwrap().final_url,
            "https://example.com/a"
        );
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn firecrawl_and_jina_through_a_mock() {
        let s = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/v1/scrape"))
            .and(header("authorization", "Bearer fc-key"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "success": true,
                "data": {"markdown": "Rendered text", "metadata": {"title": "T"}}
            })))
            .mount(&s)
            .await;
        Mock::given(method("GET"))
            .and(path("/https://example.com/p"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "code": 200, "data": {"title": "J", "url": "https://example.com/p", "content": "Jina text"}
            })))
            .mount(&s)
            .await;
        let fc = RemoteReader::at(
            RemoteKind::Firecrawl,
            &s.uri(),
            Some(SecretString::new("fc-key")),
        )
        .unwrap()
        .without_target_check();
        let p = fc.read("https://example.com/p").await.unwrap();
        assert_eq!((p.title.as_str(), p.text.as_str()), ("T", "Rendered text"));
        let j = RemoteReader::at(RemoteKind::Jina, &s.uri(), None)
            .unwrap()
            .without_target_check();
        let p = j.read("https://example.com/p").await.unwrap();
        assert_eq!(p.text, "Jina text");
        // A wrong key is the service's HTTP error, not a crash.
        let wrong = RemoteReader::at(
            RemoteKind::Firecrawl,
            &s.uri(),
            Some(SecretString::new("nope")),
        )
        .unwrap()
        .without_target_check();
        assert!(matches!(
            wrong.read("https://example.com/p").await,
            Err(FetchError::Status { status: 404, .. })
        ));
        // Private links never reach a hosted reader.
        let guarded = RemoteReader::at(RemoteKind::Jina, &s.uri(), None).unwrap();
        assert!(matches!(
            guarded.read("http://127.0.0.1/admin").await,
            Err(FetchError::Blocked(_))
        ));
    }
}
