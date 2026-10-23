//! `tern ask`: a quick cited answer.
//!
//! One search, a few reads in parallel, one model call, then a citation
//! check. Every `[n]` in the answer must point at a page Tern actually read;
//! anything else is reported, never silently kept.

use std::collections::BTreeSet;
use std::sync::OnceLock;
use std::time::Instant;

use regex::Regex;
use serde::Serialize;
use serde_json::json;
use tern_core::{EffectClass, EventEnvelope, SessionId};
use tern_crawl::{Fetcher, SearchClient, SearchHit, SourceIcon, TERN_READER};
use tern_platform::AppPaths;
use tern_providers::{ChatMessage, ChatRequest, ProviderResult, UsageLedger};
use tern_store::{Harness, Store};

use crate::chat::{INBOX, Usage};
use crate::engine::{EventSink, Model, StoreSink};
use crate::providers::{self, SecretStore};
use crate::search;

/// Characters of each page given to the model.
pub const SOURCE_CHARS: usize = 6_000;

#[derive(Clone, Debug)]
pub struct AskOptions {
    pub provider: Option<String>,
    pub model: Option<String>,
    pub base_url: Option<String>,
    pub search: Option<String>,
    pub search_url: Option<String>,
    /// Search results to look at.
    pub results: usize,
    /// Pages to read from them.
    pub reads: usize,
    pub max_output_tokens: u32,
}

impl Default for AskOptions {
    fn default() -> Self {
        Self {
            provider: None,
            model: None,
            base_url: None,
            search: None,
            search_url: None,
            results: 8,
            reads: 4,
            max_output_tokens: 1200,
        }
    }
}

/// A source the answer may cite.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct Source {
    pub n: usize,
    pub title: String,
    pub url: String,
    pub site: String,
    pub icon: SourceIcon,
    pub cited: bool,
    pub truncated: bool,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct Unread {
    pub url: String,
    pub site: String,
    pub reason: String,
}

#[derive(Clone, Debug, Serialize)]
pub struct AskReport {
    pub question: String,
    pub answer: String,
    pub sources: Vec<Source>,
    pub unread: Vec<Unread>,
    /// Citation numbers in the answer that match no source.
    pub invalid_citations: Vec<usize>,
    pub search_service: &'static str,
    pub search_icon: &'static str,
    pub reader_icon: &'static str,
    pub provider: String,
    pub model: String,
    pub usage: Usage,
    pub elapsed_ms: u128,
}

/// Progress lines for the CLI and UIs.
pub trait AskProgress: Send + Sync {
    fn step(&self, line: &str);
}

impl<F: Fn(&str) + Send + Sync> AskProgress for F {
    fn step(&self, line: &str) {
        self(line)
    }
}

/// Everything one ask needs, given explicitly (tests build this by hand).
#[allow(missing_debug_implementations)]
pub struct AskDeps<'a> {
    pub session: SessionId,
    pub model: &'a dyn Model,
    pub model_name: &'a str,
    pub search: &'a SearchClient,
    pub fetcher: &'a Fetcher,
    pub sink: &'a dyn EventSink,
    pub ledger: &'a UsageLedger,
    /// Ranks the results before they are read; `None` keeps search order.
    pub decider: Option<&'a crate::decide::Decider>,
}

const SYSTEM: &str = "You are Tern, a careful research assistant. Answer the question using only the numbered sources provided. \
Put a citation like [1] or [2][3] after every sentence that states a fact, using only the source numbers given. \
If the sources do not answer the question, say so plainly instead of guessing. \
Be concise: at most about 200 words, plain prose, no headings, no list of sources at the end.";

fn cite_re() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| Regex::new(r"\[(\d{1,2}(?:\s*[,–-]\s*\d{1,2})*)\]").expect("static regex"))
}

