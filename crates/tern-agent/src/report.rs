//! Run reports: one per answered turn, written as Markdown and JSON under
//! the data folder's `runs/<chat>/turn-<n>.md|json`.
//!
//! The report is built from the chat's event log alone (the run, the pages
//! read and how Tern Crawl read them, the claim verdicts), so `tern report`
//! can rebuild it for any past turn.

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use tern_core::{EventEnvelope, SessionId};
use tern_platform::AppPaths;
pub use tern_research::report::{
    REPORT_VERSION, ReportBudget, ReportSource, ReportStep, ReportUsage, ReportWorker, RunReport,
};
use tern_store::Store;

use crate::engine::Budget;
use crate::run::{RunEnd, StepKind};
use crate::transcript::{PastTurn, past_turns};
use crate::verify;

/// Where a run's report was written.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ReportFiles {
    pub markdown: PathBuf,
    pub json: PathBuf,
}

/// The folder a chat's reports go in.
pub fn runs_dir(paths: &AppPaths, session: SessionId) -> PathBuf {
    paths.data_dir.join("runs").join(session.to_string())
}

/// The report of turn `turn` (1-based) of a chat, from its event log.
/// `None` picks the latest turn with an answer.
pub fn build(
    session: SessionId,
    events: &[EventEnvelope],
    turn: Option<usize>,
    budget: Option<Budget>,
) -> Result<RunReport, String> {
    let turns = past_turns(events);
    let (n, t) = match turn {
        Some(n) => (
            n,
            turns.get(n.wrapping_sub(1)).ok_or_else(|| {
                format!("This chat has {} turns; there is no turn {n}.", turns.len())
            })?,
        ),
        None => turns
            .iter()
            .enumerate()
            .rev()
            .find(|(_, t)| t.answer.is_some())
            .map(|(i, t)| (i + 1, t))
            .ok_or("This chat has no answered turn to report on yet.")?,
    };
    let provider = provider_before(events, t);
    Ok(from_turn(session, n, t, &provider, budget))
}

/// The provider the chat had open when the turn ran.
fn provider_before(events: &[EventEnvelope], t: &PastTurn) -> String {
    let first = t.run.steps.first().map(|s| s.seq).unwrap_or(u64::MAX);
    events
        .iter()
        .filter(|e| {
            e.seq < first && matches!(e.kind.as_str(), "session.opened" | "session.resumed")
        })
        .filter_map(|e| e.payload["provider"].as_str())
        .next_back()
        .unwrap_or_default()
        .to_owned()
}

