//! Web search through the user's own search API key, a self-hosted SearXNG,
//! or the keyless Wikipedia search API (see `docs/adr/0012-keyless-search.md`).

use std::time::Duration;

use serde::Serialize;
use serde_json::{Value, json};
use tern_core::SecretString;
use url::Url;

use crate::icon::{SourceIcon, site_of};

const TIMEOUT: Duration = Duration::from_secs(20);

/// Who is calling, with a way to reach the project, as the Wikimedia
/// User-Agent policy asks of every API client.
pub const USER_AGENT: &str = concat!(
    "Tern/",
    env!("CARGO_PKG_VERSION"),
    " (https://github.com/Khubaib7-del/Tern; research assistant)"
);

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Api {
    Tavily,
    Brave,
    Searxng,
    /// The MediaWiki Action API's full-text search on Wikipedia.
    Wikipedia,
}

/// A search service Tern can use.
#[derive(Debug)]
pub struct SearchSpec {
    pub id: &'static str,
    pub name: &'static str,
    pub api: Api,
    pub needs_key: bool,
    /// Runs on the user's own server (its address can be changed).
    pub self_hosted: bool,
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
        self_hosted: false,
        default_base: "https://api.tavily.com",
        key_url: "https://app.tavily.com",
        icon: "tavily",
    },
    SearchSpec {
        id: "brave",
        name: "Brave Search",
        api: Api::Brave,
        needs_key: true,
        self_hosted: false,
        default_base: "https://api.search.brave.com",
        key_url: "https://api-dashboard.search.brave.com",
        icon: "brave",
    },
    SearchSpec {
        id: "searxng",
        name: "SearXNG",
        api: Api::Searxng,
        needs_key: false,
        self_hosted: true,
        default_base: "http://localhost:8080",
        key_url: "",
        icon: "searxng",
    },
    SearchSpec {
        id: "wikipedia",
        name: "Wikipedia",
        api: Api::Wikipedia,
        needs_key: false,
        self_hosted: false,
        default_base: "https://en.wikipedia.org",
        key_url: "",
        icon: "wikipedia",
    },
];

/// The search Tern uses when no other is set up: free, keyless, and
/// allowed by its operator's API terms for programs like this one.
pub const KEYLESS_SEARCH: &str = "wikipedia";

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
    #[error("unknown search service `{0}`. Try tavily, brave, searxng, or wikipedia")]
    Unknown(String),
    #[error("{0} needs an API key. Add one with `tern search add {1}`")]
    MissingKey(&'static str, &'static str),
    #[error("`{0}` is not a valid URL")]
    BadUrl(String),
    #[error("{0} refused the key (HTTP {1}). Check it, or add a new one")]
    BadKey(&'static str, u16),
    #[error("{0} is rate limiting Tern. Wait a minute and try again, or check your plan")]
    RateLimited(&'static str),
    /// SearXNG answers, but its `settings.yml` has JSON results turned off.
    #[error(
        "SearXNG at {0} refused JSON results (HTTP 403). In its settings.yml, under `search:`, set `formats: [html, json]`, then restart it"
    )]
    JsonOff(String),
    /// Nothing answered at the address (refused, no route, timed out).
    #[error("{name} at {at} did not answer: {reason}. {hint}")]
    Unreachable {
        name: &'static str,
        at: String,
        reason: String,
        hint: &'static str,
    },
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
            .user_agent(USER_AGENT);
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

    /// The address it sends queries to, without a trailing slash.
    pub fn base(&self) -> &str {
        self.base.as_str().trim_end_matches('/')
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
            Api::Wikipedia => self.http.get(self.url_with(
                "/w/api.php",
                &[
                    ("action", "query"),
                    ("list", "search"),
                    ("srsearch", query),
                    ("srlimit", &limit.clamp(1, 20).to_string()),
                    ("srprop", "snippet"),
                    ("format", "json"),
                    ("formatversion", "2"),
                    ("utf8", "1"),
                ],
            )),
        };
        let hint = if self.spec.self_hosted {
            "Check that it is running, or set its server address in Providers › Search"
        } else {
            "Check your internet connection"
        };
        let resp = req.send().await.map_err(|e| SearchError::Unreachable {
            name,
            at: self.base().to_owned(),
            reason: if e.is_timeout() {
                "timed out".into()
            } else {
                root_cause(&e)
            },
            hint,
        })?;
        let status = resp.status().as_u16();
        let body = resp
            .text()
            .await
            .map_err(|e| SearchError::Network(name, e.to_string()))?;
        match status {
            200..=299 => {}
            403 if self.spec.api == Api::Searxng => {
                return Err(SearchError::JsonOff(self.base().to_owned()));
            }
            401 | 403 => return Err(SearchError::BadKey(name, status)),
            429 => return Err(SearchError::RateLimited(name)),
            _ => {
                let short: String = body.chars().take(200).collect();
                return Err(SearchError::Http(name, status, short));
            }
        }
        let v: Value =
            serde_json::from_str(&body).map_err(|e| SearchError::BadReply(name, e.to_string()))?;
        if self.spec.api == Api::Wikipedia {
            return self.wikipedia_hits(&v, limit);
        }
        let rows = match self.spec.api {
            Api::Tavily => v["results"].as_array(),
            Api::Brave => v["web"]["results"].as_array(),
            Api::Searxng | Api::Wikipedia => v["results"].as_array(),
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

impl SearchClient {
    /// Wikipedia's `query.search` rows as hits: each title becomes its
    /// article's URL on the same wiki.
    fn wikipedia_hits(&self, v: &Value, limit: usize) -> Result<Vec<SearchHit>, SearchError> {
        if let Some(e) = v.get("error") {
            let info = e["info"].as_str().unwrap_or("unknown error");
            return Err(SearchError::BadReply(self.spec.name, info.to_owned()));
        }
        let rows = v["query"]["search"].as_array().ok_or_else(|| {
            SearchError::BadReply(self.spec.name, "no `query.search` in the reply".into())
        })?;
        let mut out = Vec::new();
        for r in rows.iter().take(limit) {
            let Some(title) = r["title"].as_str().filter(|t| !t.trim().is_empty()) else {
                continue;
            };
            let mut u = self.base.clone();
            u.set_query(None);
            u.set_path(&format!("/wiki/{}", title.replace(' ', "_")));
            out.push(SearchHit {
                title: title.to_owned(),
                snippet: strip_tags(r["snippet"].as_str().unwrap_or_default()),
                site: site_of(&u),
                icon: SourceIcon::for_url(&u),
                url: u.to_string(),
            });
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
        assert!(SearchClient::new(find_search(KEYLESS_SEARCH).unwrap(), None, None).is_ok());
    }

    #[test]
    fn the_user_agent_says_how_to_reach_the_project() {
        assert!(USER_AGENT.starts_with("Tern/"));
        assert!(USER_AGENT.contains("https://github.com/Khubaib7-del/Tern"));
    }
}
