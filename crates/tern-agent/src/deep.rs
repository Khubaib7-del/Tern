//! Deep research: a plan the user approves, workers that research its
//! sub-questions in parallel, and one cited synthesis.
//!
//! | Step | What happens | Event |
//! | --- | --- | --- |
//! | Plan | The model drafts sub-questions with searches, source types, and budget shares; an unreadable reply falls back to Tern's own split | `plan.drafted` |
//! | Approve | The user runs, edits, or skips the plan, or cancels. Nothing is searched or read before this | `plan.answered` |
//! | Workers | Up to N at once (3 by default). Each searches, reads through the shared Tern Crawl reader (robots.txt, pacing, cache), and writes notes citing its pages, within its own slice of tokens, pages, and time | `worker.started`, `tool.executed`, `model.replied`, `worker.finished` |
//! | Synthesis | The workers' notes, renumbered into one list of pages, become one answer; citations are renumbered by first use and the link list appended | `model.replied`, `turn.finished` |
//!
//! Only each worker's notes and their pages reach the synthesis, never a
//! worker's full context. Each worker can be cancelled on its own
//! ([`DeepControl::cancel_worker`]) and the whole run at once. The claim
//! checks and run report of an ordinary turn then run on the answer.

use std::collections::{HashSet, VecDeque};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use futures::StreamExt;
use serde_json::{Value, json};
use tern_core::{EffectClass, EventEnvelope, SessionId, TurnId, TurnState, TurnStateMachine};
use tern_crawl::{Fetcher, SearchClient, SearchHit};
use tern_providers::{ChatMessage, ChatRequest, Usage, UsageLedger};
pub use tern_research::deep::{
    Final, Merged, NoteSource, NumberedText, WorkerNotes, WorkerStatus, finalize, merge,
    notes_prompt, parse_notes, synthesis_prompt,
};
pub use tern_research::plan::{
    MAX_SUB_QUESTIONS, PlanBudget, PlanSource, ResearchPlan, SourceKind, SubQuestion, WorkerBudget,
    fallback_plan, parse_plan, plan_prompt, single_plan, split_budget,
};

use crate::engine::{Budget, CancelFlag, EventSink, Model, OnText, TurnOutcome};
use crate::policy::{CallView, Decision, PolicyGate};
use crate::sources::PageRef;
use crate::tools::BoxFuture;

/// Most workers that may run at once.
pub const MAX_WORKERS: usize = 6;
/// Search-read-note rounds per worker: the planned searches, then at most
/// one more the worker asks for.
pub const ROUNDS: usize = 2;
/// Pages a worker reads per round.
pub const READS_PER_ROUND: usize = 3;
/// Most characters of one page shown to a worker's model.
pub const SOURCE_CHARS: usize = 6_000;
/// Results asked for per search.
const RESULTS: usize = 6;
/// Output tokens for a worker's notes.
const NOTES_TOKENS: u32 = 900;

/// Searches the web for a worker. [`SearchClient`] is the real one.
pub trait Searcher: Send + Sync {
    /// The service's id, the target the policy gate sees (`tavily`).
    fn id(&self) -> &str;
    fn search<'a>(
        &'a self,
        query: &'a str,
        limit: usize,
    ) -> BoxFuture<'a, Result<Vec<SearchHit>, String>>;
}

impl Searcher for SearchClient {
    fn id(&self) -> &str {
        self.spec().id
    }
    fn search<'a>(
        &'a self,
        query: &'a str,
        limit: usize,
    ) -> BoxFuture<'a, Result<Vec<SearchHit>, String>> {
        Box::pin(async move {
            SearchClient::search(self, query, limit)
                .await
                .map_err(|e| e.to_string())
        })
    }
}

/// A page a worker read: what its source row shows and its text.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ReadPage {
    pub page: PageRef,
    pub text: String,
}

/// Reads pages for a worker. [`Fetcher`] (Tern Crawl, with robots.txt,
/// per-host pacing, and the page cache) is the real one.
pub trait Reader: Send + Sync {
    fn read<'a>(&'a self, url: &'a str) -> BoxFuture<'a, Result<ReadPage, String>>;
    /// Whether the page is already in the cache, for the run report.
    fn cached(&self, _url: &str) -> bool {
        false
    }
}

