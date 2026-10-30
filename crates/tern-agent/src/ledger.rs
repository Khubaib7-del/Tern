//! A run's source and budget ledgers, rebuilt from its events.
//!
//! Every URL a search returned or a worker picked, and what became of it
//! (read live, read from the cache, skipped for robots.txt, the budget, a
//! duplicate, or the policy gate, or failed), and what each step and each
//! worker used. The types live in [`tern_research::ledger`]; this module
//! feeds them from a [`Run`], so any past turn's ledgers can be rebuilt.

use serde_json::Value;

pub use tern_research::ledger::{
    BudgetEntry, LaneTotal, Ledger, LedgerBuilder, LedgerReason, SourceCounts, SourceEntry,
    SourceFate, StepUse,
};

use crate::run::Run;
use crate::sources::PageRef;

/// The ledgers of one run.
pub fn of_run(run: &Run) -> Ledger {
    let mut b = LedgerBuilder::new();
    let mut took: Vec<(usize, u64)> = Vec::new();
    for s in &run.steps {
        let p = &s.payload;
        let at = s.at.as_millis() as u64;
        let worker = p["worker"]
            .as_u64()
            .or_else(|| p["meta"]["worker"].as_u64())
            .map(|n| n as usize);
        let budget = serde_json::from_value(p["budget"].clone()).ok();
        let question = p["question"].as_str().unwrap_or_default();
        match s.event.as_str() {
            "worker.started" => {
                if let Some(n) = worker {
                    b.worker(n, question, budget, at);
                    if p.get("restored").is_some() {
                        restore(&mut b, &p["restored"], n, at);
                    }
                }
            }
            "worker.finished" => {
                if let Some(n) = worker {
                    took.push((n, p["took_ms"].as_u64().unwrap_or_default()));
                    if p["restored"] == true {
                        b.worker(n, question, budget, 0);
                        restore(&mut b, p, n, 0);
                    }
                }
            }
            "tool.executed" => match p["tool"].as_str() {
                Some("web_search") => {
                    let query = p["args"]["query"].as_str().unwrap_or_default();
                    for h in p["meta"]["hits"].as_array().into_iter().flatten() {
                        b.considered(
                            str_of(h, "url"),
                            str_of(h, "site"),
                            str_of(h, "title"),
                            worker,
                            query,
                            at,
                        );
                    }
                    b.step(query, StepUse::Search, worker, (0, 0), 0, 1, at);
                }
                Some("scholar_search" | "paper_snowball") => {
                    let query = p["meta"]["query"].as_str().unwrap_or_default();
                    for row in p["meta"]["papers"].as_array().into_iter().flatten() {
                        let Ok(paper) =
                            serde_json::from_value::<tern_research::scholar::Paper>(row.clone())
                        else {
                            continue;
                        };
                        let url = str_of(row, "link");
                        let site = url::Url::parse(url)
                            .map(|u| tern_crawl::site_of(&u))
                            .unwrap_or_default();
                        b.considered(url, &site, &paper.title, worker, query, at);
                        b.paper(url, worker, paper);
                    }
                    b.step(query, StepUse::Search, worker, (0, 0), 0, 1, at);
                }
                Some("paper_read") => {
                    if let Some(page) = &s.page {
                        let hit = p["meta"]["cached"] == true;
                        match serde_json::from_value::<tern_research::scholar::Paper>(
                            p["meta"]["paper"].clone(),
                        ) {
                            Ok(paper) => b.read_paper(
                                &page.url,
                                &page.site,
                                &page.title,
                                worker,
                                hit,
                                at,
                                paper,
                            ),
                            Err(_) => b.read(&page.url, &page.site, &page.title, worker, hit, at),
                        }
                        b.step(&page.url, StepUse::Read, worker, (0, 0), 1, 0, at);
                    }
                }
                Some("read_url") => {
                    if let Some(page) = &s.page {
                        let hit = p["meta"]["crawl"]["cache"] == "hit";
                        b.read(&page.url, &page.site, &page.title, worker, hit, at);
                        b.step(&page.url, StepUse::Read, worker, (0, 0), 1, 0, at);
                    }
                }
                _ => {}
            },
            "tool.failed" if p["tool"] == "read_url" => {
                let url = p["args"]["url"].as_str().unwrap_or_default();
                let error = p["error"].as_str().unwrap_or(&s.note);
                b.failed(url, worker, error, at);
            }
            "sources.skipped" => {
                for h in p["skipped"].as_array().into_iter().flatten() {
                    let reason: LedgerReason =
                        serde_json::from_value(h["reason"].clone()).unwrap_or_default();
                    b.skipped(
                        str_of(h, "url"),
                        str_of(h, "site"),
                        str_of(h, "title"),
                        worker,
                        reason,
                        at,
                    );
                }
            }
            "model.replied" => {
                let label = match p["purpose"].as_str() {
                    Some("plan") => "plan",
                    Some("notes") => "notes",
                    Some("synthesis") => "answer",
                    _ => "model reply",
                };
                b.step(
                    label,
                    StepUse::Model,
                    worker,
                    s.tokens.unwrap_or_default(),
                    0,
                    0,
                    at,
                );
            }
            _ => {}
        }
    }
    let run_ms = run
        .steps
        .last()
        .map(|s| s.at.as_millis() as u64)
        .unwrap_or(0);
    b.finish(run_ms, &took)
}

/// A worker's work kept from a checkpoint: its pages and what it used.
fn restore(b: &mut LedgerBuilder, p: &Value, n: usize, at: u64) {
    for page in p["pages"].as_array().into_iter().flatten() {
        if let Ok(pg) = serde_json::from_value::<PageRef>(page.clone()) {
            b.restored(&pg.url, &pg.site, &pg.title, Some(n));
        }
    }
    let pages = p["pages"].as_array().map(Vec::len).unwrap_or_default() as u64;
    b.step(
        "kept from the checkpoint",
        StepUse::Worker,
        Some(n),
        (
            p["tokens"]["input"].as_u64().unwrap_or_default(),
            p["tokens"]["output"].as_u64().unwrap_or_default(),
        ),
        pages,
        p["searches"].as_u64().unwrap_or_default(),
        at,
    );
}

fn str_of<'a>(v: &'a Value, key: &str) -> &'a str {
    v[key].as_str().unwrap_or_default()
}