/// Every citation number in `text`, in order of first use. `[2-4]` expands.
pub fn citations(text: &str) -> Vec<usize> {
    let mut seen = Vec::new();
    for cap in cite_re().captures_iter(text) {
        for part in cap[1].split(',') {
            let part = part.trim();
            let nums: Vec<usize> = part
                .split(['-', '–'])
                .filter_map(|p| p.trim().parse().ok())
                .collect();
            let range = match nums.as_slice() {
                [a, b] if a <= b && b - a < 20 => (*a..=*b).collect(),
                [a] => vec![*a],
                _ => vec![],
            };
            for n in range {
                if !seen.contains(&n) {
                    seen.push(n);
                }
            }
        }
    }
    seen
}

/// Picks up to `n` hits to read, preferring different sites.
fn pick(hits: &[SearchHit], n: usize) -> Vec<SearchHit> {
    let mut chosen: Vec<SearchHit> = Vec::new();
    let mut sites = BTreeSet::new();
    for h in hits {
        if chosen.len() < n && sites.insert(h.site.clone()) {
            chosen.push(h.clone());
        }
    }
    for h in hits {
        if chosen.len() < n && !chosen.iter().any(|c| c.url == h.url) {
            chosen.push(h.clone());
        }
    }
    chosen
}

fn record(sink: &dyn EventSink, ev: EventEnvelope) -> Result<(), String> {
    sink.record(ev).map(|_| ())
}