impl Reader for Fetcher {
    fn read<'a>(&'a self, url: &'a str) -> BoxFuture<'a, Result<ReadPage, String>> {
        Box::pin(async move {
            let p = self.fetch(url).await.map_err(|e| e.to_string())?;
            Ok(ReadPage {
                page: PageRef {
                    title: p.title.clone(),
                    url: p.final_url.clone(),
                    site: p.site.clone(),
                    builtin: p.icon.builtin.map(str::to_owned),
                    favicon: p.icon.favicon.clone(),
                },
                text: p.text,
            })
        })
    }
    fn cached(&self, url: &str) -> bool {
        self.has_cached(url)
    }
}

/// The user's answer to a drafted plan.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum PlanAnswer {
    /// Run this plan: the draft, or the user's edit of it.
    Run(ResearchPlan),
    /// Run without a plan: one worker on the whole question.
    Skip,
    /// Stop before anything is searched.
    Cancel,
}

/// Puts a drafted plan in front of the user and waits for the answer.
pub trait PlanApprover: Send + Sync {
    fn review<'a>(&'a self, plan: &'a ResearchPlan) -> BoxFuture<'a, PlanAnswer>;
}

/// Runs every plan as drafted. For runs with no one to ask (`--yes`).
#[derive(Clone, Copy, Debug, Default)]
pub struct RunAsDrafted;

impl PlanApprover for RunAsDrafted {
    fn review<'a>(&'a self, plan: &'a ResearchPlan) -> BoxFuture<'a, PlanAnswer> {
        Box::pin(async move { PlanAnswer::Run(plan.clone()) })
    }
}

/// Stops a deep run as a whole or one worker at a time, from any thread.
#[derive(Clone, Debug, Default)]
pub struct DeepControl {
    all: CancelFlag,
    workers: Arc<Mutex<HashSet<usize>>>,
}

impl DeepControl {
    /// A control whose whole-run cancel is `all` (the turn's stop button).
    pub fn new(all: CancelFlag) -> Self {
        Self {
            all,
            workers: Arc::default(),
        }
    }

    pub fn cancel_all(&self) {
        self.all.cancel();
    }

    /// Stops worker `n` (its sub-question's number); the rest carry on.
    pub fn cancel_worker(&self, n: usize) {
        if let Ok(mut w) = self.workers.lock() {
            w.insert(n);
        }
    }

    pub fn is_cancelled(&self) -> bool {
        self.all.is_cancelled()
    }

    pub fn worker_cancelled(&self, n: usize) -> bool {
        self.is_cancelled() || self.workers.lock().map(|w| w.contains(&n)).unwrap_or(false)
    }

    pub fn flag(&self) -> CancelFlag {
        self.all.clone()
    }
}

/// How a deep run is shaped.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct DeepOptions {
    /// Workers at once, 1 to [`MAX_WORKERS`].
    pub workers: usize,
    /// Most sub-questions the plan may hold.
    pub max_sub_questions: usize,
    pub budget: PlanBudget,
    /// Output tokens for the synthesis.
    pub max_output_tokens: u32,
}

impl Default for DeepOptions {
    fn default() -> Self {
        Self {
            workers: 3,
            max_sub_questions: 5,
            budget: PlanBudget::default(),
            max_output_tokens: 2048,
        }
    }
}

/// The engine limits a deep turn's report measures against.
pub fn turn_budget(plan: &ResearchPlan) -> Budget {
    let n = plan.sub_questions.len() as u32;
    Budget {
        max_model_calls: 2 + n * ROUNDS as u32,
        max_tool_calls: n * (tern_research::plan::MAX_QUERIES as u32 + 1) + plan.budget.pages,
        max_tokens: plan.budget.tokens,
    }
}

/// Everything a deep run needs, given explicitly (tests build it by hand).
#[allow(missing_debug_implementations)]
pub struct DeepDeps<'a> {
    pub session: SessionId,
    pub model: &'a dyn Model,
    pub model_name: &'a str,
    pub search: &'a dyn Searcher,
    pub reader: &'a dyn Reader,
    pub sink: &'a dyn EventSink,
    pub ledger: &'a UsageLedger,
    /// Every search and read is put to the gate; `None` allows them all.
    pub gate: Option<&'a mut PolicyGate>,
    /// Streams the synthesis to a front end.
    pub on_text: Option<OnText<'a>>,
}

/// Why a worker or the run stopped early.
#[derive(Clone, Debug, PartialEq, Eq)]
enum Halt {
    Cancelled,
    TimedOut,
    OverBudget(String),
    Failed(String),
}

