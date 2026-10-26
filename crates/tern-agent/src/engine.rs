//! The bounded turn loop.
//!
//! One turn: the user's message, then model calls and tool calls until the
//! model answers or a limit stops it. Every step moves the
//! [`TurnStateMachine`], and every step is recorded as an event before the
//! next one starts, so a UI can replay exactly what happened.
//!
//! | Stop | State |
//! | --- | --- |
//! | The model answers | `Completed` (or `CompletedWithGaps` when the answer is empty) |
//! | Model-call, tool-call, or token limit | `BudgetExceeded` |
//! | The policy gate or the user refuses a step | `Blocked` |
//! | The same call three times | `Failed`, with a `loop.suspected` event |
//! | Provider error or the event log cannot be written | `Failed` |
//! | The user cancels | `Cancelled` |
//!
//! There is no silent retry and no fallback to another provider.

use std::collections::HashMap;
use std::sync::Arc;
use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, Ordering};

use serde::Serialize;
use serde_json::{Value, json};
use tern_core::{EffectClass, EventEnvelope, SessionId, TurnId, TurnState, TurnStateMachine};
use tern_providers::{
    ChatMessage, ChatRequest, ChatResponse, Provider, ProviderResult, Usage, UsageLedger,
};
use tern_store::Store;

use crate::checkpoint::{
    CHECKPOINT_VERSION, CheckpointSink, QuickState, RunCheckpoint, RunMode, Stage, now_rfc3339,
};
use crate::policy::{ApprovalAnswer, CallView, Decision, PolicyGate};
use crate::protocol::{self, Reply};
use crate::steer::{Steering, guidance_message};
use crate::tools::{BoxFuture, ToolError, ToolOutput, ToolRegistry, validate};

/// A chat model the loop can call. [`Provider`] is the real one; tests script one.
pub trait Model: Send + Sync {
    /// The catalog id, for the usage ledger (`openai`, `ollama`).
    fn provider_id(&self) -> &str;
    fn complete<'a>(&'a self, req: &'a ChatRequest) -> BoxFuture<'a, ProviderResult<ChatResponse>>;

    /// A streamed call. `on_text` gets text as it arrives and returns
    /// `false` to stop. Models that cannot stream answer in one piece.
    fn complete_stream<'a>(
        &'a self,
        req: &'a ChatRequest,
        on_text: &'a (dyn Fn(&str) -> bool + Send + Sync),
    ) -> BoxFuture<'a, ProviderResult<ChatResponse>> {
        Box::pin(async move {
            let reply = self.complete(req).await?;
            if !reply.text.is_empty() {
                on_text(&reply.text);
            }
            Ok(reply)
        })
    }

    /// [`Model::complete_stream`] with reasoning too. The default streams
    /// text only.
    fn complete_deltas<'a>(
        &'a self,
        req: &'a ChatRequest,
        on: &'a (dyn Fn(tern_providers::Delta<'_>) -> bool + Send + Sync),
    ) -> BoxFuture<'a, ProviderResult<ChatResponse>> {
        Box::pin(async move {
            let text = move |t: &str| on(tern_providers::Delta::Text(t));
            self.complete_stream(req, &text).await
        })
    }
}

impl Model for Provider {
    fn provider_id(&self) -> &str {
        self.spec().id
    }
    fn complete<'a>(&'a self, req: &'a ChatRequest) -> BoxFuture<'a, ProviderResult<ChatResponse>> {
        Box::pin(self.chat(req))
    }
    fn complete_stream<'a>(
        &'a self,
        req: &'a ChatRequest,
        on_text: &'a (dyn Fn(&str) -> bool + Send + Sync),
    ) -> BoxFuture<'a, ProviderResult<ChatResponse>> {
        Box::pin(self.chat_stream(req, on_text))
    }
    fn complete_deltas<'a>(
        &'a self,
        req: &'a ChatRequest,
        on: &'a (dyn Fn(tern_providers::Delta<'_>) -> bool + Send + Sync),
    ) -> BoxFuture<'a, ProviderResult<ChatResponse>> {
        Box::pin(self.chat_stream_deltas(req, on))
    }
}

/// How much of a reply's reasoning the log keeps.
const REASONING_KEEP: usize = 20_000;

fn cap_chars(s: &str, n: usize) -> String {
    match s.char_indices().nth(n) {
        Some((at, _)) => format!("{}…", &s[..at]),
        None => s.to_owned(),
    }
}

/// Receives reply text as it streams: the model call's number in the turn
/// (from 1) and the new text. A new number means a new reply, so a front end
/// starts its draft over.
pub type OnText<'a> = &'a (dyn Fn(u32, &str) + Send + Sync);

/// What the user sees when a step needs their go-ahead.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct ApprovalRequest {
    pub tool: String,
    pub summary: String,
    pub effect: EffectClass,
    pub target: Option<String>,
    pub args: Value,
}

/// Puts an approval in front of the user and waits for the answer.
pub trait Approver: Send + Sync {
    fn ask<'a>(&'a self, req: &'a ApprovalRequest) -> BoxFuture<'a, ApprovalAnswer>;
}

/// Refuses every approval. For runs with no one to ask.
#[derive(Debug, Default, Clone, Copy)]
pub struct DenyAll;

impl Approver for DenyAll {
    fn ask<'a>(&'a self, _req: &'a ApprovalRequest) -> BoxFuture<'a, ApprovalAnswer> {
        Box::pin(async { ApprovalAnswer::Deny })
    }
}

/// Where the loop writes events. A write failure stops the turn.
pub trait EventSink: Send + Sync {
    fn record(&self, event: EventEnvelope) -> Result<EventEnvelope, String>;
}

/// Keeps events in memory.
#[derive(Debug, Default)]
pub struct MemorySink(Mutex<Vec<EventEnvelope>>);

impl MemorySink {
    pub fn events(&self) -> Vec<EventEnvelope> {
        self.0.lock().expect("sink lock").clone()
    }
}