/// Runs one ask.
pub async fn run_ask(
    deps: AskDeps<'_>,
    question: &str,
    opts: &AskOptions,
    progress: &dyn AskProgress,
) -> Result<AskReport, String> {
    let started = Instant::now();
    let question = question.trim();
    if question.is_empty() {
        return Err(
            "Ask a question, for example: tern ask \"What did Vaswani et al. propose in 2017?\""
                .into(),
        );
    }
    let sid = deps.session;
    let service = deps.search.spec();
    record(
        deps.sink,
        EventEnvelope::new(
            sid,
            "ask.started",
            EffectClass::Analysis,
            "Started a quick ask",
            json!({"question": question}),
        ),
    )?;

    // 1. Search.
    let query: String = question.chars().take(380).collect();
    let hits = deps
        .search
        .search(&query, opts.results.max(1))
        .await
        .map_err(|e| {
            let _ = record(
                deps.sink,
                EventEnvelope::new(
                    sid,
                    "search.failed",
                    EffectClass::Network,
                    format!("Search on {} failed", service.name),
                    json!({"error": e.to_string()}),
                ),
            );
            e.to_string()
        })?;
    let line = format!("Searched {}: {} results", service.name, hits.len());
    progress.step(&line);
    record(
        deps.sink,
        EventEnvelope::new(
            sid,
            "search.done",
            EffectClass::Network,
            line.clone(),
            json!({"service": service.id, "icon": service.icon, "query": query, "hits": hits}),
        )
        .with_evidence(hits.iter().map(|h| h.url.clone())),
    )?;
    if hits.is_empty() {
        return Err(format!(
            "{} found nothing for that question. Try wording it differently.",
            service.name
        ));
    }

    // 2. The decision model ranks what to read first; then read in parallel.
    let hits = match deps.decider {
        Some(d) => {
            let (ranked, decision) =
                crate::decide::rank_hits(d, question, &hits, &|_: &str| false).await;
            if let Some(dec) = &decision {
                record(deps.sink, crate::decide::event(sid, None, dec))?;
            }
            ranked
        }
        None => hits,
    };
    let chosen = pick(&hits, opts.reads.max(1));
    let reads = futures::future::join_all(chosen.iter().map(|h| deps.fetcher.fetch(&h.url))).await;
    let mut pages = Vec::new();
    let mut unread = Vec::new();
    for (hit, r) in chosen.iter().zip(reads) {
        match r {
            Ok(p) => {
                let line = format!("Read {}", p.site);
                progress.step(&line);
                record(deps.sink, EventEnvelope::new(sid, "source.read", EffectClass::Network, line,
                    json!({"url": p.final_url, "title": p.title, "site": p.site, "icon": p.icon, "bytes": p.bytes, "truncated": p.truncated}))
                    .with_evidence([p.final_url.clone()]))?;
                pages.push(p);
            }
            Err(e) => {
                let reason = e.to_string();
                progress.step(&format!("Could not read {}: {reason}", hit.site));
                record(
                    deps.sink,
                    EventEnvelope::new(
                        sid,
                        "source.failed",
                        EffectClass::Network,
                        format!("Could not read {}", hit.site),
                        json!({"url": hit.url, "error": reason}),
                    ),
                )?;
                unread.push(Unread {
                    url: hit.url.clone(),
                    site: hit.site.clone(),
                    reason,
                });
            }
        }
    }
    if pages.is_empty() {
        return Err(format!(
            "Could not read any of the {} pages picked, so there is nothing to cite.",
            chosen.len()
        ));
    }

    // 3. One model call over the numbered sources.
    let mut prompt = format!("Question: {question}\n\nSources:\n");
    for (i, p) in pages.iter().enumerate() {
        let text: String = p.text.chars().take(SOURCE_CHARS).collect();
        prompt.push_str(&format!(
            "\n[{}] {}\n{}\n{}\n",
            i + 1,
            p.title,
            p.final_url,
            text
        ));
    }
    let mut req = ChatRequest::new(deps.model_name, vec![ChatMessage::user(prompt)]);
    req.system = Some(SYSTEM.to_owned());
    req.max_tokens = opts.max_output_tokens;
    req.temperature = Some(0.2);
    progress.step("Writing the answer");
    let resp: ProviderResult<_> = deps.model.complete(&req).await;
    let resp = resp.map_err(|e| {
        let _ = record(
            deps.sink,
            EventEnvelope::new(
                sid,
                "model.failed",
                EffectClass::Network,
                "The model call failed",
                json!({"error": e.to_string()}),
            ),
        );
        e.to_string()
    })?;
    deps.ledger
        .record(deps.model.provider_id(), deps.model_name, resp.usage);
    record(
        deps.sink,
        EventEnvelope::new(
            sid,
            "model.replied",
            EffectClass::Network,
            format!("{} replied", deps.model_name),
            json!({"chars": resp.text.len(), "finish_reason": resp.finish_reason}),
        ),
    )?;

    // 4. Check citations.
    let answer = resp.text.trim().to_owned();
    let cited = citations(&answer);
    let invalid: Vec<usize> = cited
        .iter()
        .copied()
        .filter(|n| *n == 0 || *n > pages.len())
        .collect();
    let sources: Vec<_> = pages
        .into_iter()
        .enumerate()
        .map(|(i, p)| Source {
            n: i + 1,
            cited: cited.contains(&(i + 1)),
            title: p.title,
            url: p.final_url,
            site: p.site,
            icon: p.icon,
            truncated: p.truncated,
        })
        .collect();
    let used = sources.iter().filter(|s| s.cited).count();
    let summary = if invalid.is_empty() {
        format!("Answered with {used} of {} sources cited", sources.len())
    } else {
        format!("Answered; {} citation(s) match no source", invalid.len())
    };
    record(
        deps.sink,
        EventEnvelope::new(
            sid,
            "ask.finished",
            EffectClass::Analysis,
            summary,
            json!({"cited": cited, "invalid": invalid, "sources": sources.len()}),
        )
        .with_evidence(sources.iter().filter(|s| s.cited).map(|s| s.url.clone())),
    )?;

    Ok(AskReport {
        question: question.to_owned(),
        answer,
        sources,
        unread,
        invalid_citations: invalid,
        search_service: service.name,
        search_icon: service.icon,
        reader_icon: TERN_READER,
        provider: deps.model.provider_id().to_owned(),
        model: deps.model_name.to_owned(),
        usage: deps.ledger.total(),
        elapsed_ms: started.elapsed().as_millis(),
    })
}