/// A step that ends the run.
struct Stop {
    state: TurnState,
    reason: String,
}

struct Deep<'r> {
    session: SessionId,
    turn: TurnId,
    model: &'r dyn Model,
    model_name: &'r str,
    search: &'r dyn Searcher,
    reader: &'r dyn Reader,
    sink: &'r dyn EventSink,
    ledger: &'r UsageLedger,
    gate: Mutex<Option<&'r mut PolicyGate>>,
    on_text: Option<OnText<'r>>,
    control: &'r DeepControl,
    usage: Mutex<Usage>,
    calls: Mutex<(u32, u32)>,
}

/// What a worker has so far.
#[derive(Default)]
struct WorkerState {
    pages: Vec<ReadPage>,
    seen: HashSet<String>,
    searches: usize,
    tokens: (u64, u64),
    notes: String,
}

/// Runs one deep-research turn on `question` (the user's message; any
/// attachments or quoted reply in it are kept in the log but left out of
/// the plan).
pub async fn run_deep(
    deps: DeepDeps<'_>,
    message: &str,
    opts: &DeepOptions,
    approver: &dyn PlanApprover,
    control: &DeepControl,
) -> TurnOutcome {
    let deep = Deep {
        session: deps.session,
        turn: TurnId::new(),
        model: deps.model,
        model_name: deps.model_name,
        search: deps.search,
        reader: deps.reader,
        sink: deps.sink,
        ledger: deps.ledger,
        gate: Mutex::new(deps.gate),
        on_text: deps.on_text,
        control,
        usage: Mutex::new(Usage::measured(0, 0)),
        calls: Mutex::new((0, 0)),
    };
    let mut fsm = TurnStateMachine::default();
    let result = deep.drive(&mut fsm, message, opts, approver).await;
    deep.finish(fsm, result)
}