/// The report of one past turn.
pub fn from_turn(
    session: SessionId,
    n: usize,
    t: &PastTurn,
    provider: &str,
    budget: Option<Budget>,
) -> RunReport {
    let run = &t.run;
    let question = t
        .message
        .as_deref()
        .map(|m| {
            let (text, _) = crate::attach::split(m);
            crate::reply::split(text).1.trim().to_owned()
        })
        .unwrap_or_default();
    let answer = t.answer.clone().unwrap_or_default();
    let model = run
        .steps
        .iter()
        .find(|s| s.event == "turn.started")
        .map(|s| s.target.clone())
        .unwrap_or_default();

    let steps = run
        .detailed()
        .map(|s| ReportStep {
            at_ms: s.at.as_millis() as u64,
            verb: s.verb.to_owned(),
            target: s.target.clone(),
            note: s.note.clone(),
            ok: s.ok,
        })
        .collect();

    // Sources: pages read (numbered as the answer cites them) and reads
    // that failed.
    let numbered = verify::numbered(&answer, &t.pages);
    let cited = tern_research::claims::citations(&answer);
    let mut sources: Vec<ReportSource> = Vec::new();
    for s in &run.steps {
        match (s.kind, s.event.as_str()) {
            (StepKind::Read, "tool.executed") => {
                let Some(p) = &s.page else { continue };
                if sources.iter().any(|x| x.url == p.url) {
                    continue;
                }
                let crawl = &s.payload["meta"]["crawl"];
                let n = numbered
                    .iter()
                    .find(|(_, q)| q.url == p.url)
                    .map(|(n, _)| *n)
                    .unwrap_or(0);
                sources.push(ReportSource {
                    n,
                    title: p.title.clone(),
                    url: p.url.clone(),
                    site: p.site.clone(),
                    reader: crawl["reader"].as_str().unwrap_or("Tern Crawl").to_owned(),
                    robots: crawl["robots"].as_str().unwrap_or("allowed").to_owned(),
                    cache: crawl["cache"].as_str().unwrap_or("unknown").to_owned(),
                    cited: numbered
                        .iter()
                        .any(|(m, q)| q.url == p.url && cited.contains(m)),
                    error: None,
                });
            }
            (StepKind::Problem, "tool.failed") if s.payload["tool"] == "read_url" => {
                let url = s.target.clone();
                let error = s.note.clone();
                let site = url::Url::parse(&url)
                    .map(|u| tern_crawl::site_of(&u))
                    .unwrap_or_default();
                sources.push(ReportSource {
                    n: 0,
                    title: String::new(),
                    robots: if error.contains("robots.txt") {
                        "blocked".into()
                    } else {
                        "not checked".into()
                    },
                    url,
                    site,
                    reader: "Tern Crawl".into(),
                    cache: "unknown".into(),
                    cited: false,
                    error: Some(error),
                });
            }
            _ => {}
        }
    }

    // A deep turn is measured against its plan's budget, not the chat's.
    let plan = run.deep.as_ref().and_then(|d| d.plan.as_ref());
    let budget = plan.map(crate::deep::turn_budget).or(budget);
    let workers: Vec<ReportWorker> = run
        .deep
        .as_ref()
        .map(|d| {
            d.workers
                .iter()
                .map(|w| ReportWorker {
                    n: w.n,
                    question: w.question.clone(),
                    status: w.status,
                    note: w.note.clone(),
                    searches: w.searches as u64,
                    pages: w.pages.len() as u64,
                    urls: w.pages.iter().map(|p| p.url.clone()).collect(),
                    tokens_in: w.tokens_in,
                    tokens_out: w.tokens_out,
                    took_ms: w.took.as_millis() as u64,
                    budget: w.budget,
                })
                .collect()
        })
        .unwrap_or_default();

    let summary = run.summary();
    let finished = run.steps.iter().rev().find(|s| {
        matches!(
            s.event.as_str(),
            "turn.finished" | "run.cancelled" | "budget.exceeded"
        )
    });
    let counter = |key: &str| {
        finished
            .and_then(|s| s.payload[key].as_u64())
            .unwrap_or_default()
    };
    let checked = run.steps.iter().rev().find(|s| s.event == "claims.checked");
    let judge_tokens = |key: &str| {
        checked
            .and_then(|s| s.payload["judge_tokens"][key].as_u64())
            .unwrap_or_default()
    };
    let usage = ReportUsage {
        model_calls: counter("model_calls").max(summary.model_calls as u64),
        tool_calls: counter("tool_calls"),
        searches: summary.searches as u64,
        pages: summary.pages as u64,
        tokens_in: summary.tokens_in,
        tokens_out: summary.tokens_out,
        took_ms: t.took.as_millis() as u64,
        judge_tokens_in: judge_tokens("input"),
        judge_tokens_out: judge_tokens("output"),
        budget: budget.map(|b| ReportBudget {
            max_model_calls: b.max_model_calls.into(),
            max_tool_calls: b.max_tool_calls.into(),
            max_tokens: b.max_tokens,
        }),
    };

    let mut errors: Vec<String> = run
        .problems()
        .filter(|s| s.event != "turn.finished")
        .map(|s| {
            let mut line = format!("{} {}", s.verb, s.target);
            if !s.note.is_empty() {
                line.push_str(&format!(": {}", s.note));
            }
            line.trim().to_owned()
        })
        .collect();
    if let Some(e) = checked.and_then(|s| s.payload["judge_error"].as_str()) {
        errors.push(format!("The model judge was not used: {e}"));
    }

    RunReport {
        version: REPORT_VERSION,
        session: session.to_string(),
        turn: n,
        started: run
            .started()
            .and_then(|t| {
                t.format(&time::format_description::well_known::Rfc3339)
                    .ok()
            })
            .unwrap_or_default(),
        question,
        answer,
        outcome: match (&run.end, &t.stop) {
            (_, Some(why)) => why.clone(),
            (Some(RunEnd::Answered), None) => "Answered".into(),
            (Some(RunEnd::Stopped(why)), None) => why.clone(),
            (None, None) => "Did not finish".into(),
        },
        provider: provider.to_owned(),
        model,
        plan: plan.map(|p| p.lines()).unwrap_or_default(),
        steps,
        sources,
        workers,
        claims: run.checks.clone(),
        usage,
        errors,
        ledger: run.ledger(),
        steers: run
            .steers
            .iter()
            .map(|s| {
                if s.outcome.is_empty() {
                    format!("“{}” (not applied before the run ended)", s.text)
                } else {
                    format!("“{}” → {}", s.text, s.outcome)
                }
            })
            .collect(),
        resumed_from: run.resumed.as_ref().map(|r| {
            let mut line = r.progress.clone();
            if !r.saved_at.is_empty() {
                line.push_str(&format!(", saved {}", r.saved_at));
            }
            if r.prior_tokens > 0 {
                line.push_str(&format!(
                    "; {} tokens were used before the stop and are not counted again here",
                    r.prior_tokens
                ));
            }
            line
        }),
    }
}

