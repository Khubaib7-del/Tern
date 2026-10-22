//! One turn's events, shaped for the run panel's three views.
//!
//! | View | Shows |
//! | --- | --- |
//! | Summary | One rollup line, the pages read, and the problems |
//! | Detailed | Each step as one line, verb first, with its target |
//! | Raw | Every event as logged: type, time, summary, and payload |
//!
//! All three read the same [`Run`]; only presentation changes
//! (`docs/02-ui-definition.md` §3). Front ends build a `Run` live from the
//! events the turn loop emits, or from the log when a chat is reopened.

use std::time::Duration;

use serde_json::Value;
use tern_core::EventEnvelope;
use time::OffsetDateTime;

use crate::report::ReportFiles;
use crate::sources::{PageRef, page_of, push_unique};
use crate::verify::{ClaimCheck, Tally};
use tern_research::deep::WorkerStatus;
use tern_research::plan::{ResearchPlan, WorkerBudget};

/// What a step was, for its icon and for the Raw view's filters.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum StepKind {
    /// The turn started or ended.
    Turn,
    /// The model replied.
    Model,
    /// A web search.
    Search,
    /// A page read.
    Read,
    /// Any other tool.
    Tool,
    /// The policy gate's decision on a call.
    Policy,
    /// An approval asked for or answered.
    Approval,
    /// A tool failed or was rejected, a loop was caught, the budget ran out.
    Problem,
    /// A deep research plan drafted.
    Plan,
    /// A deep research worker started or finished.
    Worker,
}

/// The Raw view's filter chips.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum RawFilter {
    #[default]
    All,
    Tools,
    Approvals,
    Search,
}

impl RawFilter {
    pub const ALL: [RawFilter; 4] = [Self::All, Self::Tools, Self::Approvals, Self::Search];

    pub fn label(self) -> &'static str {
        match self {
            Self::All => "All",
            Self::Tools => "Tools",
            Self::Approvals => "Approvals",
            Self::Search => "Search",
        }
    }

    pub fn admits(self, kind: StepKind) -> bool {
        match self {
            Self::All => true,
            Self::Tools => matches!(
                kind,
                StepKind::Tool
                    | StepKind::Read
                    | StepKind::Search
                    | StepKind::Problem
                    | StepKind::Worker
            ),
            Self::Approvals => {
                matches!(kind, StepKind::Approval | StepKind::Policy | StepKind::Plan)
            }
            Self::Search => matches!(kind, StepKind::Search | StepKind::Read),
        }
    }
}

/// One event of the turn.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RunStep {
    pub seq: u64,
    /// The dotted event type, for example `tool.executed`.
    pub event: String,
    /// The event's plain-language line.
    pub summary: String,
    pub kind: StepKind,
    /// The Detailed view's verb: "Searched", "Read", "Asked", "Allowed"...
    pub verb: &'static str,
    /// What the verb acted on: a query, a URL, a tool, a model.
    pub target: String,
    /// Extra muted text after the target: result counts, tokens, a reason.
    pub note: String,
    /// Time since the turn started.
    pub at: Duration,
    /// Tokens in and out, for model replies.
    pub tokens: Option<(u64, u64)>,
    /// False for failures, rejections, denials, and declined approvals.
    pub ok: bool,
    /// Shown in the Detailed view (Raw shows every step).
    pub detailed: bool,
    /// The page a read returned.
    pub page: Option<PageRef>,
    pub payload: Value,
}

impl RunStep {
    /// The payload as indented JSON, cut to `limit` characters.
    pub fn payload_text(&self, limit: usize) -> String {
        let s = serde_json::to_string_pretty(&self.payload).unwrap_or_default();
        if s.chars().count() <= limit {
            s
        } else {
            let mut cut: String = s.chars().take(limit).collect();
            cut.push_str("\n…");
            cut
        }
    }
}

