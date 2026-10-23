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
//! worker's full context.
//!
//! The run saves a checkpoint ([`crate::checkpoint`]) when the plan is
//! approved, after each worker's reads and notes, when a worker ends, and
//! before the answer; [`resume_deep`] picks a stopped run up from it. Steering
//! notes ([`crate::steer`]) are taken in while workers run: guidance for the
//! notes and the answer, or a worker dropped, added, or redirected. Each worker can be cancelled on its own
//! ([`DeepControl::cancel_worker`]) and the whole run at once. The claim
//! checks and run report of an ordinary turn then run on the answer.

use std::collections::{HashMap, HashSet, VecDeque};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use futures::StreamExt;
use futures::stream::FuturesUnordered;
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

use tern_research::ledger::LedgerReason;
use tern_research::steer::{SteerAction, parse_steer};

use crate::checkpoint::{
    CHECKPOINT_VERSION, CheckpointSink, DROPPED_BY_STEER, DeepState, RunCheckpoint, RunMode,
    SavedPage, Stage, WorkerCheckpoint, now_rfc3339,
};

/// How a worker's note starts when every search it ran failed.
pub const SEARCH_FAILED: &str = "search failed: ";
use crate::engine::{Budget, CancelFlag, EventSink, Model, OnText, TurnOutcome};
use crate::policy::{CallView, Decision, PolicyGate};
use crate::sources::PageRef;
use crate::steer::{Steering, guidance_lines};
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
    /// Where the run saves its checkpoints; `None` saves none.
    pub checkpoints: Option<&'a dyn CheckpointSink>,
    /// Steering notes from the user; `None` takes none.
    pub steering: Option<&'a Steering>,
    /// Ranks each worker's results and orders the workers; `None` keeps
    /// search order and plan order.
    pub decider: Option<&'a crate::decide::Decider>,
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

/// What a run used before it was resumed.
#[derive(Clone, Copy, Debug, Default)]
struct Prior {
    tokens_in: u64,
    tokens_out: u64,
    model_calls: u32,
    tool_calls: u32,
}

/// Most sub-questions a plan may grow to through steering.
pub const MAX_STEERED_SUB_QUESTIONS: usize = MAX_SUB_QUESTIONS + 2;

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
    checkpoints: Option<&'r dyn CheckpointSink>,
    steering: Option<&'r Steering>,
    decider: Option<&'r crate::decide::Decider>,
    /// The live checkpoint, once the plan is approved.
    ck: Mutex<Option<RunCheckpoint>>,
    prior: Prior,
    save_failed: AtomicBool,
    /// Steering guidance for every notes call still to come and the answer.
    guidance: Mutex<Vec<String>>,
    /// Redirects waiting for a running worker's next round.
    redirects: Mutex<HashMap<usize, Vec<String>>>,
    /// Workers a steer dropped.
    dropped: Mutex<HashSet<usize>>,
    /// What each applied steer did.
    applied: Mutex<Vec<String>>,
}

/// What a worker has so far.
#[derive(Default)]
struct WorkerState {
    pages: Vec<ReadPage>,
    seen: HashSet<String>,
    searches: usize,
    tokens: (u64, u64),
    notes: String,
    /// Rounds whose notes are written.
    round: usize,
    /// This round's pages are read; its notes are next.
    pending_notes: bool,
    queue: VecDeque<String>,
    /// Redirects from steering, in order.
    steered: Vec<String>,
    extra_rounds: usize,
    /// Time spent before a resume.
    took_before: u64,
}

impl WorkerState {
    fn fresh(sub: &SubQuestion) -> Self {
        Self {
            queue: sub.queries.iter().cloned().collect(),
            ..Self::default()
        }
    }

    fn restored(w: &WorkerCheckpoint) -> Self {
        Self {
            pages: w
                .pages
                .iter()
                .map(|p| ReadPage {
                    page: p.page.clone(),
                    text: p.text.clone(),
                })
                .collect(),
            seen: w.seen.iter().cloned().collect(),
            searches: w.searches,
            tokens: (w.tokens_in, w.tokens_out),
            notes: w.notes.clone(),
            round: w.round,
            pending_notes: w.pending_notes,
            queue: w.queue.iter().cloned().collect(),
            steered: w.steered.clone(),
            extra_rounds: w.extra_rounds,
            took_before: w.took_ms,
        }
    }
}

/// A worker waiting for a slot.
struct Job {
    sub: SubQuestion,
    budget: WorkerBudget,
    saved: Option<WorkerCheckpoint>,
}

/// How a run begins: with a plan to approve, or from a checkpoint.
enum Start<'a> {
    Plan(&'a dyn PlanApprover),
    Resume(Box<RunCheckpoint>),
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
    run(deps, message, opts, Start::Plan(approver), control).await
}

/// Resumes a deep run from its checkpoint. The approved plan is kept (it
/// is not put to the user again), finished workers keep their notes, and
/// unfinished ones go on from their last round with the pages they had
/// read. Steering guidance received before the stop still applies.
pub async fn resume_deep(
    deps: DeepDeps<'_>,
    checkpoint: RunCheckpoint,
    opts: &DeepOptions,
    control: &DeepControl,
) -> TurnOutcome {
    let message = checkpoint.message.clone();
    run(
        deps,
        &message,
        opts,
        Start::Resume(Box::new(checkpoint)),
        control,
    )
    .await
}