impl Deep<'_> {
    fn record(&self, kind: &str, summary: impl Into<String>, payload: Value) -> Result<(), String> {
        self.record_event(
            EventEnvelope::new(self.session, kind, effect_of(kind), summary, payload)
                .with_turn(self.turn),
        )
    }

    fn record_event(&self, e: EventEnvelope) -> Result<(), String> {
        self.sink
            .record(e)
            .map(|_| ())
            .map_err(|e| format!("Tern could not write the run log, so it stopped: {e}"))
    }

    fn add_usage(&self, u: Usage) -> Usage {
        self.ledger
            .record(self.model.provider_id(), self.model_name, u);
        let mut total = self.usage.lock().expect("usage lock");
        total.input_tokens += u.input_tokens;
        total.output_tokens += u.output_tokens;
        if u.measure == tern_core::Measure::Estimated {
            total.measure = tern_core::Measure::Estimated;
        }
        *total
    }

    fn count(&self, model: u32, tools: u32) {
        let mut c = self.calls.lock().expect("calls lock");
        c.0 += model;
        c.1 += tools;
    }

    fn go(fsm: &mut TurnStateMachine, to: TurnState) -> Result<(), Stop> {
        fsm.move_to(to).map_err(|e| Stop {
            state: TurnState::Failed,
            reason: format!("internal error: {e}"),
        })
    }

    fn log_stop(e: String) -> Stop {
        Stop {
            state: TurnState::Failed,
            reason: e,
        }
    }

    fn cancelled() -> Stop {
        Stop {
            state: TurnState::Cancelled,
            reason: "You stopped the run.".into(),
        }
    }

    /// Waits for `fut`, giving up when worker `n` (or, for `None`, the
    /// whole run) is cancelled or `deadline` passes.
    async fn guard<T>(
        &self,
        n: Option<usize>,
        deadline: Option<tokio::time::Instant>,
        fut: impl Future<Output = T>,
    ) -> Result<T, Halt> {
        let mut fut = std::pin::pin!(fut);
        loop {
            tokio::select! {
                out = &mut fut => return Ok(out),
                _ = tokio::time::sleep(Duration::from_millis(40)) => {
                    let stopped = match n {
                        Some(n) => self.control.worker_cancelled(n),
                        None => self.control.is_cancelled(),
                    };
                    if stopped {
                        return Err(Halt::Cancelled);
                    }
                    if deadline.is_some_and(|d| tokio::time::Instant::now() >= d) {
                        return Err(Halt::TimedOut);
                    }
                }
            }
        }
    }

    /// Puts one search or read to the policy gate. Workers cannot stop to
    /// ask, so a call that needs approval ends the worker.
    fn allow(&self, tool: &str, target: Option<&str>, args: &Value) -> Result<(), Halt> {
        let mut gate = self.gate.lock().expect("gate lock");
        let Some(gate) = gate.as_mut() else {
            return Ok(());
        };
        match gate.check(&CallView {
            tool,
            effect: EffectClass::Network,
            target,
            args,
        }) {
            Decision::Allow { .. } => Ok(()),
            Decision::Ask => Err(Halt::Failed(format!(
                "`{tool}` on {} needs your approval, and workers cannot stop to ask; ask it as a normal turn",
                target.unwrap_or("this target")
            ))),
            Decision::Deny { reason } => Err(Halt::Failed(reason)),
        }
    }

    async fn drive(
        &self,
        fsm: &mut TurnStateMachine,
        message: &str,
        opts: &DeepOptions,
        approver: &dyn PlanApprover,
    ) -> Result<(TurnState, String), Stop> {
        Self::go(fsm, TurnState::Preparing)?;
        let concurrency = opts.workers.clamp(1, MAX_WORKERS);
        self.record(
            "turn.started",
            "Started a deep research turn",
            json!({
                "model": self.model_name,
                "provider": self.model.provider_id(),
                "mode": "deep",
                "workers": concurrency,
                "tools": ["web_search", "read_url"],
                "budget": opts.budget,
                "message": message,
            }),
        )
        .map_err(Self::log_stop)?;
        let question = question_of(message);
        if question.is_empty() {
            return Err(Stop {
                state: TurnState::Failed,
                reason: "Deep research needs a question.".into(),
            });
        }

        // 1. The plan.
        Self::go(fsm, TurnState::ModelStreaming)?;
        let (system, prompt) = plan_prompt(&question, opts.max_sub_questions);
        let mut req = ChatRequest::new(self.model_name, vec![ChatMessage::user(prompt)])
            .with_system(system)
            .with_max_tokens(1200);
        req.temperature = Some(0.2);
        self.count(1, 0);
        let reply = self
            .guard(None, None, self.model.complete(&req))
            .await
            .map_err(|_| Self::cancelled())?;
        let (plan, why) = match reply {
            Ok(r) => {
                let total = self.add_usage(r.usage);
                self.record_event(
                    EventEnvelope::new(
                        self.session,
                        "model.replied",
                        EffectClass::Network,
                        format!("{} drafted a plan", self.model_name),
                        json!({"model": r.model, "purpose": "plan", "chars": r.text.len(), "finish_reason": r.finish_reason}),
                    )
                    .with_turn(self.turn)
                    .with_usage(r.usage.to_delta()),
                )
                .map_err(Self::log_stop)?;
                if total.total() > opts.budget.tokens {
                    return Err(Stop {
                        state: TurnState::BudgetExceeded,
                        reason: format!(
                            "Drafting the plan used {} tokens, over the run's {}.",
                            total.total(),
                            opts.budget.tokens
                        ),
                    });
                }
                match parse_plan(&question, &r.text, opts.budget, opts.max_sub_questions) {
                    Ok(p) => (p, None),
                    Err(e) => (
                        fallback_plan(&question, opts.budget, opts.max_sub_questions),
                        Some(format!(
                            "The model's plan could not be read ({e}), so Tern split the question itself."
                        )),
                    ),
                }
            }
            Err(e) => (
                fallback_plan(&question, opts.budget, opts.max_sub_questions),
                Some(format!(
                    "The model could not draft a plan ({e}), so Tern split the question itself."
                )),
            ),
        };
        let (low, high) = plan.estimate();
        self.record(
            "plan.drafted",
            format!(
                "Planned {} sub-question{}",
                plan.sub_questions.len(),
                if plan.sub_questions.len() == 1 { "" } else { "s" }
            ),
            json!({"plan": plan, "estimate": {"low": low, "high": high}, "note": why, "workers": concurrency}),
        )
        .map_err(Self::log_stop)?;

        // 2. The user's go-ahead. Nothing is searched or read before it.
        Self::go(fsm, TurnState::ToolRequested)?;
        Self::go(fsm, TurnState::PolicyCheck)?;
        Self::go(fsm, TurnState::ApprovalRequested)?;
        let answer = self
            .guard(None, None, approver.review(&plan))
            .await
            .unwrap_or(PlanAnswer::Cancel);
        let (plan, label, edited) = match answer {
            PlanAnswer::Cancel => {
                self.record(
                    "plan.answered",
                    "You cancelled the plan",
                    json!({"answer": "cancel"}),
                )
                .map_err(Self::log_stop)?;
                return Err(Stop {
                    state: TurnState::Cancelled,
                    reason: "You cancelled the plan, so nothing was searched.".into(),
                });
            }
            PlanAnswer::Skip => (single_plan(&question, opts.budget), "skip", false),
            PlanAnswer::Run(mut p) => {
                p.normalise(MAX_SUB_QUESTIONS);
                if p.sub_questions.is_empty() {
                    (single_plan(&question, opts.budget), "skip", false)
                } else {
                    let edited = p != plan;
                    (p, "run", edited)
                }
            }
        };
        self.record(
            "plan.answered",
            match (label, edited) {
                ("skip", _) => "You skipped the plan: one worker on the whole question".to_owned(),
                (_, true) => "You edited and approved the plan".to_owned(),
                _ => "You approved the plan".to_owned(),
            },
            json!({"answer": label, "edited": edited, "plan": plan, "workers": concurrency}),
        )
        .map_err(Self::log_stop)?;
        if self.control.is_cancelled() {
            return Err(Self::cancelled());
        }
        Self::go(fsm, TurnState::Executing)?;

        // 3. Workers, up to `concurrency` at once.
        let budgets = plan.worker_budgets(concurrency);
        // Collected first, so the stream holds plain futures rather than a
        // closure over borrowed items (which keeps the run `Send`).
        let mut jobs = Vec::with_capacity(budgets.len());
        for (sub, b) in plan.sub_questions.iter().zip(budgets) {
            jobs.push(self.worker(&question, sub, b));
        }
        let mut notes: Vec<WorkerNotes> = futures::stream::iter(jobs)
            .buffer_unordered(concurrency)
            .collect()
            .await;
        notes.sort_by_key(|w| w.worker);
        if self.control.is_cancelled() {
            return Err(Self::cancelled());
        }
        Self::go(fsm, TurnState::RecordResult)?;

        // 4. One answer from every worker's notes.
        let merged = merge(&notes);
        if merged.sources.is_empty() {
            let why: Vec<String> = notes
                .iter()
                .map(|w| format!("{}: {}", w.worker, w.status.label().to_lowercase()))
                .collect();
            return Err(Stop {
                state: TurnState::Failed,
                reason: format!(
                    "No worker found a page to cite ({}), so there is nothing to write from.",
                    why.join(", ")
                ),
            });
        }
        let used = self.usage.lock().expect("usage lock").total();
        if used >= plan.budget.tokens {
            return Err(Stop {
                state: TurnState::BudgetExceeded,
                reason: format!(
                    "The workers used {used} tokens of the run's {}, leaving none for the answer.",
                    plan.budget.tokens
                ),
            });
        }
        Self::go(fsm, TurnState::ModelStreaming)?;
        let (system, prompt) = synthesis_prompt(&question, &merged);
        let mut req = ChatRequest::new(self.model_name, vec![ChatMessage::user(prompt)])
            .with_system(system)
            .with_max_tokens(opts.max_output_tokens.max(256));
        req.temperature = Some(0.2);
        self.count(1, 0);
        let call = self.calls.lock().expect("calls lock").0;
        let reply = match self.on_text {
            Some(sink) => {
                let control = self.control.clone();
                let forward = move |t: &str| {
                    sink(call, t);
                    !control.is_cancelled()
                };
                self.guard(None, None, self.model.complete_stream(&req, &forward))
                    .await
            }
            None => self.guard(None, None, self.model.complete(&req)).await,
        }
        .map_err(|_| Self::cancelled())?
        .map_err(|e| Stop {
            state: TurnState::Failed,
            reason: e.to_string(),
        })?;
        if self.control.is_cancelled() {
            return Err(Self::cancelled());
        }
        let total = self.add_usage(reply.usage);
        self.record_event(
            EventEnvelope::new(
                self.session,
                "model.replied",
                EffectClass::Network,
                format!("{} wrote the answer", self.model_name),
                json!({"model": reply.model, "purpose": "synthesis", "chars": reply.text.len(), "finish_reason": reply.finish_reason}),
            )
            .with_turn(self.turn)
            .with_usage(reply.usage.to_delta()),
        )
        .map_err(Self::log_stop)?;
        let done = finalize(&reply.text, &merged.sources);
        Self::go(fsm, TurnState::Verifying)?;
        if total.total() > plan.budget.tokens {
            // The answer is kept; the report shows the overrun.
            let _ = self.record(
                "budget.noted",
                format!(
                    "The run used {} tokens, over its {}",
                    total.total(),
                    plan.budget.tokens
                ),
                json!({"used": total.total(), "budget": plan.budget.tokens}),
            );
        }
        Ok(if done.text.trim().is_empty() {
            (TurnState::CompletedWithGaps, done.text)
        } else {
            (TurnState::Completed, done.text)
        })
    }

    /// One worker: search, read, note, at most [`ROUNDS`] times, within
    /// its slice of the budget. Always returns its notes, possibly empty.
    async fn worker(&self, question: &str, sub: &SubQuestion, budget: WorkerBudget) -> WorkerNotes {
        let n = sub.n;
        let started = Instant::now();
        let mut st = WorkerState::default();
        let result = if self.control.worker_cancelled(n) {
            Err(Halt::Cancelled)
        } else {
            match self.record(
                "worker.started",
                format!("Worker {n} started: {}", sub.question),
                json!({"worker": n, "question": sub.question, "budget": budget}),
            ) {
                Ok(()) => {
                    let deadline =
                        tokio::time::Instant::now() + Duration::from_secs(budget.seconds.max(1));
                    self.work(question, sub, budget, deadline, &mut st).await
                }
                Err(e) => Err(Halt::Failed(e)),
            }
        };
        let (status, note) = match result {
            Ok(()) if st.notes.trim().is_empty() => {
                (WorkerStatus::Failed, "the model wrote no notes".to_owned())
            }
            Ok(()) => (WorkerStatus::Done, String::new()),
            Err(Halt::Cancelled) => (WorkerStatus::Cancelled, "you stopped it".to_owned()),
            Err(Halt::TimedOut) => (
                WorkerStatus::TimedOut,
                format!("its {} s ran out", budget.seconds),
            ),
            Err(Halt::OverBudget(why)) => (WorkerStatus::OverBudget, why),
            Err(Halt::Failed(why)) => (WorkerStatus::Failed, why),
        };
        let pages: Vec<&PageRef> = st.pages.iter().map(|p| &p.page).collect();
        let took_ms = started.elapsed().as_millis() as u64;
        let _ = self.record(
            "worker.finished",
            match status {
                WorkerStatus::Done => format!("Worker {n} finished: {} pages read", pages.len()),
                _ => format!("Worker {n} stopped: {note}"),
            },
            json!({
                "worker": n,
                "question": sub.question,
                "status": status,
                "note": note,
                "notes": st.notes,
                "pages": pages,
                "searches": st.searches,
                "tokens": {"input": st.tokens.0, "output": st.tokens.1},
                "budget": budget,
                "took_ms": took_ms,
            }),
        );
        WorkerNotes {
            worker: n,
            question: sub.question.clone(),
            status,
            notes: st.notes,
            sources: st
                .pages
                .iter()
                .map(|p| NoteSource {
                    url: p.page.url.clone(),
                    title: p.page.title.clone(),
                    site: p.page.site.clone(),
                })
                .collect(),
        }
    }

    async fn work(
        &self,
        question: &str,
        sub: &SubQuestion,
        budget: WorkerBudget,
        deadline: tokio::time::Instant,
        st: &mut WorkerState,
    ) -> Result<(), Halt> {
        let n = sub.n;
        let mut queue: VecDeque<String> = sub.queries.iter().cloned().collect();
        for round in 0..ROUNDS {
            // The planned searches first; later rounds run the one the
            // worker asked for.
            let queries: Vec<String> = if round == 0 {
                queue.drain(..).collect()
            } else {
                queue.pop_front().into_iter().collect()
            };
            if queries.is_empty() {
                break;
            }
            let mut hits: Vec<SearchHit> = Vec::new();
            for q in queries {
                let args = json!({"query": q});
                self.allow("web_search", Some(self.search.id()), &args)?;
                let found = self
                    .guard(Some(n), Some(deadline), self.search.search(&q, RESULTS))
                    .await?;
                st.searches += 1;
                self.count(0, 1);
                match found {
                    Ok(h) => {
                        self.record_event(
                            EventEnvelope::new(
                                self.session,
                                "tool.executed",
                                EffectClass::Network,
                                format!("Worker {n} searched: {q}"),
                                json!({"tool": "web_search", "args": args, "worker": n,
                                       "meta": {"query": q, "results": h.len(), "worker": n}}),
                            )
                            .with_turn(self.turn)
                            .with_evidence(h.iter().map(|x| x.url.clone())),
                        )
                        .map_err(Halt::Failed)?;
                        hits.extend(h);
                    }
                    Err(e) => {
                        self.record(
                            "tool.failed",
                            format!("Worker {n}: search failed"),
                            json!({"tool": "web_search", "args": args, "error": e, "worker": n}),
                        )
                        .map_err(Halt::Failed)?;
                    }
                }
            }
            let left = (budget.pages as usize).saturating_sub(st.pages.len());
            if left == 0 {
                return Err(Halt::OverBudget(format!(
                    "it read its {} pages",
                    budget.pages
                )));
            }
            let chosen = pick(&hits, left.min(READS_PER_ROUND), &st.seen);
            if chosen.is_empty() && st.pages.is_empty() {
                return Err(Halt::Failed(if hits.is_empty() {
                    "the searches found nothing".to_owned()
                } else {
                    "no result was new".to_owned()
                }));
            }
            let mut to_read = Vec::new();
            for h in chosen {
                st.seen.insert(h.url.clone());
                let args = json!({"url": h.url});
                self.allow("read_url", Some(&h.site), &args)?;
                to_read.push(h);
            }
            let cached: Vec<bool> = to_read.iter().map(|h| self.reader.cached(&h.url)).collect();
            let reads = self
                .guard(
                    Some(n),
                    Some(deadline),
                    futures::future::join_all(to_read.iter().map(|h| self.reader.read(&h.url))),
                )
                .await?;
            let mut new_pages = 0;
            for ((h, r), hit) in to_read.iter().zip(reads).zip(cached) {
                self.count(0, 1);
                let args = json!({"url": h.url});
                match r {
                    Ok(p) => {
                        if st.pages.iter().any(|q| q.page.url == p.page.url) {
                            continue;
                        }
                        self.record_event(
                            EventEnvelope::new(
                                self.session,
                                "tool.executed",
                                EffectClass::Network,
                                format!("Worker {n} read {}", p.page.site),
                                json!({"tool": "read_url", "args": args, "worker": n,
                                       "meta": {"page": p.page, "worker": n,
                                                "crawl": {"reader": "Tern Crawl", "robots": "allowed",
                                                          "cache": if hit { "hit" } else { "miss" }}}}),
                            )
                            .with_turn(self.turn)
                            .with_evidence([p.page.url.clone()]),
                        )
                        .map_err(Halt::Failed)?;
                        st.pages.push(p);
                        new_pages += 1;
                    }
                    Err(e) => {
                        self.record(
                            "tool.failed",
                            format!("Worker {n} could not read {}", h.site),
                            json!({"tool": "read_url", "args": args, "error": e, "worker": n}),
                        )
                        .map_err(Halt::Failed)?;
                    }
                }
            }
            if st.pages.is_empty() {
                return Err(Halt::Failed("no page could be read".into()));
            }
            if new_pages == 0 && round > 0 {
                break;
            }

            // Notes over every page read so far, cut to fit the slice.
            let spent = st.tokens.0 + st.tokens.1;
            let left = budget.tokens.saturating_sub(spent);
            if left < 1_500 {
                return Err(Halt::OverBudget(format!(
                    "it used {spent} of its {} tokens",
                    budget.tokens
                )));
            }
            let room = (left.saturating_sub(u64::from(NOTES_TOKENS) + 600) * 4 * 4 / 5) as usize;
            let per = (room / st.pages.len()).clamp(400, SOURCE_CHARS);
            let texts: Vec<String> = st
                .pages
                .iter()
                .map(|p| p.text.chars().take(per).collect())
                .collect();
            let numbered: Vec<NumberedText<'_>> = st
                .pages
                .iter()
                .zip(&texts)
                .enumerate()
                .map(|(i, (p, t))| NumberedText {
                    n: i + 1,
                    title: &p.page.title,
                    url: &p.page.url,
                    text: t,
                })
                .collect();
            let again = round + 1 < ROUNDS && st.pages.len() < budget.pages as usize;
            let (system, prompt) = notes_prompt(question, sub, &numbered, again);
            let mut req = ChatRequest::new(self.model_name, vec![ChatMessage::user(prompt)])
                .with_system(system)
                .with_max_tokens(NOTES_TOKENS);
            req.temperature = Some(0.2);
            self.count(1, 0);
            let reply = self
                .guard(Some(n), Some(deadline), self.model.complete(&req))
                .await?
                .map_err(|e| Halt::Failed(e.to_string()))?;
            self.add_usage(reply.usage);
            st.tokens.0 += reply.usage.input_tokens;
            st.tokens.1 += reply.usage.output_tokens;
            self.record_event(
                EventEnvelope::new(
                    self.session,
                    "model.replied",
                    EffectClass::Network,
                    format!("Worker {n} wrote notes"),
                    json!({"model": reply.model, "purpose": "notes", "worker": n, "chars": reply.text.len()}),
                )
                .with_turn(self.turn)
                .with_usage(reply.usage.to_delta()),
            )
            .map_err(Halt::Failed)?;
            let (notes, next) = parse_notes(&reply.text);
            if !notes.is_empty() {
                st.notes = notes;
            }
            let spent = st.tokens.0 + st.tokens.1;
            if spent > budget.tokens {
                return Err(Halt::OverBudget(format!(
                    "it used {spent} of its {} tokens",
                    budget.tokens
                )));
            }
            match next {
                Some(q) if again => queue.push_back(q),
                _ => break,
            }
        }
        Ok(())
    }

    fn finish(
        &self,
        mut fsm: TurnStateMachine,
        result: Result<(TurnState, String), Stop>,
    ) -> TurnOutcome {
        let (state, answer, reason) = match result {
            Ok((state, text)) => {
                let gap = (state == TurnState::CompletedWithGaps)
                    .then(|| "The model gave an empty answer.".to_owned());
                (state, Some(text), gap)
            }
            Err(stop) => (stop.state, None, Some(stop.reason)),
        };
        if fsm.state() != state {
            let _ = fsm.move_to(state);
        }
        let kind = match state {
            TurnState::BudgetExceeded => "budget.exceeded",
            TurnState::Cancelled => "run.cancelled",
            _ => "turn.finished",
        };
        let usage = *self.usage.lock().expect("usage lock");
        let (model_calls, tool_calls) = *self.calls.lock().expect("calls lock");
        let _ = self.record_event(
            EventEnvelope::new(
                self.session,
                kind,
                EffectClass::Analysis,
                reason.clone().unwrap_or_else(|| "Answered".into()),
                json!({
                    "state": state,
                    "mode": "deep",
                    "model_calls": model_calls,
                    "tool_calls": tool_calls,
                    "answer_chars": answer.as_ref().map(String::len),
                    "answer": answer,
                }),
            )
            .with_turn(self.turn)
            .with_usage(usage.to_delta()),
        );
        TurnOutcome {
            turn_id: self.turn,
            state: fsm.state(),
            answer,
            reason,
            states: fsm.history().to_vec(),
            usage,
            model_calls,
            tool_calls,
        }
    }
}

/// The effect class an event of this kind is logged under.
fn effect_of(kind: &str) -> EffectClass {
    match kind {
        "tool.failed" | "tool.executed" => EffectClass::Network,
        _ => EffectClass::Analysis,
    }
}

/// The question in a message, without attachments or a quoted reply.
pub fn question_of(message: &str) -> String {
    let (text, _) = crate::attach::split(message);
    let (_, text) = crate::reply::split(text);
    text.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// Up to `n` hits not read yet, different sites first.
fn pick(hits: &[SearchHit], n: usize, seen: &HashSet<String>) -> Vec<SearchHit> {
    let mut chosen: Vec<SearchHit> = Vec::new();
    let mut sites = HashSet::new();
    let fresh = |h: &&SearchHit| !seen.contains(&h.url);
    for h in hits.iter().filter(fresh) {
        if chosen.len() < n
            && !chosen.iter().any(|c| c.url == h.url)
            && sites.insert(h.site.clone())
        {
            chosen.push(h.clone());
        }
    }
    for h in hits.iter().filter(fresh) {
        if chosen.len() < n && !chosen.iter().any(|c| c.url == h.url) {
            chosen.push(h.clone());
        }
    }
    chosen
}

#[cfg(test)]
mod tests;
