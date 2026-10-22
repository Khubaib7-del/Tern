//! `tern research --deep`: a plan you approve, parallel workers, one
//! cited answer, then the claim checks and run report.

use std::io::{BufRead, IsTerminal, Write};

use tern_agent::deep::{DeepControl, MAX_WORKERS, PlanAnswer, PlanApprover, ResearchPlan};
use tern_agent::tools::BoxFuture;
use tern_agent::{ChatOptions, ChatSession, Preferences, RunReport};
use tern_core::EventEnvelope;

use crate::ChatArgs;

pub async fn run(
    question: String,
    deep: bool,
    chat: ChatArgs,
    workers: Option<usize>,
    yes: bool,
    json: bool,
) -> Result<(), String> {
    if !deep {
        return Err(
            "`tern research` runs deep research: add `--deep`. For a quick cited answer, use `tern ask`."
                .into(),
        );
    }
    let prefs = Preferences::load();
    let saved = prefs.chat_options();
    let opts = ChatOptions {
        provider: chat.provider.or(saved.provider),
        model: chat.model.or(saved.model),
        base_url: chat.base_url,
        search: chat.search.or(saved.search),
        search_url: chat.search_url,
        ..saved
    };
    let mut deep_opts = prefs.deep_options();
    if let Some(n) = workers {
        deep_opts.workers = n.clamp(1, MAX_WORKERS);
    }
    let mut session = ChatSession::open(opts).await?;
    let tty = std::io::stderr().is_terminal();
    let approver = CliPlan {
        ask: !yes && std::io::stdin().is_terminal() && tty,
        workers: deep_opts.workers,
    };
    let observer = move |e: &EventEnvelope| {
        if let Some(line) = progress_line(e) {
            if tty {
                eprintln!("\x1b[2m• {line}\x1b[0m");
            } else {
                eprintln!("• {line}");
            }
        }
    };
    let control = DeepControl::default();
    eprintln!(
        "Chat {} · if this stops, `tern resume {}` picks it up",
        session.session_id(),
        session.session_id()
    );
    let steer = session.steering();
    let steer_on = approver.ask;
    let started = std::sync::Once::new();
    let observer = move |e: &EventEnvelope| {
        // Steering input opens once the plan is answered, so it never
        // races the plan prompt for the terminal.
        if steer_on && e.kind == "plan.answered" {
            let steer = steer.clone();
            started.call_once(move || {
                read_steering(
                    steer,
                    "Type a note and press Enter to steer (focus on…, drop 3, add: …, redirect 2: …).",
                )
            });
        }
        observer(e);
    };
    let out = session
        .deep_research(&question, &deep_opts, &approver, &control, &observer, None)
        .await;
    let Some(answer) = out.answer.clone() else {
        return Err(out.reason.unwrap_or_else(|| "The run stopped.".into()));
    };
    let files = session.check_last_turn(&observer).await;
    let report = files
        .as_ref()
        .and_then(|f| std::fs::read_to_string(&f.json).ok())
        .and_then(|t| RunReport::from_json(&t).ok());
    if json {
        match &report {
            Some(r) => println!("{}", r.to_json()),
            None => return Err("The run report could not be written.".into()),
        }
        return Ok(());
    }
    println!();
    println!("{answer}");
    println!();
    let mut tail = Vec::new();
    if let Some(r) = &report {
        let done = r
            .workers
            .iter()
            .filter(|w| w.status == tern_agent::WorkerStatus::Done)
            .count();
        tail.push(format!("{done} of {} workers finished", r.workers.len()));
        if !r.claims.is_empty() {
            tail.push(r.tally().line());
        }
    }
    let used = session.usage();
    tail.push(format!("{} · {} tokens", session.model(), used.total()));
    println!("{}", tail.join(" · "));
    if let Some(f) = &files {
        eprintln!("Report: {}", f.markdown.display());
    }
    Ok(())
}

/// Reads steering notes from the terminal on a thread of its own, one per
/// line, until input ends. Does nothing when stdin is not a terminal.
pub fn read_steering(steer: tern_agent::Steering, hint: &str) {
    if !std::io::stdin().is_terminal() {
        return;
    }
    eprintln!("{hint}");
    std::thread::spawn(move || {
        let stdin = std::io::stdin();
        while let Some(line) = read_line(&stdin) {
            if steer.send(&line) {
                eprintln!(
                    "• Steer queued: {}",
                    tern_agent::steer::parse_steer(line.trim()).describe()
                );
            }
        }
    });
}

/// Shows the plan in the terminal and asks what to do with it.
struct CliPlan {
    ask: bool,
    workers: usize,
}