async fn run(
    deps: DeepDeps<'_>,
    message: &str,
    opts: &DeepOptions,
    start: Start<'_>,
    control: &DeepControl,
) -> TurnOutcome {
    let prior = match &start {
        Start::Resume(c) => Prior {
            tokens_in: c.tokens_in,
            tokens_out: c.tokens_out,
            model_calls: c.model_calls,
            tool_calls: c.tool_calls,
        },
        Start::Plan(_) => Prior::default(),
    };
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
        checkpoints: deps.checkpoints,
        steering: deps.steering,
        decider: deps.decider,
        ck: Mutex::new(None),
        prior,
        save_failed: AtomicBool::new(false),
        guidance: Mutex::default(),
        redirects: Mutex::default(),
        dropped: Mutex::default(),
        applied: Mutex::default(),
    };
    let mut fsm = TurnStateMachine::default();
    let result = deep.drive(&mut fsm, message, opts, start).await;
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

    /// Tokens used by the run, before a resume included.
    fn used(&self) -> u64 {
        self.usage.lock().expect("usage lock").total()
            + self.prior.tokens_in
            + self.prior.tokens_out
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

    /// Changes the live checkpoint with `f` and saves it. Nothing happens
    /// before the plan is approved or when the run saves no checkpoints;
    /// a failed save is logged once and the run goes on.
    fn save_with(&self, f: impl FnOnce(&mut RunCheckpoint)) {
        let Some(sink) = self.checkpoints else {
            return;
        };
        let mut ck = self.ck.lock().expect("checkpoint lock");
        let Some(c) = ck.as_mut() else {
            return;
        };
        f(c);
        let usage = *self.usage.lock().expect("usage lock");
        let (model_calls, tool_calls) = *self.calls.lock().expect("calls lock");
        c.tokens_in = self.prior.tokens_in + usage.input_tokens;
        c.tokens_out = self.prior.tokens_out + usage.output_tokens;
        c.model_calls = self.prior.model_calls + model_calls;
        c.tool_calls = self.prior.tool_calls + tool_calls;
        c.guidance = self.guidance.lock().expect("guidance lock").clone();
        c.steers = self.applied.lock().expect("steer lock").clone();
        c.saved_at = now_rfc3339();
        if let Err(e) = sink.save(c)
            && !self.save_failed.swap(true, Ordering::SeqCst)
        {
            let _ = self.record(
                "checkpoint.failed",
                "Tern could not save a checkpoint; the run goes on, but could not be resumed from here",
                json!({"error": e}),
            );
        }
    }

    /// Starts the checkpoint once the plan is approved.
    fn begin_checkpoint(&self, message: &str, plan: &ResearchPlan, concurrency: usize) {
        if self.checkpoints.is_none() {
            return;
        }
        let budgets = plan.worker_budgets(concurrency);
        let workers = plan
            .sub_questions
            .iter()
            .zip(budgets)
            .map(|(s, b)| WorkerCheckpoint {
                n: s.n,
                question: s.question.clone(),
                budget: b,
                queue: s.queries.clone(),
                ..WorkerCheckpoint::default()
            })
            .collect();
        *self.ck.lock().expect("checkpoint lock") = Some(RunCheckpoint {
            version: CHECKPOINT_VERSION,
            session: self.session.to_string(),
            turn: self.turn.to_string(),
            mode: RunMode::Deep,
            message: message.to_owned(),
            model: self.model_name.to_owned(),
            stage: Stage::PlanApproved,
            deep: Some(DeepState {
                plan: plan.clone(),
                concurrency,
                workers,
            }),
            ..RunCheckpoint::default()
        });
        self.save_with(|_| {});
    }

    /// Saves one worker's progress, and how it ended when it did.
    fn save_worker(
        &self,
        sub: &SubQuestion,
        budget: WorkerBudget,
        st: &WorkerState,
        finished: Option<(WorkerStatus, &str)>,
        took_ms: u64,
    ) {
        self.save_with(|c| {
            if c.stage < Stage::Workers {
                c.stage = Stage::Workers;
            }
            let d = c.deep.get_or_insert_with(DeepState::default);
            let i = match d.workers.iter().position(|w| w.n == sub.n) {
                Some(i) => i,
                None => {
                    d.workers.push(WorkerCheckpoint {
                        n: sub.n,
                        ..WorkerCheckpoint::default()
                    });
                    d.workers.len() - 1
                }
            };
            let w = &mut d.workers[i];
            w.question = sub.question.clone();
            w.budget = budget;
            w.finished = finished.map(|f| f.0);
            w.note = finished.map(|f| f.1.to_owned()).unwrap_or_default();
            w.round = st.round;
            w.pending_notes = st.pending_notes;
            w.queue = st.queue.iter().cloned().collect();
            w.searches = st.searches;
            w.tokens_in = st.tokens.0;
            w.tokens_out = st.tokens.1;
            w.notes = st.notes.clone();
            w.pages = st
                .pages
                .iter()
                .map(|p| SavedPage {
                    page: p.page.clone(),
                    text: p.text.chars().take(SOURCE_CHARS).collect(),
                })
                .collect();
            let mut seen: Vec<String> = st.seen.iter().cloned().collect();
            seen.sort();
            w.seen = seen;
            w.took_ms = took_ms;
            w.steered = st.steered.clone();
            w.extra_rounds = st.extra_rounds;
        });
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

    /// Search results come from the user's own provider: their domains
    /// are known to the gate.
    fn know_listed(&self, hits: &[SearchHit]) {
        if let Some(g) = self.gate.lock().expect("gate lock").as_mut() {
            for h in hits {
                g.know_text(&h.url);
            }
        }
    }

    /// A page is about to be read: outside content enters the session.
    fn saw_pages(&self) {
        if let Some(g) = self.gate.lock().expect("gate lock").as_mut() {
            g.saw_untrusted();
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
        start: Start<'_>,
    ) -> Result<(TurnState, String), Stop> {
        Self::go(fsm, TurnState::Preparing)?;
        let (approver, resume) = match start {
            Start::Plan(a) => (Some(a), None),
            Start::Resume(c) => (None, Some(*c)),
        };
        let concurrency = resume
            .as_ref()
            .and_then(|c| c.deep.as_ref())
            .map(|d| d.concurrency)
            .unwrap_or(opts.workers)
            .clamp(1, MAX_WORKERS);
        let mut started = json!({
            "model": self.model_name,
            "provider": self.model.provider_id(),
            "mode": "deep",
            "workers": concurrency,
            "tools": ["web_search", "read_url"],
            "budget": opts.budget,
            "message": message,
        });
        if let Some(c) = &resume {
            started["resumed"] = json!({"turn": c.turn, "stage": c.stage, "saved_at": c.saved_at});
        }
        self.record(
            "turn.started",
            if resume.is_some() {
                "Resumed a deep research turn"
            } else {
                "Started a deep research turn"
            },
            started,
        )
        .map_err(Self::log_stop)?;
        let question = question_of(message);
        if question.is_empty() {
            return Err(Stop {
                state: TurnState::Failed,
                reason: "Deep research needs a question.".into(),
            });
        }

        let (mut plan, restored) = match (resume, approver) {
            (Some(c), _) => {
                Self::go(fsm, TurnState::ModelStreaming)?;
                Self::go(fsm, TurnState::ToolRequested)?;
                Self::go(fsm, TurnState::PolicyCheck)?;
                Self::go(fsm, TurnState::ApprovalRequested)?;
                self.restore(c)?
            }
            (None, Some(approver)) => {
                self.preflight(&question).await?;
                let plan = self
                    .plan_and_approve(fsm, &question, opts, approver, concurrency)
                    .await?;
                self.begin_checkpoint(message, &plan, concurrency);
                (plan, Vec::new())
            }
            (None, None) => unreachable!("a run starts from a plan or a checkpoint"),
        };
        if self.control.is_cancelled() {
            return Err(Self::cancelled());
        }
        Self::go(fsm, TurnState::Executing)?;

        // 3. Workers, up to `concurrency` at once. Steering is taken in
        // between: before any starts, whenever one finishes, and every
        // 40 ms while they run.
        let mut done: Vec<WorkerNotes> = restored
            .iter()
            .filter(|w| w.finished.is_some())
            .map(notes_from_checkpoint)
            .collect();
        let mut pending: VecDeque<Job> = if restored.is_empty() {
            plan.sub_questions
                .iter()
                .cloned()
                .zip(plan.worker_budgets(concurrency))
                .map(|(sub, budget)| Job {
                    sub,
                    budget,
                    saved: None,
                })
                .collect()
        } else {
            restored
                .iter()
                .filter(|w| w.finished.is_none())
                .map(|w| Job {
                    sub: plan
                        .sub_questions
                        .iter()
                        .find(|s| s.n == w.n)
                        .cloned()
                        .unwrap_or_else(|| SubQuestion {
                            n: w.n,
                            question: w.question.clone(),
                            intent: String::new(),
                            queries: vec![w.question.clone()],
                            sources: vec![SourceKind::Any],
                            share: 0,
                        }),
                    budget: w.budget,
                    saved: Some(w.clone()),
                })
                .collect()
        };
        // Now or later: with more sub-questions than slots, the decision
        // model picks which start first. Worker numbers stay as planned.
        if restored.is_empty()
            && pending.len() > concurrency
            && let Some(d) = self.decider
        {
            let subs: Vec<String> = pending.iter().map(|j| j.sub.question.clone()).collect();
            if let Some((order, dec)) =
                crate::decide::order_sub_questions(d, &question, &subs).await
            {
                let mut old: Vec<Option<Job>> = pending.drain(..).map(Some).collect();
                pending = order.iter().filter_map(|&i| old[i].take()).collect();
                pending.extend(old.into_iter().flatten());
                let _ =
                    self.record_event(crate::decide::event(self.session, Some(self.turn), &dec));
            }
        }
        let mut running = FuturesUnordered::new();
        let mut running_ids: Vec<usize> = Vec::new();
        loop {
            self.steer_tick(&mut plan, &mut pending, &mut done, &running_ids)
                .map_err(Self::log_stop)?;
            if self.control.is_cancelled() {
                break;
            }
            while running.len() < concurrency {
                let Some(job) = pending.pop_front() else {
                    break;
                };
                running_ids.push(job.sub.n);
                running.push(self.worker(&question, job.sub, job.budget, job.saved));
            }
            if running.is_empty() {
                break;
            }
            tokio::select! {
                Some(w) = running.next() => {
                    running_ids.retain(|n| *n != w.worker);
                    done.push(w);
                }
                _ = tokio::time::sleep(Duration::from_millis(40)) => {}
            }
        }
        // A stopped run still lets each worker log how it ended.
        while let Some(w) = running.next().await {
            done.push(w);
        }
        let mut notes = done;
        notes.sort_by_key(|w| w.worker);
        if self.control.is_cancelled() {
            return Err(Self::cancelled());
        }
        Self::go(fsm, TurnState::RecordResult)?;

        // 4. One answer from every worker's notes.
        let merged = merge(&notes);
        if merged.sources.is_empty() {
            return Err(Stop {
                state: TurnState::Failed,
                reason: self.no_sources_reason(&notes),
            });
        }
        let used = self.used();
        if used >= plan.budget.tokens {
            return Err(Stop {
                state: TurnState::BudgetExceeded,
                reason: format!(
                    "The workers used {used} tokens of the run's {}, leaving none for the answer.",
                    plan.budget.tokens
                ),
            });
        }
        self.save_with(|c| c.stage = Stage::Synthesis);
        Self::go(fsm, TurnState::ModelStreaming)?;
        let (system, mut prompt) = synthesis_prompt(&question, &merged);
        prompt.push_str(&guidance_lines(
            &self.guidance.lock().expect("guidance lock"),
        ));
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
        self.too_late();
        let done = finalize(&reply.text, &merged.sources);
        Self::go(fsm, TurnState::Verifying)?;
        let total = total.total() + self.prior.tokens_in + self.prior.tokens_out;
        if total > plan.budget.tokens {
            // The answer is kept; the report shows the overrun.
            let _ = self.record(
                "budget.noted",
                format!(
                    "The run used {} tokens, over its {}",
                    total, plan.budget.tokens
                ),
                json!({"used": total, "budget": plan.budget.tokens}),
            );
        }
        Ok(if done.text.trim().is_empty() {
            (TurnState::CompletedWithGaps, done.text)
        } else {
            (TurnState::Completed, done.text)
        })
    }

    /// One real search before the plan costs a model call: a search
    /// service that does not answer stops the run here, with its own error.
    async fn preflight(&self, question: &str) -> Result<(), Stop> {
        let q: String = question.chars().take(200).collect();
        let args = json!({"query": q, "preflight": true});
        if self
            .allow("web_search", Some(self.search.id()), &args)
            .is_err()
        {
            // The gate decides per search later; nothing to check here.
            return Ok(());
        }
        let found = self
            .guard(None, None, self.search.search(&q, 1))
            .await
            .map_err(|_| Self::cancelled())?;
        match found {
            Ok(h) => {
                let _ = self.record(
                    "search.checked",
                    format!(
                        "Web search answered ({} result{})",
                        h.len(),
                        if h.len() == 1 { "" } else { "s" }
                    ),
                    json!({"service": self.search.id(), "results": h.len()}),
                );
                Ok(())
            }
            Err(e) => {
                let _ = self.record(
                    "tool.failed",
                    format!("Web search isn't working: {e}"),
                    json!({"tool": "web_search", "args": args, "error": e, "preflight": true}),
                );
                Err(Stop {
                    state: TurnState::Failed,
                    reason: format!(
                        "Web search isn't working, so deep research did not start: {}. Fix it in Providers › Search, or turn off Deep research to answer without the web.",
                        e.trim_end_matches('.')
                    ),
                })
            }
        }
    }

    /// Why a run with no source to cite failed, in words that point at the
    /// fix: the search service's own error when every search failed.
    fn no_sources_reason(&self, notes: &[WorkerNotes]) -> String {
        let ck = self.ck.lock().expect("checkpoint lock");
        let note_of = |n: usize| -> String {
            ck.as_ref()
                .and_then(|c| c.deep.as_ref())
                .and_then(|d| d.workers.iter().find(|w| w.n == n))
                .map(|w| w.note.clone())
                .unwrap_or_default()
        };
        let notes_text: Vec<(usize, String)> = notes
            .iter()
            .map(|w| (w.worker, note_of(w.worker)))
            .collect();
        drop(ck);
        let search_failed: Vec<&str> = notes_text
            .iter()
            .filter_map(|(_, t)| t.strip_prefix(SEARCH_FAILED))
            .collect();
        if !notes_text.is_empty() && search_failed.len() == notes_text.len() {
            return format!(
                "No search service answered: {}. Fix web search in Providers › Search, then retry.",
                search_failed[0].trim_end_matches('.')
            );
        }
        let why: Vec<String> = notes
            .iter()
            .zip(&notes_text)
            .map(|(w, (_, t))| {
                let label = w.status.label().to_lowercase();
                if t.is_empty() {
                    format!("{}: {label}", w.worker)
                } else {
                    format!("{}: {label}, {t}", w.worker)
                }
            })
            .collect();
        format!(
            "No worker found a page to cite ({}), so there is nothing to write from.",
            why.join("; ")
        )
    }

    /// Steps 1 and 2: the plan, and the user's go-ahead. Nothing is
    /// searched or read before it.
    async fn plan_and_approve(
        &self,
        fsm: &mut TurnStateMachine,
        question: &str,
        opts: &DeepOptions,
        approver: &dyn PlanApprover,
        concurrency: usize,
    ) -> Result<ResearchPlan, Stop> {
        Self::go(fsm, TurnState::ModelStreaming)?;
        let (system, prompt) = plan_prompt(question, opts.max_sub_questions);
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
                match parse_plan(question, &r.text, opts.budget, opts.max_sub_questions) {
                    Ok(p) => (p, None),
                    Err(e) => (
                        fallback_plan(question, opts.budget, opts.max_sub_questions),
                        Some(format!(
                            "The model's plan could not be read ({e}), so Tern split the question itself."
                        )),
                    ),
                }
            }
            Err(e) => (
                fallback_plan(question, opts.budget, opts.max_sub_questions),
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
            PlanAnswer::Skip => (single_plan(question, opts.budget), "skip", false),
            PlanAnswer::Run(mut p) => {
                p.normalise(MAX_SUB_QUESTIONS);
                if p.sub_questions.is_empty() {
                    (single_plan(question, opts.budget), "skip", false)
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
        Ok(plan)
    }

    /// Takes a checkpoint back: logs the resume, the plan as approved, and
    /// each finished worker as it ended, so the run's views and report show
    /// the whole run.
    fn restore(&self, mut c: RunCheckpoint) -> Result<(ResearchPlan, Vec<WorkerCheckpoint>), Stop> {
        // Failed workers (and ones stopped with nothing kept) are not
        // finished work: they run again from the start.
        let mut retried = Vec::new();
        if let Some(d) = c.deep.as_mut() {
            for w in d.workers.iter_mut().filter(|w| w.retryable()) {
                retried.push(w.n);
                w.reset();
            }
            if !retried.is_empty() && c.stage > Stage::Workers {
                c.stage = Stage::Workers;
            }
        }
        let Some(d) = c.deep.clone() else {
            return Err(Stop {
                state: TurnState::Failed,
                reason: "That checkpoint is not a deep research run.".into(),
            });
        };
        *self.guidance.lock().expect("guidance lock") = c.guidance.clone();
        *self.applied.lock().expect("steer lock") = c.steers.clone();
        self.record(
            "run.resumed",
            format!("Picked up from the checkpoint: {}", c.progress()),
            json!({
                "turn": c.turn,
                "stage": c.stage,
                "saved_at": c.saved_at,
                "progress": c.progress(),
                "prior": {"input": c.tokens_in, "output": c.tokens_out,
                          "model_calls": c.model_calls, "tool_calls": c.tool_calls},
                "guidance": c.guidance,
                "resumes": c.resumes + 1,
                "retried": retried,
            }),
        )
        .map_err(Self::log_stop)?;
        self.record(
            "plan.answered",
            "Kept the plan you approved",
            json!({"answer": "run", "edited": false, "resumed": true, "plan": d.plan, "workers": d.concurrency}),
        )
        .map_err(Self::log_stop)?;
        for w in d.workers.iter().filter(|w| w.finished.is_some()) {
            let pages: Vec<&PageRef> = w.pages.iter().map(|p| &p.page).collect();
            self.record(
                "worker.finished",
                format!("Worker {} had finished; its notes were kept", w.n),
                json!({
                    "worker": w.n,
                    "question": w.question,
                    "status": w.finished,
                    "note": w.note,
                    "notes": w.notes,
                    "pages": pages,
                    "searches": w.searches,
                    "tokens": {"input": w.tokens_in, "output": w.tokens_out},
                    "budget": w.budget,
                    "took_ms": w.took_ms,
                    "restored": true,
                }),
            )
            .map_err(Self::log_stop)?;
        }
        let mut ck = c;
        ck.resumes += 1;
        ck.turn = self.turn.to_string();
        *self.ck.lock().expect("checkpoint lock") = Some(ck);
        self.save_with(|_| {});
        Ok((d.plan, d.workers))
    }

    /// Picks up steering notes and applies each (a safe point).
    fn steer_tick(
        &self,
        plan: &mut ResearchPlan,
        pending: &mut VecDeque<Job>,
        done: &mut Vec<WorkerNotes>,
        running: &[usize],
    ) -> Result<(), String> {
        let Some(inbox) = self.steering else {
            return Ok(());
        };
        let notes = inbox.take();
        if notes.is_empty() {
            return Ok(());
        }
        for note in notes {
            let action = parse_steer(&note.text);
            self.record(
                "steer.received",
                format!("You steered the run: {}", note.text),
                json!({"text": note.text, "action": action}),
            )?;
            self.apply(action, &note.text, plan, pending, done, running)?;
        }
        let plan = plan.clone();
        self.save_with(move |c| {
            if let Some(d) = c.deep.as_mut() {
                d.plan = plan;
            }
        });
        Ok(())
    }

    fn steer_applied(&self, summary: String, mut payload: Value) -> Result<(), String> {
        if payload["applied"] == true {
            self.applied
                .lock()
                .expect("steer lock")
                .push(summary.clone());
        }
        if payload.get("applied").is_none() {
            payload["applied"] = json!(true);
        }
        self.record("steer.applied", summary, payload)
    }

    fn apply(
        &self,
        action: SteerAction,
        text: &str,
        plan: &mut ResearchPlan,
        pending: &mut VecDeque<Job>,
        done: &mut Vec<WorkerNotes>,
        running: &[usize],
    ) -> Result<(), String> {
        let not_applied = |why: String| {
            self.steer_applied(
                format!("Not applied: {why}"),
                json!({"text": text, "applied": false, "why": why}),
            )
        };
        match action {
            SteerAction::Guide { text: g } => {
                self.guidance.lock().expect("guidance lock").push(g.clone());
                self.steer_applied(
                    format!("Applied: the workers' notes and the answer will follow “{g}”"),
                    json!({"text": text, "applied": true, "how": "guidance"}),
                )
            }
            SteerAction::Drop { n } => {
                if let Some(i) = pending.iter().position(|j| j.sub.n == n) {
                    let job = pending.remove(i).expect("position is in range");
                    let st = job
                        .saved
                        .as_ref()
                        .map(WorkerState::restored)
                        .unwrap_or_default();
                    let note = "dropped by your steer";
                    let pages: Vec<&PageRef> = st.pages.iter().map(|p| &p.page).collect();
                    self.record(
                        "worker.finished",
                        format!("Worker {n} stopped: {note}"),
                        json!({"worker": n, "question": job.sub.question, "status": WorkerStatus::Cancelled,
                               "note": note, "notes": st.notes, "pages": pages, "searches": st.searches,
                               "tokens": {"input": st.tokens.0, "output": st.tokens.1},
                               "budget": job.budget, "took_ms": st.took_before}),
                    )?;
                    self.save_worker(
                        &job.sub,
                        job.budget,
                        &st,
                        Some((WorkerStatus::Cancelled, note)),
                        st.took_before,
                    );
                    done.push(WorkerNotes {
                        worker: n,
                        question: job.sub.question.clone(),
                        status: WorkerStatus::Cancelled,
                        notes: st.notes.clone(),
                        sources: note_sources(&st.pages),
                    });
                    self.steer_applied(
                        format!("Applied: dropped sub-question {n} before it started"),
                        json!({"text": text, "applied": true, "how": "drop", "dropped": n}),
                    )
                } else if running.contains(&n) {
                    self.dropped.lock().expect("dropped lock").insert(n);
                    self.control.cancel_worker(n);
                    self.steer_applied(
                        format!("Applied: stopping worker {n}; its notes so far are kept"),
                        json!({"text": text, "applied": true, "how": "drop", "dropped": n}),
                    )
                } else if done.iter().any(|w| w.worker == n) {
                    not_applied(format!("sub-question {n} had already finished"))
                } else {
                    not_applied(format!("there is no sub-question {n}"))
                }
            }
            SteerAction::Add { question } => self.add_worker(text, &question, None, plan, pending),
            SteerAction::Redirect { n, to } => {
                if let Some(job) = pending.iter_mut().find(|j| j.sub.n == n) {
                    if job
                        .saved
                        .as_ref()
                        .is_some_and(WorkerCheckpoint::has_progress)
                    {
                        // A resumed worker keeps what it read; the new
                        // direction is its next search.
                        self.redirects
                            .lock()
                            .expect("redirect lock")
                            .entry(n)
                            .or_default()
                            .push(to);
                        return Ok(());
                    }
                    job.sub.question = to.clone();
                    job.sub.queries = vec![to.chars().take(380).collect()];
                    job.sub.intent.clear();
                    job.saved = None;
                    if let Some(s) = plan.sub_questions.iter_mut().find(|s| s.n == n) {
                        *s = job.sub.clone();
                    }
                    self.steer_applied(
                        format!("Applied: sub-question {n} now asks “{to}”"),
                        json!({"text": text, "applied": true, "how": "redirect",
                               "redirected": {"n": n, "question": to}}),
                    )
                } else if running.contains(&n) {
                    // Taken in at the worker's next round, which logs it.
                    self.redirects
                        .lock()
                        .expect("redirect lock")
                        .entry(n)
                        .or_default()
                        .push(to);
                    Ok(())
                } else if done.iter().any(|w| w.worker == n) {
                    self.add_worker(text, &to, Some(n), plan, pending)
                } else {
                    not_applied(format!("there is no sub-question {n}"))
                }
            }
        }
    }

    /// Adds a worker for `question` with a slice like the others'; the
    /// run's budget grows by that slice, and the log says so.
    fn add_worker(
        &self,
        text: &str,
        question: &str,
        instead_of: Option<usize>,
        plan: &mut ResearchPlan,
        pending: &mut VecDeque<Job>,
    ) -> Result<(), String> {
        if plan.sub_questions.len() >= MAX_STEERED_SUB_QUESTIONS {
            return self.steer_applied(
                format!(
                    "Not applied: the plan already has {} sub-questions",
                    plan.sub_questions.len()
                ),
                json!({"text": text, "applied": false, "why": "too many sub-questions"}),
            );
        }
        let n = plan.sub_questions.iter().map(|s| s.n).max().unwrap_or(0) + 1;
        let parts = plan.sub_questions.len() as u64 + 1;
        let budget = WorkerBudget {
            tokens: (plan.budget.tokens / parts).max(4_000),
            pages: ((u64::from(plan.budget.pages) / parts) as u32).max(1),
            seconds: (plan.budget.seconds / 2)
                .clamp(30.min(plan.budget.seconds), plan.budget.seconds.max(1)),
        };
        plan.budget.tokens += budget.tokens;
        plan.budget.pages += budget.pages;
        let sub = SubQuestion {
            n,
            question: question.to_owned(),
            intent: String::new(),
            queries: vec![question.chars().take(380).collect()],
            sources: vec![SourceKind::Any],
            share: 0,
        };
        plan.sub_questions.push(sub.clone());
        self.save_worker(&sub, budget, &WorkerState::fresh(&sub), None, 0);
        pending.push_back(Job {
            sub,
            budget,
            saved: None,
        });
        let summary = match instead_of {
            Some(m) => format!(
                "Applied: sub-question {m} had finished, so “{question}” runs as sub-question {n}"
            ),
            None => format!("Applied: added sub-question {n}, “{question}”"),
        };
        self.steer_applied(
            summary,
            json!({"text": text, "applied": true, "how": "add",
                   "added": {"n": n, "question": question, "budget": budget},
                   "budget_added": {"tokens": budget.tokens, "pages": budget.pages}}),
        )
    }

    fn take_redirects(&self, n: usize) -> Vec<String> {
        self.redirects
            .lock()
            .expect("redirect lock")
            .remove(&n)
            .unwrap_or_default()
    }

    /// Notes that arrive while the answer is written are too late for it.
    fn too_late(&self) {
        let Some(inbox) = self.steering else {
            return;
        };
        for note in inbox.take() {
            let _ = self.record(
                "steer.received",
                format!("You steered the run: {}", note.text),
                json!({"text": note.text, "action": parse_steer(&note.text)}),
            );
            let _ = self.steer_applied(
                "Not applied: the answer was already being written; send it as a new message"
                    .into(),
                json!({"text": note.text, "applied": false, "why": "too late"}),
            );
        }
    }

    /// One worker: search, read, note, at most [`ROUNDS`] times (one more
    /// when steering redirects it), within its slice of the budget. Always
    /// returns its notes, possibly empty.
    async fn worker(
        &self,
        question: &str,
        sub: SubQuestion,
        budget: WorkerBudget,
        saved: Option<WorkerCheckpoint>,
    ) -> WorkerNotes {
        let n = sub.n;
        let started = Instant::now();
        let resumed = saved.as_ref().is_some_and(WorkerCheckpoint::has_progress);
        let mut st = match &saved {
            Some(w) if resumed => WorkerState::restored(w),
            _ => WorkerState::fresh(&sub),
        };
        let result = if self.control.worker_cancelled(n) {
            Err(Halt::Cancelled)
        } else {
            let mut payload = json!({"worker": n, "question": sub.question, "budget": budget});
            if resumed {
                let pages: Vec<&PageRef> = st.pages.iter().map(|p| &p.page).collect();
                payload["restored"] = json!({
                    "pages": pages,
                    "searches": st.searches,
                    "tokens": {"input": st.tokens.0, "output": st.tokens.1},
                    "round": st.round,
                });
            }
            let line = if resumed {
                format!("Worker {n} picked up where it stopped: {}", sub.question)
            } else {
                format!("Worker {n} started: {}", sub.question)
            };
            match self.record("worker.started", line, payload) {
                Ok(()) => {
                    let left_ms = (budget.seconds.max(1) * 1_000)
                        .saturating_sub(st.took_before)
                        .max(1_000);
                    let deadline = tokio::time::Instant::now() + Duration::from_millis(left_ms);
                    self.save_worker(&sub, budget, &st, None, st.took_before);
                    self.work(question, &sub, budget, deadline, &mut st, started)
                        .await
                }
                Err(e) => Err(Halt::Failed(e)),
            }
        };
        let dropped = self.dropped.lock().map(|d| d.contains(&n)).unwrap_or(false);
        let (status, note) = match result {
            Ok(()) if st.notes.trim().is_empty() => {
                (WorkerStatus::Failed, "the model wrote no notes".to_owned())
            }
            Ok(()) => (WorkerStatus::Done, String::new()),
            Err(Halt::Cancelled) if dropped => {
                (WorkerStatus::Cancelled, DROPPED_BY_STEER.to_owned())
            }
            Err(Halt::Cancelled) => (WorkerStatus::Cancelled, "you stopped it".to_owned()),
            Err(Halt::TimedOut) => (
                WorkerStatus::TimedOut,
                format!("its {} s ran out", budget.seconds),
            ),
            Err(Halt::OverBudget(why)) => (WorkerStatus::OverBudget, why),
            Err(Halt::Failed(why)) => (WorkerStatus::Failed, why),
        };
        for to in self.take_redirects(n) {
            let _ = self.steer_applied(
                format!("Not applied: worker {n} finished before it could take “{to}”; add it as a new sub-question"),
                json!({"text": to, "applied": false, "worker": n, "why": "worker finished"}),
            );
        }
        let pages: Vec<&PageRef> = st.pages.iter().map(|p| &p.page).collect();
        let took_ms = st.took_before + started.elapsed().as_millis() as u64;
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
        // Stopping the whole run leaves the worker to go on from its last
        // safe point on a resume, so that save is kept as it was.
        let whole_run =
            status == WorkerStatus::Cancelled && self.control.is_cancelled() && !dropped;
        if !whole_run {
            self.save_worker(&sub, budget, &st, Some((status, note.as_str())), took_ms);
        }
        WorkerNotes {
            worker: n,
            question: sub.question.clone(),
            status,
            notes: st.notes,
            sources: note_sources(&st.pages),
        }
    }

    /// Logs the results a round left unread on purpose, for the ledger.
    fn note_skips(
        &self,
        n: usize,
        hits: &[SearchHit],
        chosen: &[SearchHit],
        seen: &HashSet<String>,
        left: usize,
    ) -> Result<(), Halt> {
        let mut skipped = Vec::new();
        let mut listed = HashSet::new();
        for h in hits {
            if chosen.iter().any(|c| c.url == h.url) || !listed.insert(h.url.clone()) {
                continue;
            }
            let reason = if seen.contains(&h.url) {
                LedgerReason::Duplicate
            } else if left == 0 || (chosen.len() >= left && left < READS_PER_ROUND) {
                LedgerReason::Budget
            } else {
                continue;
            };
            skipped.push(json!({"url": h.url, "site": h.site, "title": h.title, "reason": reason}));
        }
        if skipped.is_empty() {
            return Ok(());
        }
        self.record(
            "sources.skipped",
            format!(
                "Worker {n} left {} result{} unread",
                skipped.len(),
                if skipped.len() == 1 { "" } else { "s" }
            ),
            json!({"worker": n, "skipped": skipped}),
        )
        .map_err(Halt::Failed)
    }

    async fn work(
        &self,
        question: &str,
        sub: &SubQuestion,
        budget: WorkerBudget,
        deadline: tokio::time::Instant,
        st: &mut WorkerState,
        started: Instant,
    ) -> Result<(), Halt> {
        let n = sub.n;
        let took = |st: &WorkerState| st.took_before + started.elapsed().as_millis() as u64;
        while st.round < ROUNDS + st.extra_rounds {
            let round = st.round;
            if !st.pending_notes {
                // The planned searches first; later rounds run the one the
                // worker asked for, or a steer's redirect.
                // Taken off the queue only once the round's pages are read,
                // so a run stopped mid-round redoes the round on resume.
                let take = if round == 0 {
                    st.queue.len()
                } else {
                    st.queue.len().min(1)
                };
                let queries: Vec<String> = st.queue.iter().take(take).cloned().collect();
                if queries.is_empty() {
                    break;
                }
                let mut hits: Vec<SearchHit> = Vec::new();
                let mut search_ok = 0usize;
                let mut search_error: Option<String> = None;
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
                            search_ok += 1;
                            self.know_listed(&h);
                            let listed: Vec<Value> = h
                                .iter()
                                .map(|x| json!({"url": x.url, "title": x.title, "site": x.site}))
                                .collect();
                            self.record_event(
                                EventEnvelope::new(
                                    self.session,
                                    "tool.executed",
                                    EffectClass::Network,
                                    format!("Worker {n} searched: {q}"),
                                    json!({"tool": "web_search", "args": args, "worker": n,
                                           "meta": {"query": q, "results": h.len(), "worker": n, "hits": listed}}),
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
                                format!("Worker {n}: search failed: {e}"),
                                json!({"tool": "web_search", "args": args, "error": e, "worker": n}),
                            )
                            .map_err(Halt::Failed)?;
                            search_error.get_or_insert(e);
                        }
                    }
                }
                let left = (budget.pages as usize).saturating_sub(st.pages.len());
                if left == 0 {
                    self.note_skips(n, &hits, &[], &st.seen, 0)?;
                    return Err(Halt::OverBudget(format!(
                        "it read its {} pages",
                        budget.pages
                    )));
                }
                // The decision model ranks what to read first; pages this
                // worker already read are ruled out.
                let hits = match self.decider {
                    Some(d) if hits.len() > 1 => {
                        let seen = &st.seen;
                        let (ranked, dec) =
                            crate::decide::rank_hits(d, &sub.question, &hits, &|u: &str| {
                                seen.contains(u)
                            })
                            .await;
                        if let Some(dec) = dec {
                            let mut e = crate::decide::event(self.session, Some(self.turn), &dec);
                            e.payload["worker"] = json!(n);
                            self.record_event(e).map_err(Halt::Failed)?;
                        }
                        ranked
                    }
                    _ => hits,
                };
                let chosen = pick(&hits, left.min(READS_PER_ROUND), &st.seen);
                self.note_skips(n, &hits, &chosen, &st.seen, left)?;
                if chosen.is_empty() && st.pages.is_empty() {
                    return Err(Halt::Failed(if hits.is_empty() && search_ok == 0 {
                        match search_error {
                            Some(e) => format!("{SEARCH_FAILED}{e}"),
                            None => "no search ran".to_owned(),
                        }
                    } else if hits.is_empty() {
                        format!(
                            "the search{} found no results",
                            if search_ok == 1 { "" } else { "es" }
                        )
                    } else {
                        "no result was new".to_owned()
                    }));
                }
                let mut to_read = Vec::new();
                for h in chosen {
                    st.seen.insert(h.url.clone());
                    let args = json!({"url": h.url});
                    if let Err(e) = self.allow("read_url", Some(&h.site), &args) {
                        let _ = self.record(
                            "sources.skipped",
                            format!("Worker {n} was not allowed to read {}", h.site),
                            json!({"worker": n, "skipped": [{"url": h.url, "site": h.site, "title": h.title,
                                                               "reason": LedgerReason::Policy}]}),
                        );
                        return Err(e);
                    }
                    self.saw_pages();
                    to_read.push(h);
                }
                let cached: Vec<bool> =
                    to_read.iter().map(|h| self.reader.cached(&h.url)).collect();
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
                st.queue.drain(..take);
                if st.pages.is_empty() {
                    return Err(Halt::Failed("no page could be read".into()));
                }
                if new_pages == 0 && round > 0 {
                    break;
                }
                // Safe point: the round's pages are read.
                st.pending_notes = true;
                self.save_worker(sub, budget, st, None, took(st));
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
            let per = (room / st.pages.len().max(1)).clamp(400, SOURCE_CHARS);
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
            let again =
                round + 1 < ROUNDS + st.extra_rounds && st.pages.len() < budget.pages as usize;
            let (system, mut prompt) = notes_prompt(question, sub, &numbered, again);
            if let Some(to) = st.steered.last() {
                prompt.push_str(&format!(
                    "\n\nThe user redirected this sub-question while it ran: {to}. Focus your notes on that."
                ));
            }
            prompt.push_str(&guidance_lines(
                &self.guidance.lock().expect("guidance lock"),
            ));
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
            st.pending_notes = false;
            st.round += 1;

            // Safe point: a redirect from steering becomes the next search,
            // with one more round if the worker had none left.
            let taken = self.take_redirects(n);
            for to in &taken {
                st.queue.push_front(to.chars().take(380).collect());
                st.steered.push(to.clone());
                let _ = self.steer_applied(
                    format!("Applied: worker {n} now looks into “{to}”"),
                    json!({"text": to, "applied": true, "how": "redirect", "worker": n,
                           "redirected": {"n": n, "question": to}}),
                );
            }
            if !taken.is_empty() && st.round >= ROUNDS + st.extra_rounds && st.extra_rounds == 0 {
                st.extra_rounds = 1;
            }
            let more = match next {
                Some(q) if again => {
                    st.queue.push_back(q);
                    true
                }
                _ => !taken.is_empty(),
            };
            self.save_worker(sub, budget, st, None, took(st));
            let spent = st.tokens.0 + st.tokens.1;
            if spent > budget.tokens {
                return Err(Halt::OverBudget(format!(
                    "it used {spent} of its {} tokens",
                    budget.tokens
                )));
            }
            if !more {
                break;
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
        // An answered run needs no checkpoint; a stopped one keeps it.
        let answered = answer.is_some();
        let resumable = !answered && self.ck.lock().map(|c| c.is_some()).unwrap_or(false);
        if answered && let Some(sink) = self.checkpoints {
            let _ = sink.clear(&self.session.to_string());
        } else if resumable {
            self.save_with(|_| {});
        }
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
                    "resumable": resumable,
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

/// A finished worker's notes, from its checkpoint.
fn notes_from_checkpoint(w: &WorkerCheckpoint) -> WorkerNotes {
    WorkerNotes {
        worker: w.n,
        question: w.question.clone(),
        status: w.finished.unwrap_or_default(),
        notes: w.notes.clone(),
        sources: w
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

fn note_sources(pages: &[ReadPage]) -> Vec<NoteSource> {
    pages
        .iter()
        .map(|p| NoteSource {
            url: p.page.url.clone(),
            title: p.page.title.clone(),
            site: p.page.site.clone(),
        })
        .collect()
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