impl EventSink for MemorySink {
    fn record(&self, mut event: EventEnvelope) -> Result<EventEnvelope, String> {
        let mut v = self.0.lock().expect("sink lock");
        event.seq = v.len() as u64 + 1;
        v.push(event.clone());
        Ok(event)
    }
}

/// Writes events to the Tern database, redacted, in order.
#[derive(Debug)]
pub struct StoreSink {
    store: Mutex<Store>,
    /// A chat that has no row yet: it is written, with its opening
    /// event, just before the first event of its own.
    draft: Mutex<Option<Draft>>,
}

/// A session that exists only in memory until something happens in it.
#[derive(Debug)]
struct Draft {
    id: SessionId,
    project: tern_core::ProjectId,
    harness: tern_store::Harness,
    title: String,
    opened: EventEnvelope,
}

impl StoreSink {
    pub fn new(store: Store) -> Self {
        Self {
            store: Mutex::new(store),
            draft: Mutex::new(None),
        }
    }

    /// Holds session `id` back: its row and `opened` event are written
    /// only when the first other event for it is recorded, so opening a
    /// chat and leaving it saves nothing.
    pub fn defer_session(
        &self,
        id: SessionId,
        project: tern_core::ProjectId,
        harness: tern_store::Harness,
        title: &str,
        opened: EventEnvelope,
    ) {
        if let Ok(mut d) = self.draft.lock() {
            *d = Some(Draft {
                id,
                project,
                harness,
                title: title.to_owned(),
                opened,
            });
        }
    }

    /// Whether session `id` is still a draft with no row.
    pub fn is_draft(&self, id: SessionId) -> bool {
        self.draft
            .lock()
            .map(|d| d.as_ref().is_some_and(|d| d.id == id))
            .unwrap_or(false)
    }

    /// Writes the draft session's row now, if it is still a draft.
    pub fn materialize(&self) -> Result<(), String> {
        let mut store = self
            .store
            .lock()
            .map_err(|_| "the store lock was poisoned".to_owned())?;
        self.materialize_in(&mut store)
    }

    fn materialize_in(&self, store: &mut Store) -> Result<(), String> {
        let mut draft = self
            .draft
            .lock()
            .map_err(|_| "the draft lock was poisoned".to_owned())?;
        if let Some(d) = draft.take() {
            store
                .create_session_with_id(d.id, d.project, d.harness, &d.title)
                .map_err(|e| e.to_string())?;
            store.append(&d.opened).map_err(|e| e.to_string())?;
        }
        Ok(())
    }

    /// Runs `f` with the store (for small reads and renames between turns).
    pub fn with_store<T>(&self, f: impl FnOnce(&Store) -> T) -> Result<T, String> {
        let guard = self
            .store
            .lock()
            .map_err(|_| "the store lock was poisoned".to_owned())?;
        Ok(f(&guard))
    }

    /// [`StoreSink::with_store`] for writes. A draft session is written
    /// first, so writes about it find its row.
    pub fn with_store_mut<T>(&self, f: impl FnOnce(&mut Store) -> T) -> Result<T, String> {
        let mut guard = self
            .store
            .lock()
            .map_err(|_| "the store lock was poisoned".to_owned())?;
        self.materialize_in(&mut guard)?;
        Ok(f(&mut guard))
    }

    pub fn into_inner(self) -> Store {
        self.store.into_inner().expect("store lock")
    }
}

impl EventSink for StoreSink {
    fn record(&self, event: EventEnvelope) -> Result<EventEnvelope, String> {
        let mut store = self
            .store
            .lock()
            .map_err(|_| "the store lock was poisoned".to_owned())?;
        if self.is_draft(event.session_id) {
            self.materialize_in(&mut store)?;
        }
        store.append(&event).map_err(|e| e.to_string())
    }
}

/// Whether a provider's finish reason means the answer hit the output
/// limit and was cut off (`length`, `max_tokens`, `MAX_TOKENS`).
pub fn is_length_stop(finish_reason: Option<&str>) -> bool {
    finish_reason.is_some_and(|r| {
        matches!(
            r.to_ascii_lowercase().as_str(),
            "length" | "max_tokens" | "max_output_tokens" | "model_length"
        )
    })
}

/// The message a Continue sends after an answer was cut off.
pub const CONTINUE_PROMPT: &str = "Your last answer was cut off at the length limit. Continue exactly where it stopped, without repeating what you already wrote.";

/// Limits for one turn.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
pub struct Budget {
    pub max_model_calls: u32,
    pub max_tool_calls: u32,
    /// Input plus output tokens across the turn.
    pub max_tokens: u64,
}

impl Default for Budget {
    fn default() -> Self {
        Self {
            max_model_calls: 8,
            max_tool_calls: 12,
            max_tokens: 200_000,
        }
    }
}

/// Set from any thread to stop the turn at the next safe point.
#[derive(Clone, Debug, Default)]
pub struct CancelFlag(Arc<AtomicBool>);

impl CancelFlag {
    pub fn cancel(&self) {
        self.0.store(true, Ordering::SeqCst);
    }
    pub fn is_cancelled(&self) -> bool {
        self.0.load(Ordering::SeqCst)
    }
}

/// How a turn ended.
#[derive(Clone, Debug, PartialEq)]
pub struct TurnOutcome {
    pub turn_id: TurnId,
    pub state: TurnState,
    pub answer: Option<String>,
    /// Why the turn stopped, when it did not complete.
    pub reason: Option<String>,
    pub states: Vec<TurnState>,
    pub usage: Usage,
    pub model_calls: u32,
    pub tool_calls: u32,
}

