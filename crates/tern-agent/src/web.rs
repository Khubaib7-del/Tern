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
        "Search the web. Write a short keyword query of 2-6 words (names, products, versions: `SubQ Snap 1.1`, `LangChain founder`), not a whole sentence; call it again for each separate topic. Returns titles, links, and snippets; read a link before citing it."
    }
    fn effect(&self) -> EffectClass {
        EffectClass::Network
    }
    fn input_schema(&self) -> Value {
        json!({"type": "object", "required": ["query"], "additionalProperties": false,
               "properties": {
                   "query": {"type": "string", "description": "A short keyword query (2-6 words): the names, product and version, not the user's whole sentence."},
                   "limit": {"type": "integer"}}})
    }
    fn target(&self, _args: &Value) -> Option<String> {
        Some(self.0.spec().id.to_owned())
    }
    fn idempotent(&self) -> bool {
        true
    }
    fn call<'a>(&'a self, args: Value) -> BoxFuture<'a, Result<ToolOutput, ToolError>> {
        Box::pin(async move {
            let asked = args["query"].as_str().unwrap_or_default();
            let limit = args["limit"].as_u64().unwrap_or(6).clamp(1, 10) as usize;
            let service = self.0.spec().name;
            let wikipedia = self.0.spec().id == tern_crawl::KEYLESS_SEARCH;
            // Results that share none of the question's main terms are
            // dropped (`relevance`): a keyword index answers what it does
            // not know with pages matching one common word.
            let topic = crate::relevance::Topic::of(asked);
            let keep = |hits: Vec<tern_crawl::SearchHit>| {
                topic.filter(hits, |h| format!("{} {} {}", h.title, h.snippet, h.url))
            };
            // A keyword index (Wikipedia, SearXNG) finds nothing for a
            // whole sentence with typos; shorter keyword queries are tried
            // in turn before Tern says there was nothing.
            let mut tried = vec![asked.to_owned()];
            let first = self
                .0
                .search(asked, limit)
                .await
                .map_err(|e| ToolError::Failed(e.to_string()))?;
            // Pages are counted once however many ladder queries return
            // them: `found` is the distinct pages seen, `filtered` the
            // distinct ones dropped as off-topic.
            let mut seen: std::collections::BTreeSet<String> = std::collections::BTreeSet::new();
            let mut kept_urls: std::collections::BTreeSet<String> =
                std::collections::BTreeSet::new();
            seen.extend(first.iter().map(|h| h.url.clone()));
            let (mut hits, _) = keep(first);
            kept_urls.extend(hits.iter().map(|h| h.url.clone()));
            let mut q = asked.to_owned();
            if hits.is_empty() {
                for alt in crate::webintent::fallback_queries(asked) {
                    tried.push(alt.clone());
                    if let Ok(h) = self.0.search(&alt, limit).await {
                        seen.extend(h.iter().map(|h| h.url.clone()));
                        let (k, _) = keep(h);
                        kept_urls.extend(k.iter().map(|h| h.url.clone()));
                        if !k.is_empty() {
                            hits = k;
                            q = alt;
                            break;
                        }
                    }
                }
            }
            let found = seen.len();
            let filtered = seen.difference(&kept_urls).count();
            crate::diag::note(&format!(
                "web_search: service={service} query={asked:?} used={q:?} tried={} found={found} filtered={filtered} results={} terms={:?}",
                tried.len(),
                hits.len(),
                topic.key_terms()
            ));
            if hits.is_empty() {
                let also = if tried.len() > 1 {
                    format!(" (also tried: {})", tried[1..].join("; "))
                } else {
                    String::new()
                };
                let off = if filtered > 0 {
                    format!(" {filtered} result(s) were about something else and were dropped.")
                } else {
                    String::new()
                };
                let (text, notice) = if wikipedia {
                    (
                        format!(
                            "Wikipedia has nothing on \"{asked}\"{also}.{off} Wikipedia only holds encyclopedia articles, so new products, models and niche topics are often missing. Tell the user plainly that Wikipedia had nothing on this, answer only what you are sure of without inventing details, and say that a free Tavily key (1,000 searches a month) or a Brave key under Settings › Search & Crawlers gives full-web results. Do not claim you cannot reach the web."
                        ),
                        Some("wikipedia_empty"),
                    )
                } else {
                    (
                        format!(
                            "{service} found nothing on \"{asked}\"{also}.{off} Tell the user the search found nothing; do not claim you cannot reach the web."
                        ),
                        None,
                    )
                };
                return Ok(ToolOutput {
                    meta: json!({ "query": asked, "results": 0, "tried": tried, "service": service,
                                  "found": found, "filtered": filtered, "notice": notice }),
                    ..ToolOutput::text(text)
                });
            }
            let q = q.as_str();
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
            let mut out = format!(
                "Web search results for \"{q}\" from {service}, retrieved just now{}. These are live results from the web: answer from them and cite them as [n] with the links.\n",
                if q == asked {
                    String::new()
                } else {
                    format!(" (\"{asked}\" found nothing, so Tern searched the keywords)")
                }
            );
            if let Some(d) = &decision {
                out.push_str(&format!(
                    "Ranked by the decision model ({}): most worth reading first.\n",
                    d.model
                ));
            }
            for (i, h) in hits.iter().enumerate() {
                // A site's ad copy is not content to quote (#80).
                let snippet = if crate::webintent::is_boilerplate(&h.snippet) {
                    "(snippet left out: the site's own marketing text)"
                } else {
                    h.snippet.as_str()
                };
                out.push_str(&format!(
                    "{}. {} ({})\n   {}\n   {}\n",
                    i + 1,
                    h.title,
                    h.site,
                    h.url,
                    snippet
                ));
            }
            let mut meta = json!({ "query": q, "asked": asked, "tried": tried, "service": service, "results": hits.len(),
                "found": found, "filtered": filtered,
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
            // The reader chosen in Settings › Search & Crawler; hosted
            // readers fall back to Tern Crawl when they fail.
            let crawler = crate::crawlers::active();
            let keychain = crawler
                .hosted()
                .then(crate::providers::keychain)
                .and_then(Result::ok);
            let read = crate::crawlers::read(crawler, u, &self.0, keychain.as_deref())
                .await
                .map_err(|e| ToolError::Failed(e.to_string()))?;
            let reader = read.reader.name();
            let fell_back = read.fell_back.clone();
            let p = read.page;
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
                    "crawl": {
                        "reader": reader,
                        "robots": if reader == "Tern Crawl" { "allowed" } else { "not checked" },
                        "cache": cache,
                        "fell_back": fell_back,
                    },
                }),
            })
        })
    }
}