/// Writes the report's Markdown and JSON into `dir`.
pub fn write(dir: &Path, report: &RunReport) -> Result<ReportFiles, String> {
    std::fs::create_dir_all(dir)
        .map_err(|e| format!("Tern could not make {}: {e}", dir.display()))?;
    let stem = format!("turn-{}", report.turn);
    let files = ReportFiles {
        markdown: dir.join(format!("{stem}.md")),
        json: dir.join(format!("{stem}.json")),
    };
    for (path, text) in [
        (&files.markdown, report.to_markdown()),
        (&files.json, report.to_json()),
    ] {
        std::fs::write(path, text)
            .map_err(|e| format!("Tern could not write {}: {e}", path.display()))?;
    }
    Ok(files)
}

/// The chat `which` names: a session id, a unique prefix of one, or
/// `last` (the most recent chat).
pub fn find_session(store: &Store, which: &str) -> Result<SessionId, String> {
    let which = which.trim();
    let recent = store.recent_sessions(500).map_err(|e| e.to_string())?;
    if which.is_empty() || which.eq_ignore_ascii_case("last") {
        return recent
            .first()
            .map(|s| s.id)
            .ok_or_else(|| "There are no chats yet.".to_owned());
    }
    if let Ok(id) = which.parse::<SessionId>() {
        return Ok(id);
    }
    let lower = which.to_lowercase();
    let hits: Vec<_> = recent
        .iter()
        .filter(|s| {
            let full = s.id.to_string().to_lowercase();
            let bare = full
                .strip_prefix(&SessionId::PREFIX.to_lowercase())
                .unwrap_or(&full)
                .to_owned();
            full.starts_with(&lower) || bare.starts_with(&lower)
        })
        .collect();
    match hits.as_slice() {
        [one] => Ok(one.id),
        [] => Err(format!(
            "No chat matches `{which}`. `tern report --list` shows recent chats."
        )),
        _ => Err(format!(
            "`{which}` matches {} chats; give more of the id.",
            hits.len()
        )),
    }
}

/// The chat `which` names (`last`, an id, or the start of one), from the
/// app's database.
pub fn find_chat(which: &str) -> Result<SessionId, String> {
    let paths = AppPaths::discover().map_err(|e| e.to_string())?;
    let db = paths.database();
    if !db.exists() {
        return Err("There are no chats yet.".into());
    }
    let store = Store::open(&db).map_err(|e| e.to_string())?;
    find_session(&store, which)
}