/// Opens the keychain, provider, search service, and database, then asks.
pub async fn ask(
    question: &str,
    opts: AskOptions,
    progress: &dyn AskProgress,
) -> Result<AskReport, String> {
    let keychain = providers::keychain();
    let secrets: Option<&dyn SecretStore> = keychain.as_ref().ok().map(|b| b.as_ref());
    let paths = AppPaths::discover().map_err(|e| e.to_string())?;
    paths.ensure().map_err(|e| e.to_string())?;
    let store = Store::open(&paths.database()).map_err(|e| e.to_string())?;
    ask_with(question, opts, secrets, store, progress).await
}

/// [`ask`] with the keychain and database given.
pub async fn ask_with(
    question: &str,
    opts: AskOptions,
    secrets: Option<&dyn SecretStore>,
    store: Store,
    progress: &dyn AskProgress,
) -> Result<AskReport, String> {
    let search = search::connect(opts.search.as_deref(), secrets, opts.search_url.as_deref())?;
    let pid = match &opts.provider {
        Some(p) => p.clone(),
        None => providers::list(secrets)
            .into_iter()
            .find(|r| r.key_saved)
            .map(|r| r.id.to_owned())
            .ok_or("No model provider has a key yet. Add one with `tern provider add <id>`, or use `--provider ollama`.")?,
    };
    let provider = providers::connect_by_id(&pid, secrets, opts.base_url.as_deref())
        .map_err(|e| e.to_string())?;
    let model = match &opts.model {
        Some(m) => m.clone(),
        None => provider
            .list_models()
            .await
            .map_err(|e| e.to_string())?
            .into_iter()
            .find(|m| m.can_chat() && !m.is_retired())
            .map(|m| m.id)
            .ok_or_else(|| {
                format!(
                    "{} lists no chat models. Pick one with `--model`.",
                    provider.spec().name
                )
            })?,
    };
    let project = store.ensure_project(INBOX).map_err(|e| e.to_string())?;
    let title: String = question.trim().chars().take(80).collect();
    let session = store
        .create_session(project, Harness::Research, &title)
        .map_err(|e| e.to_string())?;
    let sink = StoreSink::new(store);
    let ledger = UsageLedger::default();
    // Results are read from the open web, so the guard stays on even when
    // the search service itself runs on this machine.
    let fetcher = crate::web::shared_reader();
    let decider = crate::decide::decider_from(&crate::Preferences::load().decision, secrets);
    let deps = AskDeps {
        decider: Some(&decider),
        session,
        model: &provider,
        model_name: &model,
        search: &search,
        fetcher: &fetcher,
        sink: &sink,
        ledger: &ledger,
    };
    run_ask(deps, question, &opts, progress).await
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn citation_parsing() {
        assert_eq!(
            citations("A [1]. B [2][3]. C [1, 4]. D [5-7]. E [9–8]."),
            [1, 2, 3, 4, 5, 6, 7]
        );
        assert_eq!(citations("no cites, [a], [ 2 ]"), Vec::<usize>::new());
        assert_eq!(
            citations("a year like [2017] is not a citation"),
            Vec::<usize>::new()
        );
    }

    fn hit(url: &str, site: &str) -> SearchHit {
        SearchHit {
            title: url.into(),
            url: url.into(),
            snippet: String::new(),
            site: site.into(),
            icon: SourceIcon {
                builtin: None,
                favicon: None,
            },
        }
    }

    #[test]
    fn pick_prefers_different_sites_then_fills() {
        let hits = [
            hit("a1", "a"),
            hit("a2", "a"),
            hit("b1", "b"),
            hit("c1", "c"),
        ];
        let urls: Vec<_> = pick(&hits, 3).into_iter().map(|h| h.url).collect();
        assert_eq!(urls, ["a1", "b1", "c1"]);
        let urls: Vec<_> = pick(&hits, 4).into_iter().map(|h| h.url).collect();
        assert_eq!(urls, ["a1", "b1", "c1", "a2"]);
    }
}
