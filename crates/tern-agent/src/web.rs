//! The web tools the turn loop can call: `web_search` and `read_url`.

use std::sync::Arc;
use std::time::Duration;

use serde_json::{Value, json};
use tern_core::EffectClass;
use tern_crawl::{Fetcher, SearchClient};

use crate::sources::PageRef;
use crate::tools::{BoxFuture, Tool, ToolError, ToolOutput};

/// Characters of a page handed to the model per read.
pub const READ_CHARS: usize = 8_000;

/// Web search. With a decider, results come back ranked by the decision
/// model (pages most worth reading first), and the decision rides in the
/// tool's metadata for the run's Decisions view.
#[derive(Debug, Clone)]
pub struct WebSearchTool(
    pub Arc<SearchClient>,
    pub Option<Arc<crate::decide::Decider>>,
);

impl Tool for WebSearchTool {
    fn name(&self) -> &'static str {
        "web_search"
    }
    fn summary(&self) -> &'static str {
        "Search the web. Returns titles, links, and snippets; read a link before citing it."
    }
    fn effect(&self) -> EffectClass {
        EffectClass::Network
    }
    fn input_schema(&self) -> Value {
        json!({"type": "object", "required": ["query"], "additionalProperties": false,
               "properties": {"query": {"type": "string"}, "limit": {"type": "integer"}}})
    }
    fn target(&self, _args: &Value) -> Option<String> {
        Some(self.0.spec().id.to_owned())
    }
    fn idempotent(&self) -> bool {
        true
    }
    fn call<'a>(&'a self, args: Value) -> BoxFuture<'a, Result<ToolOutput, ToolError>> {
        Box::pin(async move {
            let q = args["query"].as_str().unwrap_or_default();
            let limit = args["limit"].as_u64().unwrap_or(6).clamp(1, 10) as usize;
            let hits = self
                .0
                .search(q, limit)
                .await
                .map_err(|e| ToolError::Failed(e.to_string()))?;
            if hits.is_empty() {
                return Ok(ToolOutput {
                    meta: json!({ "query": q, "results": 0 }),
                    ..ToolOutput::text(format!("No results for \"{q}\"."))
                });
            }
            let mut decision = None;
            let hits = match &self.1 {
                Some(d) => {
                    let (ranked, dec) =
                        crate::decide::rank_hits(d, q, &hits, &|_: &str| false).await;
                    decision = dec;
                    ranked
                }
                None => hits,
            };
            let mut out = String::new();
            if let Some(d) = &decision {
                out.push_str(&format!(
                    "Ranked by the decision model ({}): most worth reading first.\n",
                    d.model
                ));
            }
            for (i, h) in hits.iter().enumerate() {
                out.push_str(&format!(
                    "{}. {} ({})\n   {}\n   {}\n",
                    i + 1,
                    h.title,
                    h.site,
                    h.url,
                    h.snippet
                ));
            }
            let mut meta = json!({ "query": q, "results": hits.len(),
                "hits": hits.iter().map(|h| json!({"url": h.url, "title": h.title, "site": h.site})).collect::<Vec<_>>() });
            if let Some(d) = &decision {
                meta["decision"] = serde_json::to_value(d).unwrap_or_default();
            }
            Ok(ToolOutput {
                content: out,
                evidence_refs: hits.iter().map(|h| h.url.clone()).collect(),
                meta,
            })
        })
    }
}

/// The app's one reader: every chat, quick ask, and the browser share its
/// robots files, per-host pacing, and page cache (on disk for a day under
/// the data folder's `cache/crawl`).
pub fn shared_reader() -> Fetcher {
    static READER: std::sync::OnceLock<Fetcher> = std::sync::OnceLock::new();
    READER
        .get_or_init(|| match tern_platform::AppPaths::discover() {
            Ok(paths) => Fetcher::new().with_cache_dir(paths.crawl_cache()),
            Err(_) => Fetcher::new(),
        })
        .clone()
}

#[derive(Debug, Clone)]
pub struct ReadUrlTool(pub Fetcher);

impl Default for ReadUrlTool {
    fn default() -> Self {
        Self(shared_reader())
    }
}

impl Tool for ReadUrlTool {
    fn name(&self) -> &'static str {
        "read_url"
    }
    fn summary(&self) -> &'static str {
        "Read one web page as text. Cite pages you have read, by their final URL."
    }
    fn effect(&self) -> EffectClass {
        EffectClass::Network
    }
    fn input_schema(&self) -> Value {
        json!({"type": "object", "required": ["url"], "additionalProperties": false,
               "properties": {"url": {"type": "string"}}})
    }
    fn target(&self, args: &Value) -> Option<String> {
        args["url"]
            .as_str()
            .and_then(|u| url::Url::parse(u).ok())
            .map(|u| tern_crawl::site_of(&u))
    }
    fn timeout(&self) -> Duration {
        Duration::from_secs(30)
    }
    fn idempotent(&self) -> bool {
        true
    }
    fn call<'a>(&'a self, args: Value) -> BoxFuture<'a, Result<ToolOutput, ToolError>> {
        Box::pin(async move {
            let u = args["url"].as_str().unwrap_or_default();
            let cache = if self.0.has_cached(u) { "hit" } else { "miss" };
            let p = self
                .0
                .fetch(u)
                .await
                .map_err(|e| ToolError::Failed(e.to_string()))?;
            let text: String = p.text.chars().take(READ_CHARS).collect();
            let page = PageRef {
                title: p.title.clone(),
                url: p.final_url.clone(),
                site: p.site.clone(),
                builtin: p.icon.builtin.map(str::to_owned),
                favicon: p.icon.favicon.clone(),
            };
            Ok(ToolOutput {
                content: format!("{}\n{}\n\n{}", p.title, p.final_url, text),
                evidence_refs: vec![p.final_url],
                meta: json!({
                    "page": page,
                    // How Tern Crawl read it, for the run report.
                    "crawl": {"reader": "Tern Crawl", "robots": "allowed", "cache": cache},
                }),
            })
        })
    }
}