/// Counts for the Summary view's rollup line.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct RunSummary {
    pub searches: usize,
    pub results: u64,
    pub pages: usize,
    pub tools: usize,
    pub model_calls: usize,
    pub approvals: usize,
    pub problems: usize,
    pub tokens_in: u64,
    pub tokens_out: u64,
    pub took: Duration,
}

impl RunSummary {
    /// "Searched 2 times · read 6 pages · 3 model calls · 12.4k tokens".
    pub fn line(&self) -> String {
        let mut parts = Vec::new();
        if self.searches > 0 {
            parts.push(format!("Searched {}", times(self.searches)));
        }
        if self.pages > 0 {
            parts.push(format!("read {}", plural(self.pages, "page")));
        }
        if self.tools > 0 {
            parts.push(format!("{} other", plural(self.tools, "tool")));
        }
        if self.approvals > 0 {
            parts.push(plural(self.approvals, "approval"));
        }
        if self.model_calls > 0 {
            parts.push(plural(self.model_calls, "model call"));
        }
        let tokens = self.tokens_in + self.tokens_out;
        if tokens > 0 {
            parts.push(format!("{} tokens", compact(tokens)));
        }
        if parts.is_empty() {
            return String::new();
        }
        let mut line = parts.join(" · ");
        if let Some(first) = line.get(0..1) {
            let upper = first.to_uppercase();
            line.replace_range(0..1, &upper);
        }
        line
    }
}

/// How the run ended, if it has.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum RunEnd {
    Answered,
    /// Stopped short, with the reason the log gives.
    Stopped(String),
}

/// Where a deep run's plan stands.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PlanState {
    /// Drafted and waiting for the user.
    Waiting,
    Approved,
    Edited,
    /// Run without a plan: one worker on the whole question.
    Skipped,
    Cancelled,
}

/// One deep-research worker, as a front end's workers view shows it.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct WorkerRow {
    /// The sub-question's number in the plan.
    pub n: usize,
    pub question: String,
    pub status: WorkerStatus,
    /// Pages read, in order.
    pub pages: Vec<PageRef>,
    pub searches: usize,
    pub tokens_in: u64,
    pub tokens_out: u64,
    /// Why it ended early, when it did.
    pub note: String,
    pub budget: Option<WorkerBudget>,
    pub took: Duration,
}

/// A deep run's plan and workers, rebuilt from its events.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct DeepRun {
    pub plan: Option<ResearchPlan>,
    /// The preflight token range shown with the plan.
    pub estimate: Option<(u64, u64)>,
    /// Why Tern drafted the plan itself, when it did.
    pub plan_note: Option<String>,
    pub plan_answer: Option<PlanState>,
    /// Workers allowed at once.
    pub concurrency: usize,
    pub workers: Vec<WorkerRow>,
}

impl DeepRun {
    fn worker(&mut self, n: usize) -> Option<&mut WorkerRow> {
        self.workers.iter_mut().find(|w| w.n == n)
    }

    /// How many workers are running.
    pub fn running(&self) -> usize {
        self.workers
            .iter()
            .filter(|w| w.status == WorkerStatus::Running)
            .count()
    }

    /// Finished workers, of all.
    pub fn finished(&self) -> usize {
        self.workers
            .iter()
            .filter(|w| w.status.is_finished())
            .count()
    }