/// Checkpoints, steering, and resume for a turn. All optional.
#[derive(Clone, Copy, Default)]
pub struct RunHooks<'a> {
    /// Saves the turn at the start and after every tool result.
    pub checkpoints: Option<&'a dyn CheckpointSink>,
    /// Notes the user sends while the turn runs, read before each model call.
    pub steering: Option<&'a Steering>,
    /// Picks the turn up from this checkpoint instead of starting over.
    pub resume: Option<&'a RunCheckpoint>,
    /// How hard the model should think, for models that can.
    pub effort: Option<tern_providers::Effort>,
    /// Receives the model's reasoning as it streams (call number, text),
    /// for a front end's Thinking section.
    pub on_reasoning: Option<OnText<'a>>,
    /// Offer the tools natively too (the provider's function calling), on
    /// top of the text protocol in the system prompt. A provider that
    /// refuses them gets the turn again without, and the model is
    /// remembered ([`mark_no_native_tools`]).
    pub native_tools: bool,
    /// Before the first model call, read what the message plainly asks
    /// for: a file by its full path (`file_read`, approved like any call
    /// outside the workspace) and a web search when it says "use the web"
    /// (`web_search`). Works with models that never call tools.
    pub prefetch: bool,
}

/// What a message plainly asks Tern to read before the model answers: the
/// file at a full path it names (when that file exists), and a web search
/// when it says to use the web. `previous` is the user's previous
/// question, searched for when the message only points back ("browse
/// about that"). Returns `(tool, args)` pairs.
pub fn prefetch_calls(
    text: &str,
    previous: Option<&str>,
    tools: &ToolRegistry,
) -> Vec<(&'static str, Value)> {
    let mut out = Vec::new();
    if tools.get("file_read").is_some()
        && let Some(raw) = crate::paths::find_in_text(text)
        && crate::paths::user_path(&raw).is_file()
    {
        out.push(("file_read", json!({"path": raw})));
    }
    if tools.get("web_search").is_some()
        && let Some(query) = crate::webintent::web_query_with(text, previous)
    {
        out.push(("web_search", json!({"query": query})));
    }
    out
}

/// The search a message asks for ("use the web tool", "search the web
/// for", "look it up", "google it", "browse"): the message without that
/// phrase. See [`crate::webintent`].
pub fn web_query(text: &str) -> Option<String> {
    crate::webintent::web_query(text)
}

/// A short human detail of a call for status lines: the query, the file's
/// name, the site.
pub fn call_detail(name: &str, args: &Value) -> Option<String> {
    let s = |k: &str| args.get(k).and_then(Value::as_str).map(str::trim);
    let d = match name {
        "web_search" | "vault_search" => s("query").map(str::to_owned),
        "read_url" => s("url").map(|u| {
            let rest = u.split("://").nth(1).unwrap_or(u);
            rest.split('/')
                .next()
                .unwrap_or(rest)
                .trim_start_matches("www.")
                .to_owned()
        }),
        _ => s("path")
            .or_else(|| s("name"))
            .or_else(|| s("file"))
            .map(|p| p.rsplit(['/', '\\']).next().unwrap_or(p).to_owned()),
    }?;
    let d: String = d.chars().take(80).collect();
    (!d.is_empty()).then_some(d)
}

/// Models that refused native tools this session, as `provider/model`.
static NO_NATIVE_TOOLS: std::sync::LazyLock<Mutex<std::collections::HashSet<String>>> =
    std::sync::LazyLock::new(Default::default);

/// Whether `provider`'s `model` may be offered native tools: yes unless
/// it refused them earlier this session.
pub fn native_tools_ok(provider: &str, model: &str) -> bool {
    !NO_NATIVE_TOOLS
        .lock()
        .is_ok_and(|s| s.contains(&format!("{provider}/{model}")))
}

/// Remembers that `provider`'s `model` refuses native tools.
pub fn mark_no_native_tools(provider: &str, model: &str) {
    if let Ok(mut s) = NO_NATIVE_TOOLS.lock() {
        s.insert(format!("{provider}/{model}"));
    }
}

/// Whether a provider error says the model cannot take tools (Ollama
/// "does not support tools", Mistral "Function calling is not enabled",
/// OpenRouter "No endpoints found that support tool use").
pub fn rejects_tools(error: &str) -> bool {
    let e = error.to_ascii_lowercase();
    (e.contains("tool") || e.contains("function call"))
        && [
            "not support",
            "unsupported",
            "not enabled",
            "not available",
            "no endpoints",
            "not allowed",
            "extra inputs",
            "unrecognized",
            "unknown",
        ]
        .iter()
        .any(|w| e.contains(w))
}

impl std::fmt::Debug for RunHooks<'_> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("RunHooks")
            .field("checkpoints", &self.checkpoints.is_some())
            .field("steering", &self.steering.is_some())
            .field("resume", &self.resume.map(|c| c.turn.as_str()))
            .finish()
    }
}

/// Longest tool result the model is shown. Errors are never cut.
const RESULT_LIMIT: usize = 12_000;
/// The same call this many times in one turn is treated as a loop.
const REPEAT_LIMIT: u32 = 3;

/// Everything one turn needs. Build it, then call [`TurnLoop::run`].
#[allow(missing_debug_implementations)]
pub struct TurnLoop<'a> {
    pub session: SessionId,
    pub model: &'a dyn Model,
    pub model_name: &'a str,
    pub tools: &'a ToolRegistry,
    pub gate: &'a mut PolicyGate,
    pub approver: &'a dyn Approver,
    pub sink: &'a dyn EventSink,
    pub ledger: &'a UsageLedger,
    pub budget: Budget,
    pub cancel: CancelFlag,
    /// Extra system instructions for the harness (Research or Study).
    pub instructions: Option<&'a str>,
    pub max_output_tokens: u32,
    /// Streams reply text to a front end. `None` makes one call per reply.
    pub on_text: Option<OnText<'a>>,
    /// Checkpoints, steering, and resume.
    pub hooks: RunHooks<'a>,
}

struct Run<'r, 'a> {
    cfg: &'r mut TurnLoop<'a>,
    turn: TurnId,
    fsm: TurnStateMachine,
    usage: Usage,
    model_calls: u32,
    tool_calls: u32,
    /// Tokens used before a resume, counted against the budget.
    prior: u64,
    /// The checkpoint as last saved.
    ck: Option<RunCheckpoint>,
    /// Applied steers, for the checkpoint.
    steers: Vec<String>,
    guidance: Vec<String>,
    /// Native tools still offered this turn.
    native: bool,
    /// The tools called, in order, for the diagnostic log.
    called: Vec<String>,
}

