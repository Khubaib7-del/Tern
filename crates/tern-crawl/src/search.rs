//! Web search through the user's own search API key, or a self-hosted SearXNG.

use std::time::Duration;

use serde::Serialize;
use serde_json::{Value, json};
use tern_core::SecretString;
use url::Url;

use crate::icon::{SourceIcon, site_of};

const TIMEOUT: Duration = Duration::from_secs(20);

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Api {
    Tavily,
    Brave,
    Searxng,
}

/// A search service Tern can use.
#[derive(Debug)]
pub struct SearchSpec {
    pub id: &'static str,
    pub name: &'static str,
    pub api: Api,
    pub needs_key: bool,
    pub default_base: &'static str,
    /// Where to create a key; empty for self-hosted.
    pub key_url: &'static str,
    /// The built-in glyph the app shows for this service.
    pub icon: &'static str,
}

pub static SEARCH_CATALOG: &[SearchSpec] = &[
    SearchSpec {
        id: "tavily",
        name: "Tavily",
        api: Api::Tavily,
        needs_key: true,
        default_base: "https://api.tavily.com",
        key_url: "https://app.tavily.com",
        icon: "tavily",
    },
    SearchSpec {
        id: "brave",
        name: "Brave Search",
        api: Api::Brave,
        needs_key: true,
        default_base: "https://api.search.brave.com",
        key_url: "https://api-dashboard.search.brave.com",
        icon: "brave",
    },
    SearchSpec {
        id: "searxng",
        name: "SearXNG",
        api: Api::Searxng,
        needs_key: false,
        default_base: "http://localhost:8080",
        key_url: "",
        icon: "searxng",
    },
];

pub fn find_search(id: &str) -> Option<&'static SearchSpec> {
    SEARCH_CATALOG
        .iter()
        .find(|s| s.id.eq_ignore_ascii_case(id))
}

/// One result, before it is read.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct SearchHit {
    pub title: String,
    pub url: String,
    pub snippet: String,
    pub site: String,
    pub icon: SourceIcon,
}