    fn push(&mut self, e: &EventEnvelope, at: Duration) {
        let p = &e.payload;
        let worker = p["worker"]
            .as_u64()
            .or_else(|| p["meta"]["worker"].as_u64())
            .map(|n| n as usize);
        match e.kind.as_str() {
            "plan.drafted" => {
                self.plan = serde_json::from_value(p["plan"].clone()).ok();
                self.estimate = p["estimate"]["low"]
                    .as_u64()
                    .zip(p["estimate"]["high"].as_u64());
                self.plan_note = str_at(p, "note").map(str::to_owned);
                self.plan_answer = Some(PlanState::Waiting);
            }
            "plan.answered" => {
                self.plan_answer = Some(match str_at(p, "answer").unwrap_or_default() {
                    "cancel" => PlanState::Cancelled,
                    "skip" => PlanState::Skipped,
                    _ if p["edited"] == true => PlanState::Edited,
                    _ => PlanState::Approved,
                });
                if let Ok(plan) = serde_json::from_value::<ResearchPlan>(p["plan"].clone()) {
                    self.workers = plan
                        .sub_questions
                        .iter()
                        .map(|s| WorkerRow {
                            n: s.n,
                            question: s.question.clone(),
                            ..WorkerRow::default()
                        })
                        .collect();
                    self.plan = Some(plan);
                }
            }
            "worker.started" => {
                if let Some(w) = worker.and_then(|n| self.worker(n)) {
                    w.status = WorkerStatus::Running;
                    w.budget = serde_json::from_value(p["budget"].clone()).ok();
                    w.took = at;
                }
            }
            "worker.finished" => {
                if let Some(w) = worker.and_then(|n| self.worker(n)) {
                    w.status = serde_json::from_value(p["status"].clone()).unwrap_or_default();
                    w.note = str_at(p, "note").unwrap_or_default().to_owned();
                    w.took = Duration::from_millis(p["took_ms"].as_u64().unwrap_or_default());
                    if w.budget.is_none() {
                        w.budget = serde_json::from_value(p["budget"].clone()).ok();
                    }
                }
            }
            "tool.executed" => {
                let page = page_of(&e.kind, p);
                if let Some(w) = worker.and_then(|n| self.worker(n)) {
                    match (str_at(p, "tool"), page) {
                        (Some("web_search"), _) => w.searches += 1,
                        (_, Some(pg)) => push_unique(&mut w.pages, pg),
                        _ => {}
                    }
                }
            }
            "model.replied" => {
                if let (Some(w), Some(u)) =
                    (worker.and_then(|n| self.worker(n)), e.usage_delta.as_ref())
                {
                    w.tokens_in += u.input_tokens;
                    w.tokens_out += u.output_tokens;
                }
            }
            _ => {}
        }
    }
}

/// One turn's steps, in order.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Run {
    started: Option<OffsetDateTime>,
    pub steps: Vec<RunStep>,
    pub end: Option<RunEnd>,
    /// The answer's claims and their verdicts, once checked.
    pub checks: Vec<ClaimCheck>,
    /// Where the run's report was written.
    pub report: Option<ReportFiles>,
    /// The plan and workers, for a deep research turn.
    pub deep: Option<Box<DeepRun>>,
}

impl Run {
    pub fn new() -> Self {
        Self::default()
    }

    /// Adds an event. The first event's time is the turn's start.
    pub fn push(&mut self, e: &EventEnvelope) {
        let started = *self.started.get_or_insert(e.ts);
        let at = (e.ts - started).try_into().unwrap_or_default();
        if matches!(
            e.kind.as_str(),
            "turn.finished" | "run.cancelled" | "budget.exceeded"
        ) {
            let completed = e.kind == "turn.finished"
                && str_at(&e.payload, "state").is_some_and(|s| s.starts_with("completed"));
            self.end = Some(if completed {
                RunEnd::Answered
            } else {
                RunEnd::Stopped(e.user_summary.clone())
            });
        }
        if e.kind == "turn.started" && str_at(&e.payload, "mode") == Some("deep") {
            self.deep = Some(Box::new(DeepRun {
                concurrency: e.payload["workers"].as_u64().unwrap_or(1) as usize,
                ..DeepRun::default()
            }));
        }
        if let Some(deep) = self.deep.as_mut() {
            deep.push(e, at);
        }
        if e.kind == "claims.checked" {
            self.checks = serde_json::from_value(e.payload["checks"].clone()).unwrap_or_default();
            self.report = serde_json::from_value(e.payload["report"].clone()).ok();
        }
        self.steps.push(step_of(e, at));
    }