/// A step failed in a way that ends the turn.
struct Stop {
    state: TurnState,
    reason: String,
}

impl<'a> TurnLoop<'a> {
    /// Runs one turn on top of `history` (earlier messages in the session).
    pub async fn run(&mut self, history: Vec<ChatMessage>, user_message: &str) -> TurnOutcome {
        self.run_message(history, ChatMessage::user(user_message))
            .await
    }

    /// Runs one turn whose user message may carry pictures.
    pub async fn run_message(
        &mut self,
        history: Vec<ChatMessage>,
        message: ChatMessage,
    ) -> TurnOutcome {
        let mut run = Run {
            turn: TurnId::new(),
            fsm: TurnStateMachine::default(),
            usage: Usage::measured(0, 0),
            model_calls: 0,
            tool_calls: 0,
            prior: 0,
            ck: None,
            steers: Vec::new(),
            guidance: Vec::new(),
            native: self.hooks.native_tools,
            called: Vec::new(),
            cfg: self,
        };
        let result = run.drive(history, message).await;
        let offered: Vec<&str> = run.cfg.tools.names().collect();
        let line = format!(
            "turn: provider={} model={} tools_offered=[{}] native_tools={} tool_calls=[{}] model_calls={}",
            run.cfg.model.provider_id(),
            run.cfg.model_name,
            offered.join(","),
            run.native && !offered.is_empty(),
            run.called.join(","),
            run.model_calls,
        );
        let out = run.finish(result);
        crate::diag::note(&format!(
            "{line} state={:?}{}",
            out.state,
            out.reason
                .as_deref()
                .map(|r| format!(" reason={}", r.chars().take(200).collect::<String>()))
                .unwrap_or_default()
        ));
        out
    }
}

impl Run<'_, '_> {
    fn event(
        &self,
        kind: &str,
        effect: EffectClass,
        summary: impl Into<String>,
        payload: Value,
    ) -> EventEnvelope {
        EventEnvelope::new(self.cfg.session, kind, effect, summary, payload).with_turn(self.turn)
    }

    fn record(&self, e: EventEnvelope) -> Result<(), Stop> {
        self.cfg.sink.record(e).map(|_| ()).map_err(|e| Stop {
            state: TurnState::Failed,
            reason: format!("Tern could not write the run log, so it stopped: {e}"),
        })
    }

    fn go(&mut self, to: TurnState) -> Result<(), Stop> {
        self.fsm.move_to(to).map_err(|e| Stop {
            state: TurnState::Failed,
            reason: format!("internal error: {e}"),
        })
    }

    fn check_cancel(&self) -> Result<(), Stop> {
        if self.cfg.cancel.is_cancelled() {
            return Err(Stop {
                state: TurnState::Cancelled,
                reason: "You stopped the run.".into(),
            });
        }
        Ok(())
    }

    fn over_budget(reason: String) -> Stop {
        Stop {
            state: TurnState::BudgetExceeded,
            reason,
        }
    }