#[derive(Clone, Debug, PartialEq, Eq, thiserror::Error)]
pub enum SearchError {
    #[error("unknown search service `{0}`. Try tavily, brave, or searxng")]
    Unknown(String),
    #[error("{0} needs an API key. Add one with `tern search add {1}`")]
    MissingKey(&'static str, &'static str),
    #[error("`{0}` is not a valid URL")]
    BadUrl(String),
    #[error("{0} refused the key (HTTP {1}). Check it, or add a new one")]
    BadKey(&'static str, u16),
    #[error("{0} is rate limiting this key. Wait a minute, or check your plan")]
    RateLimited(&'static str),
    #[error("{0} answered HTTP {1}: {2}")]
    Http(&'static str, u16, String),
    #[error("could not reach {0}: {1}")]
    Network(&'static str, String),
    #[error("{0} sent a reply Tern could not read: {1}")]
    BadReply(&'static str, String),
}

#[derive(Debug)]
pub struct SearchClient {
    spec: &'static SearchSpec,
    key: Option<SecretString>,
    base: Url,
    http: reqwest::Client,
}

impl SearchClient {
    pub fn new(
        spec: &'static SearchSpec,
        key: Option<SecretString>,
        base_url: Option<&str>,
    ) -> Result<Self, SearchError> {
        let raw = base_url.unwrap_or(spec.default_base).trim_end_matches('/');
        let base = Url::parse(raw).map_err(|_| SearchError::BadUrl(raw.to_owned()))?;
        if spec.needs_key && key.as_ref().is_none_or(SecretString::is_empty) {
            return Err(SearchError::MissingKey(spec.name, spec.id));
        }
        let mut b = reqwest::Client::builder()
            .timeout(TIMEOUT)
            .user_agent(concat!("Tern/", env!("CARGO_PKG_VERSION")));
        let local = matches!(
            base.host_str(),
            Some("localhost" | "127.0.0.1" | "::1" | "[::1]")
        );
        if local {
            b = b.no_proxy();
        }
        Ok(Self {
            spec,
            key,
            base,
            http: b
                .build()
                .map_err(|e| SearchError::Network(spec.name, e.to_string()))?,
        })
    }

    pub fn spec(&self) -> &'static SearchSpec {
        self.spec
    }

    fn url(&self, path: &str) -> String {
        format!("{}{path}", self.base.as_str().trim_end_matches('/'))
    }

    fn url_with(&self, path: &str, params: &[(&str, &str)]) -> String {
        let mut u = Url::parse(&self.url(path)).expect("base is a valid URL");
        u.query_pairs_mut().extend_pairs(params);
        u.to_string()
    }

    /// Up to `limit` results, in the service's order, without duplicates.
    pub async fn search(&self, query: &str, limit: usize) -> Result<Vec<SearchHit>, SearchError> {
        let name = self.spec.name;
        let key = self
            .key
            .as_ref()
            .map(|k| k.expose().to_owned())
            .unwrap_or_default();
        let req = match self.spec.api {
            Api::Tavily => self
                .http
                .post(self.url("/search"))
                .bearer_auth(&key)
                .json(&json!({"query": query, "max_results": limit, "search_depth": "basic"})),
            Api::Brave => self
                .http
                .get(self.url_with(
                    "/res/v1/web/search",
                    &[("q", query), ("count", &limit.min(20).to_string())],
                ))
                .header("X-Subscription-Token", &key)
                .header("Accept", "application/json"),
            Api::Searxng => self
                .http
                .get(self.url_with("/search", &[("q", query), ("format", "json")])),
        };
        let resp = req.send().await.map_err(|e| {
            if e.is_timeout() {
                SearchError::Network(name, "timed out".into())
            } else {
                SearchError::Network(name, root_cause(&e))
            }
        })?;
        let status = resp.status().as_u16();
        let body = resp
            .text()
            .await
            .map_err(|e| SearchError::Network(name, e.to_string()))?;
        match status {
            200..=299 => {}
            401 | 403 => return Err(SearchError::BadKey(name, status)),
            429 => return Err(SearchError::RateLimited(name)),
            _ => {
                let short: String = body.chars().take(200).collect();
                return Err(SearchError::Http(name, status, short));
            }
        }
        let v: Value =
            serde_json::from_str(&body).map_err(|e| SearchError::BadReply(name, e.to_string()))?;
        let rows = match self.spec.api {
            Api::Tavily => v["results"].as_array(),
            Api::Brave => v["web"]["results"].as_array(),
            Api::Searxng => v["results"].as_array(),
        };
        let snippet_key = match self.spec.api {
            Api::Brave => "description",
            _ => "content",
        };
        let mut out: Vec<SearchHit> = Vec::new();
        for r in rows.into_iter().flatten() {
            let Some(u) = r["url"].as_str().and_then(|u| Url::parse(u).ok()) else {
                continue;
            };
            if !matches!(u.scheme(), "http" | "https") || out.iter().any(|h| h.url == u.as_str()) {
                continue;
            }
            out.push(SearchHit {
                title: strip_tags(r["title"].as_str().unwrap_or_default()),
                snippet: strip_tags(r[snippet_key].as_str().unwrap_or_default()),
                site: site_of(&u),
                icon: SourceIcon::for_url(&u),
                url: u.to_string(),
            });
            if out.len() >= limit {
                break;
            }
        }
        Ok(out)
    }
}

fn root_cause(e: &reqwest::Error) -> String {
    let mut reason = e.to_string();
    let mut src = std::error::Error::source(e);
    while let Some(s) = src {
        reason = s.to_string();
        src = s.source();
    }
    reason
}

/// Removes `<strong>`-style tags and decodes the common entities.
fn strip_tags(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut in_tag = false;
    for c in s.chars() {
        match c {
            '<' => in_tag = true,
            '>' if in_tag => in_tag = false,
            _ if !in_tag => out.push(c),
            _ => {}
        }
    }
    out.replace("&amp;", "&")
        .replace("&lt;", "<")
        .replace("&gt;", ">")
        .replace("&quot;", "\"")
        .replace("&#39;", "'")
        .replace("&#x27;", "'")
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn strips_markup_from_snippets() {
        assert_eq!(
            strip_tags("The <strong>Transformer</strong> &amp; attention"),
            "The Transformer & attention"
        );
    }

    #[test]
    fn keyed_services_need_a_key() {
        let e = SearchClient::new(find_search("tavily").unwrap(), None, None).unwrap_err();
        assert_eq!(
            e.to_string(),
            "Tavily needs an API key. Add one with `tern search add tavily`"
        );
        assert!(SearchClient::new(find_search("searxng").unwrap(), None, None).is_ok());
    }
}