    /// When the turn started.
    pub fn started(&self) -> Option<OffsetDateTime> {
        self.started
    }

    /// Counts per verdict.
    pub fn tally(&self) -> Tally {
        Tally::of(&self.checks)
    }

    pub fn is_empty(&self) -> bool {
        self.steps.is_empty()
    }

    /// Steps for the Detailed view.
    pub fn detailed(&self) -> impl Iterator<Item = &RunStep> {
        self.steps.iter().filter(|s| s.detailed)
    }

    /// Steps for the Raw view under `filter`.
    pub fn raw(&self, filter: RawFilter) -> impl Iterator<Item = &RunStep> {
        self.steps.iter().filter(move |s| filter.admits(s.kind))
    }

    /// How many steps the transcript's "N steps" counts: tool work,
    /// approvals, and problems, not model replies or bookkeeping.
    pub fn step_count(&self) -> usize {
        self.steps
            .iter()
            .filter(|s| {
                matches!(
                    s.kind,
                    StepKind::Search
                        | StepKind::Read
                        | StepKind::Tool
                        | StepKind::Approval
                        | StepKind::Problem
                ) && s.event != "approval.requested"
            })
            .count()
    }

    /// Pages read, in order, without repeats.
    pub fn pages(&self) -> Vec<PageRef> {
        let mut out = Vec::new();
        for p in self.steps.iter().filter_map(|s| s.page.clone()) {
            push_unique(&mut out, p);
        }
        out
    }

    /// Steps that went wrong, for the Summary view.
    pub fn problems(&self) -> impl Iterator<Item = &RunStep> {
        self.steps.iter().filter(|s| !s.ok)
    }

    pub fn summary(&self) -> RunSummary {
        let mut s = RunSummary {
            took: self.steps.last().map(|x| x.at).unwrap_or_default(),
            pages: self.pages().len(),
            ..RunSummary::default()
        };
        for step in &self.steps {
            match (step.kind, step.event.as_str()) {
                (StepKind::Search, "tool.executed") => {
                    s.searches += 1;
                    s.results += step.payload["meta"]["results"].as_u64().unwrap_or(0);
                }
                (StepKind::Tool, "tool.executed") => s.tools += 1,
                (StepKind::Model, _) => {
                    s.model_calls += 1;
                    if let Some((i, o)) = step.tokens {
                        s.tokens_in += i;
                        s.tokens_out += o;
                    }
                }
                (StepKind::Approval, "approval.answered") => s.approvals += 1,
                _ => {}
            }
            if !step.ok {
                s.problems += 1;
            }
        }
        s
    }
}