    async fn drive(
        &mut self,
        history: Vec<ChatMessage>,
        user_message: ChatMessage,
    ) -> Result<(TurnState, String), Stop> {
        self.go(TurnState::Preparing)?;
        let resume = self.cfg.hooks.resume.cloned();
        let message_text = match &resume {
            Some(c) => c.message.clone(),
            None => user_message.content.clone(),
        };
        // What the user typed is where trusted web addresses start.
        for m in history
            .iter()
            .filter(|m| m.role == tern_providers::Role::User)
        {
            self.cfg.gate.note_user_message(&m.content);
        }
        self.cfg.gate.note_user_message(&message_text);
        let mut started = json!({
            "model": self.cfg.model_name,
            "provider": self.cfg.model.provider_id(),
            "tools": self.cfg.tools.names().collect::<Vec<_>>(),
            "budget": self.cfg.budget,
            // Kept so the chat can be reopened (see `transcript`).
            "message": message_text,
            "images": user_message.images.iter().map(|i| i.name.as_str()).collect::<Vec<_>>(),
        });
        if let Some(c) = &resume {
            started["resumed"] = json!({"turn": c.turn, "stage": c.stage, "saved_at": c.saved_at});
        }
        self.record(self.event(
            "turn.started",
            EffectClass::Analysis,
            if resume.is_some() {
                "Resumed a turn"
            } else {
                "Started a turn"
            },
            started,
        ))?;
        let system = protocol::system_prompt(self.cfg.instructions, self.cfg.tools);
        let mut messages = history;
        // Exchanges an earlier turn left empty would make the provider
        // refuse this request (see [`crate::chat::tidy_history`]).
        crate::chat::tidy_history(&mut messages);
        let base = messages.len();
        let mut seen: HashMap<String, u32> = HashMap::new();
        match &resume {
            Some(c) => {
                let q = c.quick.clone().unwrap_or_default();
                if q.messages.is_empty() {
                    messages.push(user_message);
                } else {
                    // A checkpoint saved by an older build may still hold
                    // pictures; they are never replayed.
                    let mut saved = q.messages;
                    crate::chat::strip_pictures(&mut saved);
                    messages.extend(saved);
                }
                seen.extend(q.seen);
                self.model_calls = c.model_calls;
                self.tool_calls = c.tool_calls;
                self.prior = c.tokens_in + c.tokens_out;
                self.steers = c.steers.clone();
                self.guidance = c.guidance.clone();
                self.record(self.event(
                    "run.resumed",
                    EffectClass::Analysis,
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
                    }),
                ))?;
                let mut ck = c.clone();
                ck.resumes += 1;
                ck.turn = self.turn.to_string();
                self.ck = Some(ck);
            }
            None => {
                messages.push(user_message);
                if self.cfg.hooks.checkpoints.is_some() {
                    self.ck = Some(RunCheckpoint {
                        version: CHECKPOINT_VERSION,
                        session: self.cfg.session.to_string(),
                        turn: self.turn.to_string(),
                        mode: RunMode::Quick,
                        message: message_text.clone(),
                        model: self.cfg.model_name.to_owned(),
                        stage: Stage::Started,
                        ..RunCheckpoint::default()
                    });
                }
            }
        }
        self.save(&messages[base..], &seen);
        // Whether the web was searched this turn, and whether Tern's own
        // search after a refusal (or a copied reply) was used up.
        let mut searched = false;
        let mut net_used = false;
        let mut repeat_nudged = false;
        let previous_answer = messages[..base]
            .iter()
            .rev()
            .find(|m| m.role == tern_providers::Role::Assistant)
            .map(|m| m.content.clone());
        if self.cfg.hooks.prefetch && resume.is_none() {
            let previous = messages[..base]
                .iter()
                .rev()
                .find(|m| m.role == tern_providers::Role::User)
                .map(|m| m.content.clone());
            for (name, args) in prefetch_calls(&message_text, previous.as_deref(), self.cfg.tools) {
                self.check_cancel()?;
                self.go(TurnState::ToolRequested)?;
                self.called.push(name.to_owned());
                let result = self.tool_step(name, &args).await?;
                // As if the model had asked: its call, then the result.
                messages.push(ChatMessage::assistant(tern_providers::tool_block(
                    name, &args,
                )));
                let found_nothing = name == "web_search"
                    && (result.contains("Wikipedia has nothing on")
                        || result.contains(" found nothing on \""));
                let note = if found_nothing {
                    "(Tern searched the web just now because the message asked for it, and the search found nothing on the question. Say so plainly as the result above says; do not make up an answer and do not say you cannot access the internet.)"
                } else if name == "web_search" {
                    "(Tern searched the web just now because the message asked for it. These results are current: answer the user's question from them and cite the links as [n]. Do not say you cannot access the internet.)"
                } else {
                    "(Tern ran this because the message asked for it. Answer the message using it.)"
                };
                crate::diag::note(&format!(
                    "prefetch: tool={name} detail={:?} injected_chars={}",
                    call_detail(name, &args).unwrap_or_default(),
                    result.len()
                ));
                messages.push(ChatMessage::user(format!("{result}\n\n{note}")));
                searched |= name == "web_search";
                self.save(&messages[base..], &seen);
            }
        }
        let mut empty_retried = false;

        loop {
            self.check_cancel()?;
            self.take_steering(&mut messages)?;
            let b = self.cfg.budget;
            if self.model_calls >= b.max_model_calls {
                return Err(Self::over_budget(format!(
                    "The turn reached its limit of {} model calls.",
                    b.max_model_calls
                )));
            }
            self.go(TurnState::ModelStreaming)?;
            let mut req = ChatRequest::new(self.cfg.model_name, messages.clone())
                .with_system(system.clone())
                .with_max_tokens(self.cfg.max_output_tokens)
                .with_effort(self.cfg.hooks.effort);
            if self.native {
                req = req.with_tools(self.cfg.tools.specs());
            }
            self.model_calls += 1;
            let model = self.cfg.model;
            let thought = Mutex::new((
                String::new(),
                None::<std::time::Instant>,
                None::<std::time::Duration>,
            ));
            let asked = std::time::Instant::now();
            let reply = match self.cfg.on_text {
                Some(sink) => {
                    let call = self.model_calls;
                    let cancel = self.cfg.cancel.clone();
                    let on_reasoning = self.cfg.hooks.on_reasoning;
                    let thought = &thought;
                    let forward = move |d: tern_providers::Delta<'_>| {
                        match d {
                            tern_providers::Delta::Text(t) => {
                                let mut th = thought.lock().expect("thought lock");
                                if th.1.is_some() && th.2.is_none() {
                                    th.2 = Some(asked.elapsed());
                                }
                                drop(th);
                                sink(call, t);
                            }
                            tern_providers::Delta::Reasoning(r) => {
                                let mut th = thought.lock().expect("thought lock");
                                th.0.push_str(r);
                                th.1.get_or_insert_with(std::time::Instant::now);
                                drop(th);
                                if let Some(f) = on_reasoning {
                                    f(call, r);
                                }
                            }
                        }
                        !cancel.is_cancelled()
                    };
                    model.complete_deltas(&req, &forward).await
                }
                None => model.complete(&req).await,
            };
            let reply = match reply {
                // The model takes no native tools: ask again with the text
                // protocol only, and remember it for later turns.
                Err(e) if !req.tools.is_empty() && rejects_tools(&e.to_string()) => {
                    self.native = false;
                    self.model_calls -= 1;
                    mark_no_native_tools(self.cfg.model.provider_id(), self.cfg.model_name);
                    self.record(self.event(
                        "model.no_native_tools",
                        EffectClass::Analysis,
                        format!(
                            "{} does not take tools natively; Tern asks for tools in plain text",
                            self.cfg.model_name
                        ),
                        json!({"error": e.to_string()}),
                    ))?;
                    self.go(TurnState::Verifying)?;
                    continue;
                }
                r => r,
            }
            .map_err(|e| Stop {
                state: TurnState::Failed,
                reason: e.to_string(),
            })?;
            // A stopped stream returns what arrived; it is not an answer.
            self.check_cancel()?;
            self.cfg.ledger.record(
                self.cfg.model.provider_id(),
                self.cfg.model_name,
                reply.usage,
            );
            self.add_usage(reply.usage);
            self.record(
                self.event(
                    "model.replied",
                    EffectClass::Network,
                    format!("{} replied", self.cfg.model_name),
                    {
                        let (text, _, took) = thought.into_inner().unwrap_or_default();
                        let mut p = json!({"finish_reason": reply.finish_reason, "model": reply.model, "chars": reply.text.len()});
                        if !text.is_empty() {
                            // The reasoning is kept (capped) so a reopened chat
                            // still has its Thinking section.
                            p["reasoning"] = json!(cap_chars(&text, REASONING_KEEP));
                            p["thought_ms"] = json!(took.unwrap_or_else(|| asked.elapsed()).as_millis() as u64);
                        }
                        if let Some(e) = self.cfg.hooks.effort {
                            p["effort"] = json!(e.id());
                        }
                        p
                    },
                )
                .with_usage(reply.usage.to_delta()),
            )?;
            if self.usage.total() + self.prior > b.max_tokens {
                return Err(Self::over_budget(format!(
                    "The turn used {} tokens, over its limit of {}.",
                    self.usage.total() + self.prior,
                    b.max_tokens
                )));
            }

            match protocol::parse(&reply.text) {
                // An empty reply (only reasoning, or nothing) is asked for
                // once more as it stands; a second empty one ends the turn
                // with a gap. It never joins the conversation: providers
                // refuse an empty assistant message on every later request.
                Reply::Answer(text) if text.is_empty() && !empty_retried => {
                    empty_retried = true;
                    self.record(self.event(
                        "model.empty_reply",
                        EffectClass::Analysis,
                        format!(
                            "{} gave an empty reply; Tern asked once more",
                            self.cfg.model_name
                        ),
                        json!({"finish_reason": reply.finish_reason}),
                    ))?;
                    self.go(TurnState::Verifying)?;
                }
                Reply::Answer(text) => {
                    let refused = crate::webintent::is_refusal(&text);
                    let copied = previous_answer
                        .as_deref()
                        .is_some_and(|p| crate::webintent::same_reply(&text, p));
                    // The model says it cannot reach the web, or copies its
                    // last reply word for word, while web search is on: Tern
                    // searches the question itself once and asks again.
                    if (refused || copied)
                        && !net_used
                        && self.cfg.tools.get("web_search").is_some()
                    {
                        net_used = true;
                        let question = crate::webintent::web_query_with(
                            &message_text,
                            messages[..base]
                                .iter()
                                .rev()
                                .find(|m| m.role == tern_providers::Role::User)
                                .map(|m| m.content.as_str()),
                        )
                        .or_else(|| {
                            let (_, q) = crate::reply::split(&message_text);
                            let q = q.split("<attachments>").next().unwrap_or(q).trim();
                            (!crate::webintent::keywords(q).is_empty()).then(|| q.to_owned())
                        });
                        if let Some(query) = question {
                            let args = json!({"query": query});
                            self.record(self.event(
                                "web.safety_net",
                                EffectClass::Analysis,
                                if refused {
                                    "The model said it could not look this up; Tern searched the web for it"
                                } else {
                                    "The model repeated its last reply; Tern searched the web for the question"
                                },
                                json!({"query": query, "refused": refused, "copied": copied, "searched_before": searched}),
                            ))?;
                            crate::diag::note(&format!(
                                "safety_net: refused={refused} copied={copied} query={query:?}"
                            ));
                            self.go(TurnState::ToolRequested)?;
                            self.called.push("web_search".to_owned());
                            let result = self.tool_step("web_search", &args).await?;
                            searched = true;
                            messages.push(ChatMessage::assistant(tern_providers::tool_block(
                                "web_search",
                                &args,
                            )));
                            messages.push(ChatMessage::user(format!(
                                "{result}\n\n(Tern searched the web just now. You do have these live results: answer the user's question from them, cite the links as [n], and do not say you cannot access the internet or repeat an earlier reply.)"
                            )));
                            self.save(&messages[base..], &seen);
                            continue;
                        }
                    }
                    // A copy of the last reply is never shown again: asked
                    // once more, then the turn ends with a gap.
                    if copied {
                        if !repeat_nudged {
                            repeat_nudged = true;
                            self.record(self.event(
                                "model.repeated",
                                EffectClass::Analysis,
                                format!(
                                    "{} repeated its last reply; Tern asked again",
                                    self.cfg.model_name
                                ),
                                json!({"chars": text.len()}),
                            ))?;
                            messages.push(ChatMessage::assistant(reply.text));
                            messages.push(ChatMessage::user(
                                "That reply is word for word the same as your previous one. Answer the latest message itself, with something new.".to_owned(),
                            ));
                            self.go(TurnState::Verifying)?;
                            continue;
                        }
                        self.go(TurnState::Verifying)?;
                        return Err(Stop {
                            state: TurnState::CompletedWithGaps,
                            reason: "The model gave the same reply as before, twice, so Tern did not show it again.".into(),
                        });
                    }
                    self.go(TurnState::Verifying)?;
                    return Ok(if text.is_empty() {
                        (TurnState::CompletedWithGaps, text)
                    } else {
                        (TurnState::Completed, text)
                    });
                }
                Reply::Malformed(why) => {
                    self.go(TurnState::ToolRequested)?;
                    self.go(TurnState::RecordResult)?;
                    self.record(self.event(
                        "tool.rejected",
                        EffectClass::Analysis,
                        "The model asked for a tool in a form Tern could not read",
                        json!({"reason": why}),
                    ))?;
                    messages.push(ChatMessage::assistant(reply.text));
                    messages.push(ChatMessage::user(format!(
                        "Tern could not read that tool request: {why}. Try again or answer directly."
                    )));
                    self.save(&messages[base..], &seen);
                }
                Reply::Tool { .. } => {
                    self.go(TurnState::ToolRequested)?;
                    // Every call in the reply runs in this one round (a
                    // provider's parallel tool calls, or several blocks),
                    // and all the results go back together.
                    let calls = protocol::parse_calls(&reply.text);
                    let mut results = Vec::with_capacity(calls.len());
                    for (i, (name, args)) in calls.iter().enumerate() {
                        let key = format!("{name}:{args}");
                        let n = seen.entry(key).or_default();
                        *n += 1;
                        if *n >= REPEAT_LIMIT {
                            self.record(self.event(
                                "loop.suspected",
                                EffectClass::Analysis,
                                format!(
                                    "The model asked for the same `{name}` call {REPEAT_LIMIT} times"
                                ),
                                json!({"tool": name, "args": args}),
                            ))?;
                            return Err(Stop {
                                state: TurnState::Failed,
                                reason: format!(
                                    "The model kept repeating the same `{name}` call, so Tern stopped the turn."
                                ),
                            });
                        }
                        if i > 0 {
                            self.check_cancel()?;
                            self.go(TurnState::ToolRequested)?;
                        }
                        self.called.push(name.clone());
                        searched |= name == "web_search";
                        results.push(self.tool_step(name, args).await?);
                    }
                    messages.push(ChatMessage::assistant(reply.text));
                    messages.push(ChatMessage::user(results.join("\n\n")));
                    // Safe point: the tools' results are in the conversation.
                    self.save(&messages[base..], &seen);
                }
            }
        }
    }

    /// Saves the turn's conversation so far, when it saves checkpoints.
    fn save(&mut self, turn: &[ChatMessage], seen: &HashMap<String, u32>) {
        let Some(sink) = self.cfg.hooks.checkpoints else {
            return;
        };
        let (prior_in, prior_out) = (self.prior_in(), self.prior_out());
        let Some(ck) = self.ck.as_mut() else {
            return;
        };
        // Pictures are not kept in a checkpoint: a resumed run (or "Retry")
        // never replays one, so a model that cannot see is never sent one
        // again, and the file stays small. Each leaves its name.
        let mut messages = turn.to_vec();
        crate::chat::strip_pictures(&mut messages);
        ck.quick = Some(QuickState {
            messages,
            seen: seen.iter().map(|(k, v)| (k.clone(), *v)).collect(),
        });
        ck.model_calls = self.model_calls;
        ck.tool_calls = self.tool_calls;
        ck.tokens_in = prior_in + self.usage.input_tokens;
        ck.tokens_out = prior_out + self.usage.output_tokens;
        ck.steers = self.steers.clone();
        ck.guidance = self.guidance.clone();
        ck.saved_at = now_rfc3339();
        // A failed save never stops the turn.
        let _ = sink.save(ck);
    }

    fn prior_in(&self) -> u64 {
        self.cfg.hooks.resume.map(|c| c.tokens_in).unwrap_or(0)
    }

    fn prior_out(&self) -> u64 {
        self.cfg.hooks.resume.map(|c| c.tokens_out).unwrap_or(0)
    }

    /// Safe point before a model call: steering notes join the
    /// conversation as guidance.
    fn take_steering(&mut self, messages: &mut Vec<ChatMessage>) -> Result<(), Stop> {
        let Some(inbox) = self.cfg.hooks.steering else {
            return Ok(());
        };
        for note in inbox.take() {
            self.record(self.event(
                "steer.received",
                EffectClass::Analysis,
                format!("You steered the run: {}", note.text),
                json!({"text": note.text, "action": {"action": "guide", "text": note.text}}),
            ))?;
            let block = guidance_message(&note.text);
            // Kept as one user message, since some providers want turns to
            // alternate.
            match messages.last_mut() {
                Some(last) if last.role == tern_providers::Role::User => {
                    last.content.push_str("\n\n");
                    last.content.push_str(&block);
                }
                _ => messages.push(ChatMessage::user(block)),
            }
            let summary = format!(
                "Applied: the model reads “{}” before its next step",
                note.text
            );
            self.steers.push(summary.clone());
            self.guidance.push(note.text.clone());
            self.record(self.event(
                "steer.applied",
                EffectClass::Analysis,
                summary,
                json!({"text": note.text, "applied": true, "how": "guidance"}),
            ))?;
        }
        Ok(())
    }

    /// What a tool result adds to the gate's web provenance: search lists
    /// addresses the user's own provider chose; a page or a server's output
    /// is content Tern did not write.
    fn note_provenance(&mut self, name: &str, o: &ToolOutput) {
        let gate = &mut *self.cfg.gate;
        if name == "web_search" {
            for h in o.meta["hits"].as_array().into_iter().flatten() {
                if let Some(u) = h["url"].as_str() {
                    gate.know_text(u);
                }
            }
        }
        if name == "read_url" {
            for u in &o.evidence_refs {
                gate.know_text(u);
            }
        }
        if name == "read_url"
            || name.starts_with("mcp__")
            || o.content
                .to_ascii_lowercase()
                .contains(&format!("<{}", crate::untrusted::TAG))
        {
            gate.saw_untrusted();
        }
    }

    fn add_usage(&mut self, u: Usage) {
        self.usage.input_tokens += u.input_tokens;
        self.usage.output_tokens += u.output_tokens;
        if u.measure == tern_core::Measure::Estimated {
            self.usage.measure = tern_core::Measure::Estimated;
        }
    }

    /// Validates, gates, runs, and records one tool call. Returns the text
    /// the model reads next.
    async fn tool_step(&mut self, name: &str, args: &Value) -> Result<String, Stop> {
        let Some(tool) = self.cfg.tools.get(name) else {
            self.go(TurnState::RecordResult)?;
            self.record(self.event(
                "tool.rejected",
                EffectClass::Analysis,
                format!("The model asked for `{name}`, which this turn does not have"),
                json!({"tool": name}),
            ))?;
            let names = self.cfg.tools.names().collect::<Vec<_>>().join(", ");
            return Ok(format!(
                "There is no tool called `{name}`. Available: {}.",
                if names.is_empty() { "none" } else { &names }
            ));
        };
        if let Err(e) = validate(&tool.input_schema(), args) {
            self.go(TurnState::RecordResult)?;
            self.record(self.event(
                "tool.rejected",
                tool.effect(),
                format!("`{name}` was asked for with bad arguments"),
                json!({"tool": name, "error": e}),
            ))?;
            return Ok(format!("Tool `{name}` {e}. Check its input schema."));
        }
        if self.tool_calls >= self.cfg.budget.max_tool_calls {
            return Err(Self::over_budget(format!(
                "The turn reached its limit of {} tool calls.",
                self.cfg.budget.max_tool_calls
            )));
        }

        let target = tool.target(args);
        let view = CallView {
            tool: name,
            effect: tool.effect(),
            target: target.as_deref(),
            args,
        };
        self.go(TurnState::PolicyCheck)?;
        let risk = self.cfg.gate.url_risk(&view);
        let asks = tool.asks_because(args);
        let decision = self.cfg.gate.check_asking(&view, asks.is_some());
        let why = match &risk {
            Some(Ok(why)) => Some(why.clone()),
            _ => asks,
        };
        self.record(self.event(
            "policy.decided",
            tool.effect(),
            match (&decision, &why) {
                (Decision::Allow { because }, _) => format!("`{name}` allowed: {because}"),
                (Decision::Ask, Some(why)) => format!("`{name}` needs your approval: {why}"),
                (Decision::Ask, None) => format!("`{name}` needs your approval"),
                (Decision::Deny { reason }, _) => reason.clone(),
            },
            json!({"tool": name, "target": target, "decision": decision, "flag": why,
                   "detail": call_detail(name, args)}),
        ))?;
        match decision {
            Decision::Allow { .. } => {}
            Decision::Deny { reason } => {
                self.go(TurnState::Blocked)?;
                return Err(Stop {
                    state: TurnState::Blocked,
                    reason,
                });
            }
            Decision::Ask => {
                self.go(TurnState::ApprovalRequested)?;
                let req = ApprovalRequest {
                    tool: name.to_owned(),
                    summary: match &why {
                        Some(why) => format!("{} Tern asks because {why}.", tool.summary()),
                        None => tool.summary().to_owned(),
                    },
                    effect: tool.effect(),
                    target: target.clone(),
                    args: args.clone(),
                };
                self.record(self.event(
                    "approval.requested",
                    tool.effect(),
                    format!("Asked you to approve `{name}`"),
                    serde_json::to_value(&req).unwrap_or_default(),
                ))?;
                let answer = self.cfg.approver.ask(&req).await;
                let allowed = self.cfg.gate.answer(&view, answer);
                self.record(self.event(
                    "approval.answered",
                    tool.effect(),
                    if allowed {
                        format!("You approved `{name}`")
                    } else {
                        format!("You declined `{name}`")
                    },
                    json!({"tool": name, "answer": answer}),
                ))?;
                if !allowed {
                    self.go(TurnState::Blocked)?;
                    return Err(Stop {
                        state: TurnState::Blocked,
                        reason: format!("You declined `{name}`, so the turn stopped there."),
                    });
                }
                self.check_cancel()?;
            }
        }

        self.go(TurnState::Executing)?;
        self.tool_calls += 1;
        let out: Result<ToolOutput, ToolError> =
            match tokio::time::timeout(tool.timeout(), tool.call(args.clone())).await {
                Ok(r) => r,
                Err(_) => Err(ToolError::TimedOut),
            };
        self.cfg.gate.consume(&view);
        self.go(TurnState::RecordResult)?;
        if let Ok(o) = &out {
            self.note_provenance(name, o);
        }
        Ok(match out {
            Ok(o) => {
                let shown = truncate(&o.content, RESULT_LIMIT);
                self.record(
                    self.event(
                        "tool.executed",
                        tool.effect(),
                        format!("Ran `{name}`"),
                        json!({"tool": name, "version": tool.version(), "args": args, "chars": o.content.len(), "truncated": shown.len() < o.content.len(), "meta": o.meta}),
                    )
                    .with_evidence(o.evidence_refs.clone()),
                )?;
                if shown.len() < o.content.len() {
                    format!(
                        "Result of `{name}` (cut to {} of {} characters):\n{shown}",
                        shown.len(),
                        o.content.len()
                    )
                } else {
                    format!("Result of `{name}`:\n{shown}")
                }
            }
            Err(e) => {
                self.record(self.event(
                    "tool.failed",
                    tool.effect(),
                    format!("`{name}` {e}"),
                    json!({"tool": name, "args": args, "error": e, "retry_safe": tool.idempotent()}),
                ))?;
                format!("Tool `{name}` {e}.")
            }
        })
    }

    fn finish(mut self, result: Result<(TurnState, String), Stop>) -> TurnOutcome {
        let (state, answer, reason) = match result {
            Ok((state, text)) => {
                let gap = (state == TurnState::CompletedWithGaps)
                    .then(|| "The model gave an empty answer.".to_owned());
                (state, Some(text), gap)
            }
            Err(stop) => (stop.state, None, Some(stop.reason)),
        };
        if self.fsm.state() != state {
            // A failure to move here only happens after an earlier failure,
            // which already set a terminal state.
            let _ = self.fsm.move_to(state);
        }
        let kind = match state {
            TurnState::BudgetExceeded => "budget.exceeded",
            TurnState::Cancelled => "run.cancelled",
            _ => "turn.finished",
        };
        let answered = answer.is_some();
        if answered && let Some(sink) = self.cfg.hooks.checkpoints {
            let _ = sink.clear(&self.cfg.session.to_string());
        }
        let resumable = !answered && self.cfg.hooks.checkpoints.is_some() && self.ck.is_some();
        // The log may be what failed; the outcome still reports the stop.
        let _ = self.record(
            self.event(
                kind,
                EffectClass::Analysis,
                reason.clone().unwrap_or_else(|| "Answered".into()),
                json!({
                    "state": state,
                    "model_calls": self.model_calls,
                    "tool_calls": self.tool_calls,
                    "answer_chars": answer.as_ref().map(String::len),
                    "answer": answer,
                    "resumable": resumable,
                }),
            )
            .with_usage(self.usage.to_delta()),
        );
        TurnOutcome {
            turn_id: self.turn,
            state: self.fsm.state(),
            answer,
            reason,
            states: self.fsm.history().to_vec(),
            usage: self.usage,
            model_calls: self.model_calls,
            tool_calls: self.tool_calls,
        }
    }
}

fn truncate(s: &str, limit: usize) -> &str {
    if s.len() <= limit {
        return s;
    }
    let mut end = limit;
    while !s.is_char_boundary(end) {
        end -= 1;
    }
    &s[..end]
}

#[cfg(test)]
mod length_tests {
    #[test]
    fn length_stops_are_spotted_across_providers() {
        assert!(super::is_length_stop(Some("length")));
        assert!(super::is_length_stop(Some("max_tokens")));
        assert!(super::is_length_stop(Some("MAX_TOKENS")));
        assert!(!super::is_length_stop(Some("stop")));
        assert!(!super::is_length_stop(Some("end_turn")));
        assert!(!super::is_length_stop(None));
    }
}