/// `tern report`: the report for a chat's turn, rebuilt from the log and
/// written next to its earlier copy. A turn whose claims were never
/// checked (a TUI chat, or one from before checks) is checked now by word
/// overlap against Tern Crawl's copies of its pages.
pub async fn report_for(
    which: &str,
    turn: Option<usize>,
) -> Result<(RunReport, ReportFiles), String> {
    let paths = AppPaths::discover().map_err(|e| e.to_string())?;
    let db = paths.database();
    if !db.exists() {
        return Err("There are no chats yet.".into());
    }
    let store = Store::open(&db).map_err(|e| e.to_string())?;
    let session = find_session(&store, which)?;
    let events = store.replay(session, 0).map_err(|e| e.to_string())?;
    if events.is_empty() {
        return Err(format!("Chat {session} has no logged events."));
    }
    let mut report = build(session, &events, turn, None)?;
    let turns = past_turns(&events);
    let past = &turns[report.turn - 1];
    let checked_before = past.run.steps.iter().any(|s| s.event == "claims.checked");
    if !checked_before && verify::has_claims(&report.answer) {
        let checked = verify::check_answer(
            &report.answer,
            &past.pages,
            &crate::web::shared_reader(),
            None,
        )
        .await;
        report.claims = checked.checks;
    }
    let files = write(&runs_dir(&paths, session), &report)?;
    Ok((report, files))
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use tern_core::{EffectClass, Measure, UsageDelta};

    fn ev(s: SessionId, kind: &str, summary: &str, payload: serde_json::Value) -> EventEnvelope {
        EventEnvelope::new(s, kind, EffectClass::Analysis, summary, payload)
    }

    fn log(s: SessionId) -> Vec<EventEnvelope> {
        let page =
            json!({"title": "Eiffel Tower", "url": "https://e.test/tower", "site": "e.test"});
        let mut reply = ev(s, "model.replied", "m replied", json!({"model": "m"}));
        reply.usage_delta = Some(UsageDelta {
            input_tokens: 900,
            output_tokens: 100,
            cost_micros: None,
            currency: "USD".into(),
            measure: Measure::Measured,
        });
        let checks = json!([{
            "claim": 1, "text": "It was finished in 1889.", "citations": [1],
            "verdict": "supported", "overlap": 80, "method": "lexical", "note": "",
            "evidence": {"source": 1, "url": "https://e.test/tower", "title": "Eiffel Tower",
                "site": "e.test", "quote": "completed in 1889", "anchor": "https://e.test/tower#:~:text=completed", "page": null}
        }]);
        let mut events = vec![
            ev(
                s,
                "session.opened",
                "Opened",
                json!({"provider": "groq", "model": "m"}),
            ),
            ev(
                s,
                "turn.started",
                "Started",
                json!({"model": "m", "message": "When was it finished?"}),
            ),
            reply,
            ev(
                s,
                "tool.executed",
                "Ran `read_url`",
                json!({"tool": "read_url", "args": {"url": "https://e.test/tower"},
                "meta": {"page": page, "crawl": {"reader": "Tern Crawl", "robots": "allowed", "cache": "miss"}}}),
            ),
            ev(
                s,
                "tool.failed",
                "`read_url` failed",
                json!({"tool": "read_url", "args": {"url": "https://x.test/p"},
                "error": "x.test's robots.txt asks automated readers not to open this page"}),
            ),
            ev(
                s,
                "turn.finished",
                "Answered",
                json!({"state": "completed", "answer": "It was finished in 1889 [1].",
                "model_calls": 2, "tool_calls": 2}),
            ),
            ev(
                s,
                "claims.checked",
                "Checked 1 claim",
                json!({"checks": checks,
                "report": {"markdown": "/r/turn-1.md", "json": "/r/turn-1.json"}}),
            ),
        ];
        for (i, e) in events.iter_mut().enumerate() {
            e.seq = i as u64 + 1;
        }
        events
    }

    #[test]
    fn builds_from_the_log() {
        let s = SessionId::new();
        let r = build(s, &log(s), None, Some(Budget::default())).unwrap();
        assert_eq!(r.turn, 1);
        assert_eq!(r.question, "When was it finished?");
        assert_eq!((r.provider.as_str(), r.model.as_str()), ("groq", "m"));
        assert_eq!(r.outcome, "Answered");
        assert_eq!(r.sources.len(), 2);
        let s0 = &r.sources[0];
        assert_eq!((s0.n, s0.cited, s0.cache.as_str()), (1, true, "miss"));
        assert_eq!(r.sources[1].robots, "blocked");
        assert_eq!(r.claims.len(), 1);
        assert_eq!(r.usage.model_calls, 2);
        assert_eq!(r.usage.tokens_in, 900);
        assert_eq!(r.usage.budget.unwrap().max_tool_calls, 12);
        assert_eq!(r.errors.len(), 1);
        assert!(r.steps.iter().any(|x| x.verb == "Checked"));
        assert!(build(s, &log(s), Some(3), None).is_err());
    }

    #[test]
    fn writes_both_files() {
        let s = SessionId::new();
        let r = build(s, &log(s), Some(1), None).unwrap();
        let dir = tempfile::tempdir().unwrap();
        let files = write(dir.path(), &r).unwrap();
        let md = std::fs::read_to_string(&files.markdown).unwrap();
        assert!(md.contains("✓ Supported"));
        let back = RunReport::from_json(&std::fs::read_to_string(&files.json).unwrap()).unwrap();
        assert_eq!(back, r);
        assert!(files.markdown.ends_with("turn-1.md"));
    }

    #[test]
    fn finds_chats_by_prefix() {
        let mut store = Store::open_in_memory().unwrap();
        assert!(find_session(&store, "last").is_err());
        let p = store.ensure_project("Inbox").unwrap();
        let id = store
            .create_session(p, tern_store::Harness::Research, "Chat")
            .unwrap();
        store
            .append(&ev(id, "session.opened", "Opened", json!({})))
            .unwrap();
        // A chat that was only opened is not listed; one with a turn is.
        assert!(find_session(&store, "last").is_err());
        store
            .append(&ev(id, "turn.started", "Asked", json!({})))
            .unwrap();
        assert_eq!(find_session(&store, "last").unwrap(), id);
        let prefix: String = id.to_string().chars().take(10).collect();
        assert_eq!(find_session(&store, &prefix).unwrap(), id);
        let bare: String = id.to_string()[SessionId::PREFIX.len()..]
            .chars()
            .take(6)
            .collect();
        assert_eq!(find_session(&store, &bare).unwrap(), id);
        assert!(find_session(&store, "zzzz").is_err());
    }
}