/// Shapes one event into a step.
pub fn step_of(e: &EventEnvelope, at: Duration) -> RunStep {
    let p = &e.payload;
    let tool = str_at(p, "tool").unwrap_or_default().to_owned();
    let page = page_of(&e.kind, p);
    let mut step = RunStep {
        seq: e.seq,
        event: e.kind.clone(),
        summary: e.user_summary.clone(),
        kind: StepKind::Turn,
        verb: "",
        target: String::new(),
        note: String::new(),
        at,
        tokens: e
            .usage_delta
            .as_ref()
            .map(|u| (u.input_tokens, u.output_tokens)),
        ok: true,
        detailed: true,
        page: None,
        payload: p.clone(),
    };
    match e.kind.as_str() {
        "turn.started" => {
            step.verb = "Started";
            step.target = str_at(p, "model").unwrap_or("the turn").to_owned();
            step.detailed = false;
        }
        "turn.finished" | "run.cancelled" | "budget.exceeded" => {
            let completed = e.kind == "turn.finished"
                && str_at(p, "state").is_some_and(|s| s.starts_with("completed"));
            step.verb = if completed { "Answered" } else { "Stopped" };
            step.ok = completed;
            if !completed {
                step.kind = StepKind::Problem;
                step.note = e.user_summary.clone();
            }
        }
        "model.replied" => {
            step.kind = StepKind::Model;
            step.verb = "Model replied";
            step.target = str_at(p, "model").unwrap_or_default().to_owned();
            if let Some((i, o)) = step.tokens {
                step.note = format!("{} in · {} out", compact(i), compact(o));
            }
            match (str_at(p, "purpose"), p["worker"].as_u64()) {
                (_, Some(n)) => step.note = with_worker(&step.note, n),
                (Some("plan"), _) => step.note = format!("plan · {}", step.note),
                (Some("synthesis"), _) => step.note = format!("answer · {}", step.note),
                _ => {}
            }
        }
        "plan.drafted" => {
            step.kind = StepKind::Plan;
            step.verb = "Planned";
            let n = p["plan"]["sub_questions"]
                .as_array()
                .map(Vec::len)
                .unwrap_or_default();
            step.target = plural(n, "sub-question");
            step.note = match str_at(&p["plan"], "source") {
                Some("fallback") => "Tern's own split".to_owned(),
                _ => "drafted by the model".to_owned(),
            };
        }
        "plan.answered" => {
            step.kind = StepKind::Approval;
            step.target = "the plan".into();
            step.verb = match str_at(p, "answer").unwrap_or_default() {
                "cancel" => {
                    step.ok = false;
                    "You cancelled"
                }
                "skip" => {
                    step.note = "one worker on the whole question".into();
                    "You skipped"
                }
                _ if p["edited"] == true => "You edited and approved",
                _ => "You approved",
            };
        }
        "worker.started" => {
            step.kind = StepKind::Worker;
            step.verb = "Started worker";
            step.target = str_at(p, "question").unwrap_or_default().to_owned();
            step.note = format!("worker {}", p["worker"].as_u64().unwrap_or_default());
        }
        "worker.finished" => {
            let status: WorkerStatus =
                serde_json::from_value(p["status"].clone()).unwrap_or_default();
            step.kind = if status.is_ok() {
                StepKind::Worker
            } else {
                StepKind::Problem
            };
            step.ok = status.is_ok();
            step.verb = if status.is_ok() {
                "Worker done"
            } else {
                "Worker stopped"
            };
            step.target = str_at(p, "question").unwrap_or_default().to_owned();
            let pages = p["pages"].as_array().map(Vec::len).unwrap_or_default();
            let mut note = format!(
                "worker {} · {}",
                p["worker"].as_u64().unwrap_or_default(),
                plural(pages, "page")
            );
            if let Some(why) = str_at(p, "note").filter(|w| !w.is_empty()) {
                note.push_str(&format!(" · {why}"));
            }
            step.note = note;
        }
        "policy.decided" => {
            step.kind = StepKind::Policy;
            let decision = p["decision"]["decision"].as_str().unwrap_or_default();
            step.target = tool;
            match decision {
                "allow" => {
                    step.verb = "Allowed";
                    step.note = p["decision"]["because"]
                        .as_str()
                        .unwrap_or_default()
                        .to_owned();
                }
                "ask" => step.verb = "Needs approval",
                _ => {
                    step.verb = "Blocked";
                    step.ok = false;
                    step.note = p["decision"]["reason"]
                        .as_str()
                        .unwrap_or_default()
                        .to_owned();
                }
            }
        }
        "approval.requested" => {
            step.kind = StepKind::Approval;
            step.verb = "Asked you";
            step.target = tool;
            step.note = str_at(p, "target").unwrap_or_default().to_owned();
        }
        "approval.answered" => {
            step.kind = StepKind::Approval;
            let answer = str_at(p, "answer").unwrap_or_default();
            step.target = tool;
            (step.verb, step.note) = match answer {
                "allow_once" => ("You approved", "once".to_owned()),
                "allow_for_session" => ("You approved", "for this chat".to_owned()),
                _ => ("You declined", String::new()),
            };
            step.ok = answer != "deny";
        }
        "claims.checking" => {
            step.verb = "Checking claims";
            step.detailed = false;
        }
        "claims.checked" => {
            let checks: Vec<ClaimCheck> =
                serde_json::from_value(p["checks"].clone()).unwrap_or_default();
            step.verb = "Checked";
            step.target = plural(checks.len(), "claim");
            step.note = Tally::of(&checks).line();
            if let Some(err) = str_at(p, "judge_error") {
                step.note = format!("{} · judged by word overlap: {err}", step.note);
            }
        }
        "tool.executed" => {
            let args = &p["args"];
            match tool.as_str() {
                "web_search" => {
                    step.kind = StepKind::Search;
                    step.verb = "Searched";
                    step.target = args["query"].as_str().unwrap_or_default().to_owned();
                    if let Some(n) = p["meta"]["results"].as_u64() {
                        step.note = plural(n as usize, "result");
                    }
                    if let Some(w) = p["worker"].as_u64() {
                        step.note = with_worker(&step.note, w);
                    }
                }
                "read_url" => {
                    step.kind = StepKind::Read;
                    step.verb = "Read";
                    step.target = page
                        .as_ref()
                        .map(|pg| pg.url.clone())
                        .or_else(|| args["url"].as_str().map(str::to_owned))
                        .unwrap_or_default();
                    if let Some(pg) = &page {
                        step.note = pg.label().to_owned();
                    }
                    if let Some(w) = p["worker"].as_u64() {
                        step.note = with_worker(&step.note, w);
                    }
                }
                _ => {
                    step.kind = StepKind::Tool;
                    step.verb = "Ran";
                    step.target = tool;
                }
            }
            step.page = page;
        }
        "tool.failed" => {
            step.kind = StepKind::Problem;
            step.verb = "Failed";
            step.target = p["args"]["url"]
                .as_str()
                .or_else(|| p["args"]["query"].as_str())
                .map(str::to_owned)
                .unwrap_or(tool);
            step.note = str_at(p, "error")
                .map(str::to_owned)
                .unwrap_or_else(|| e.user_summary.clone());
            step.ok = false;
        }
        "tool.rejected" | "loop.suspected" => {
            step.kind = StepKind::Problem;
            step.verb = if e.kind == "loop.suspected" {
                "Caught a loop"
            } else {
                "Rejected"
            };
            step.target = tool;
            step.note = e.user_summary.clone();
            step.ok = false;
        }
        _ => {
            step.verb = "Logged";
            step.target = e.kind.clone();
            step.detailed = false;
        }
    }
    step
}