impl PlanApprover for CliPlan {
    fn review<'a>(&'a self, plan: &'a ResearchPlan) -> BoxFuture<'a, PlanAnswer> {
        let plan = plan.clone();
        let (ask, workers) = (self.ask, self.workers);
        Box::pin(async move {
            tokio::task::spawn_blocking(move || {
                eprint!("{}", describe(&plan, workers));
                if !ask {
                    return PlanAnswer::Run(plan);
                }
                let stdin = std::io::stdin();
                loop {
                    eprint!("[Enter] run · e edit · s skip the plan · c cancel: ");
                    let _ = std::io::stderr().flush();
                    let Some(line) = read_line(&stdin) else {
                        return PlanAnswer::Cancel;
                    };
                    match line.trim().to_lowercase().as_str() {
                        "" | "r" | "run" | "y" | "yes" => return PlanAnswer::Run(plan),
                        "s" | "skip" => return PlanAnswer::Skip,
                        "c" | "cancel" | "n" | "no" | "q" => return PlanAnswer::Cancel,
                        "e" | "edit" => {
                            let edited = edit(&plan, &stdin);
                            eprint!("{}", describe(&edited, workers));
                            return PlanAnswer::Run(edited);
                        }
                        _ => continue,
                    }
                }
            })
            .await
            .unwrap_or(PlanAnswer::Cancel)
        })
    }
}

fn read_line(stdin: &std::io::Stdin) -> Option<String> {
    let mut line = String::new();
    match stdin.lock().read_line(&mut line) {
        Ok(0) | Err(_) => None,
        Ok(_) => Some(line),
    }
}

/// Asks for each sub-question: Enter keeps it, `-` drops it, new text
/// replaces it (and its searches).
fn edit(plan: &ResearchPlan, stdin: &std::io::Stdin) -> ResearchPlan {
    let mut out = plan.clone();
    out.sub_questions.clear();
    for s in &plan.sub_questions {
        eprint!(
            "  {}. {}\n     new text, Enter to keep, - to drop: ",
            s.n, s.question
        );
        let _ = std::io::stderr().flush();
        let line = read_line(stdin).unwrap_or_default();
        let t = line.trim();
        match t {
            "" => out.sub_questions.push(s.clone()),
            "-" => {}
            text => {
                let mut s = s.clone();
                s.question = text.to_owned();
                s.queries = vec![text.to_owned()];
                s.intent.clear();
                out.sub_questions.push(s);
            }
        }
    }
    out.normalise(tern_agent::deep::MAX_SUB_QUESTIONS);
    out
}

/// The plan as the terminal shows it.
pub fn describe(plan: &ResearchPlan, workers: usize) -> String {
    let (low, high) = plan.estimate();
    let by = match plan.source {
        tern_agent::deep::PlanSource::Model => "drafted by the model",
        tern_agent::deep::PlanSource::Fallback => "Tern's own split",
        tern_agent::deep::PlanSource::Skipped => "one worker",
    };
    let mut out = format!(
        "\nPlan ({by}) · about {}–{} tokens · up to {} pages · {} s · {} worker{} at once\n",
        tern_agent::run::compact(low),
        tern_agent::run::compact(high),
        plan.budget.pages,
        plan.budget.seconds,
        workers,
        if workers == 1 { "" } else { "s" }
    );
    for s in &plan.sub_questions {
        out.push_str(&format!(" {}. {}\n", s.n, s.question));
        if !s.intent.is_empty() {
            out.push_str(&format!("    looks for: {}\n", s.intent));
        }
        let kinds: Vec<&str> = s.sources.iter().map(|k| k.label()).collect();
        out.push_str(&format!(
            "    searches: {} · sources: {} · {}%\n",
            s.queries.join("; "),
            kinds.join(", "),
            s.share
        ));
    }
    out
}

/// The progress line an event earns, if any.
pub fn progress_line(e: &EventEnvelope) -> Option<String> {
    match e.kind.as_str() {
        "plan.drafted" => Some(match e.payload["note"].as_str() {
            Some(note) => format!("{} · {note}", e.user_summary),
            None => e.user_summary.clone(),
        }),
        "plan.answered" | "worker.started" | "worker.finished" | "claims.checking"
        | "claims.checked" | "steer.received" | "steer.applied" | "run.resumed" => {
            Some(e.user_summary.clone())
        }
        "tool.executed" | "tool.failed" if e.payload.get("worker").is_some() => {
            Some(e.user_summary.clone())
        }
        "model.replied" if e.payload["purpose"] == "synthesis" => {
            Some("Wrote the answer from the workers' notes".into())
        }
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use tern_agent::deep::{PlanBudget, fallback_plan};
    use tern_core::{EffectClass, SessionId};

    #[test]
    fn plans_read_plainly() {
        let p = fallback_plan("Compare Rust vs Go", PlanBudget::default(), 5);
        let text = describe(&p, 3);
        assert!(text.contains("Plan (Tern's own split)"));
        assert!(text.contains("3 workers at once"));
        assert!(text.contains(" 1. Rust\n"));
    }

    #[test]
    fn only_worker_progress_is_shown() {
        let s = SessionId::new();
        let ev = |kind: &str, payload: serde_json::Value| {
            EventEnvelope::new(s, kind, EffectClass::Analysis, "line", payload)
        };
        assert!(progress_line(&ev("worker.started", json!({"worker": 1}))).is_some());
        assert!(progress_line(&ev("tool.executed", json!({"worker": 1}))).is_some());
        assert!(progress_line(&ev("tool.executed", json!({}))).is_none());
        assert!(progress_line(&ev("model.replied", json!({"purpose": "notes"}))).is_none());
        assert!(progress_line(&ev("model.replied", json!({"purpose": "synthesis"}))).is_some());
    }
}