fn str_at<'a>(v: &'a Value, key: &str) -> Option<&'a str> {
    v.get(key).and_then(Value::as_str)
}

fn with_worker(note: &str, n: u64) -> String {
    if note.is_empty() {
        format!("worker {n}")
    } else {
        format!("{note} · worker {n}")
    }
}

fn plural(n: usize, word: &str) -> String {
    format!("{n} {word}{}", if n == 1 { "" } else { "s" })
}

fn times(n: usize) -> String {
    match n {
        1 => "once".into(),
        2 => "twice".into(),
        _ => format!("{n} times"),
    }
}

/// 950, 1.2k, 128k, 1.1M.
pub fn compact(n: u64) -> String {
    match n {
        0..=999 => n.to_string(),
        1_000..=9_999 => format!("{:.1}k", n as f64 / 1_000.0),
        10_000..=999_999 => format!("{}k", n / 1_000),
        _ => format!("{:.1}M", n as f64 / 1_000_000.0),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use tern_core::{EffectClass, Measure, SessionId, UsageDelta};

    fn ev(s: SessionId, kind: &str, summary: &str, payload: Value) -> EventEnvelope {
        EventEnvelope::new(s, kind, EffectClass::Analysis, summary, payload)
    }

    fn sample() -> Run {
        let s = SessionId::new();
        let mut reply = ev(s, "model.replied", "m replied", json!({"model": "m"}));
        reply.usage_delta = Some(UsageDelta {
            input_tokens: 1200,
            output_tokens: 300,
            cost_micros: None,
            currency: "USD".into(),
            measure: Measure::Measured,
        });
        let page =
            json!({"title": "RAG paper", "url": "https://arxiv.org/abs/1", "site": "arxiv.org"});
        let mut run = Run::new();
        for e in [
            ev(s, "turn.started", "Started a turn", json!({"model": "m"})),
            reply,
            ev(
                s,
                "policy.decided",
                "`web_search` allowed: read-only",
                json!({"tool": "web_search", "decision": {"decision": "allow", "because": "read-only"}}),
            ),
            ev(
                s,
                "tool.executed",
                "Ran `web_search`",
                json!({"tool": "web_search", "args": {"query": "what is rag"}, "meta": {"results": 6}}),
            ),
            ev(
                s,
                "approval.requested",
                "Asked you",
                json!({"tool": "read_url", "target": "arxiv.org"}),
            ),
            ev(
                s,
                "approval.answered",
                "You approved",
                json!({"tool": "read_url", "answer": "allow_for_session"}),
            ),
            ev(
                s,
                "tool.executed",
                "Ran `read_url`",
                json!({"tool": "read_url", "args": {"url": "https://arxiv.org/abs/1"}, "meta": {"page": page}}),
            ),
            ev(
                s,
                "tool.failed",
                "`read_url` failed",
                json!({"tool": "read_url", "args": {"url": "https://x.test"}, "error": "timed out"}),
            ),
            ev(
                s,
                "turn.finished",
                "Answered",
                json!({"state": "completed"}),
            ),
        ] {
            run.push(&e);
        }
        run
    }

    #[test]
    fn steps_get_verbs_and_targets() {
        let run = sample();
        let d: Vec<_> = run
            .detailed()
            .map(|s| (s.verb, s.target.as_str(), s.note.as_str()))
            .collect();
        assert_eq!(
            d,
            [
                ("Model replied", "m", "1.2k in · 300 out"),
                ("Allowed", "web_search", "read-only"),
                ("Searched", "what is rag", "6 results"),
                ("Asked you", "read_url", "arxiv.org"),
                ("You approved", "read_url", "for this chat"),
                ("Read", "https://arxiv.org/abs/1", "RAG paper"),
                ("Failed", "https://x.test", "timed out"),
                ("Answered", "", ""),
            ]
        );
        assert_eq!(run.end, Some(RunEnd::Answered));
    }

    #[test]
    fn summary_rolls_up() {
        let run = sample();
        let s = run.summary();
        assert_eq!((s.searches, s.results, s.pages, s.approvals), (1, 6, 1, 1));
        assert_eq!((s.model_calls, s.problems), (1, 1));
        assert_eq!(
            s.line(),
            "Searched once · read 1 page · 1 approval · 1 model call · 1.5k tokens"
        );
        assert_eq!(run.step_count(), 4);
        assert_eq!(run.problems().count(), 1);
    }

    #[test]
    fn raw_filters() {
        let run = sample();
        assert_eq!(run.raw(RawFilter::All).count(), 9);
        assert_eq!(run.raw(RawFilter::Search).count(), 2);
        assert_eq!(run.raw(RawFilter::Approvals).count(), 3);
        assert_eq!(run.raw(RawFilter::Tools).count(), 3);
    }

    #[test]
    fn a_stopped_run_says_why() {
        let s = SessionId::new();
        let mut run = Run::new();
        run.push(&ev(s, "turn.started", "Started", json!({})));
        run.push(&ev(s, "run.cancelled", "You stopped the turn", json!({})));
        assert_eq!(
            run.end,
            Some(RunEnd::Stopped("You stopped the turn".into()))
        );
        assert_eq!(run.problems().count(), 1);
    }

    #[test]
    fn payload_is_cut() {
        let run = sample();
        let text = run.steps[3].payload_text(20);
        assert!(text.ends_with('…'));
    }
}
