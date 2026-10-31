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
//! | Provider error (after any retries) or the event log cannot be written | `Failed` |
//! | The user cancels | `Cancelled` |
//!
//! There is no silent retry and no fallback to another provider: a passing
//! provider failure (429, 5xx, 529, a dropped connection) is retried with
//! backoff only after a `provider.retry` event says so (#72).

use std::collections::HashMap;
use std::sync::Arc;
use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, Ordering};

use serde::Serialize;
use serde_json::{Value, json};
use tern_core::{
    EffectClass, ErrorClass, EventEnvelope, KeptSummary, PausedBy, SessionId, TurnId, TurnReceipt,
    TurnState, TurnStateMachine,
};
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
use crate::tools::{BoxFuture, Tool, ToolError, ToolOutput, ToolRegistry, validate};

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

/// Streamed reply text is checkpointed in the log after this many new
/// characters (#85)...
const PROGRESS_CHARS: usize = 200;
/// ...or after this long, whichever comes first.
const PROGRESS_EVERY: std::time::Duration = std::time::Duration::from_secs(1);

/// What the model reads when the turn's budget is used up and its one
/// reserved call is spent on the answer (#85).
pub const WRAP_UP_PROMPT: &str = "(Tern note: this turn's budget is used up. Do not call any more tools. Write your answer now from what you already gathered, citing it as before. End with a short section titled \"Not established\" listing what you could not check in this turn.)";

/// What the model reads instead of a tool result once the budget is used
/// up (#85).
const WRAP_UP_TOOL: &str = "Tern did not run this call: the turn's budget is used up. Do not call any more tools; write the answer from what you have.";

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

    /// Files the draft under another project before its row is written
    /// (the folder picked under an empty chat's composer). `false` once
    /// the chat has a row.
    pub fn set_draft_project(&self, project: tern_core::ProjectId) -> bool {
        match self.draft.lock() {
            Ok(mut d) => match d.as_mut() {
                Some(d) => {
                    d.project = project;
                    true
                }
                None => false,
            },
            Err(_) => false,
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

/// The message a resumed turn sends after the answer text that streamed
/// before the stop (#85).
pub const RESUME_PROMPT: &str = "(Tern note: your answer above was interrupted. Continue it exactly where it stopped, without repeating what you already wrote.)";

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
    /// After a web search for a general question, read its top three
    /// results (#85, A13): the first part of each page and its best
    /// passage go back with the search, so quotes come from read pages.
    pub quick_reads: bool,
    /// How transient provider failures (429, 5xx, 529, a dropped
    /// connection) are retried; `None` is the default policy. Each
    /// wait is a `provider.retry` event first (#72).
    pub retry: Option<tern_providers::RetryPolicy>,
    /// Where full copies of long tool results go (the model sees their
    /// head and tail and reads the rest with `read_result`). `None` keeps
    /// no copy (#72).
    pub results: Option<&'a crate::results::ResultStore>,
    /// Results of searches, fetches and reads kept for the chat (#77): the
    /// same call again gets the kept result, marked on its card. `None`
    /// caches nothing.
    pub cache: Option<&'a crate::toolcache::ToolCache>,
    /// The rate-limit governor (#85): asked before every model call, so a
    /// call waits for its window, is made to fit, or pauses for the day
    /// instead of being sent to fail. `None` sends every call at once.
    pub governor: Option<&'a tern_providers::governor::Governor>,
    /// Keep one model call in reserve (#85): when the turn's budget runs
    /// out, the model writes its answer from what it gathered, with the
    /// gaps listed, instead of the turn stopping with no answer.
    pub wrap_up: bool,
    /// A smaller model of the same provider the rest of this turn may run
    /// on when the chosen model hits its daily limit (#85, policy D7).
    /// Never sticky: it lasts this turn only, with a `model.switched` row.
    pub fallback_model: Option<&'a str>,
    /// Sampling pinned for the turn (#85, A7), for models that take a
    /// temperature: the same question gets the same kind of answer in
    /// every chat. `None` leaves the provider's default.
    pub temperature: Option<f32>,
    /// Dev chaos (#85, A17): faults injected into this turn's model
    /// calls. `None` uses the process-wide switches (`TERN_DEV=1` with
    /// `TERN_CHAOS`), which are off in normal use.
    pub chaos: Option<&'a crate::chaos::Chaos>,
    /// The token estimator (#85, A3b) this turn calibrates and reads;
    /// `None` uses the process-wide one. Tests give each run its own.
    pub estimator: Option<&'a tern_providers::estimate::Estimator>,
    /// The turn's budget in money (#85, A5), in micro-dollars, priced
    /// from the models.dev snapshot: past it, the reserved wrap-up
    /// answers. `None`, or a model with no known price, has no money
    /// budget; the token budget still holds.
    pub max_cost_micros: Option<u64>,
}

/// A `provider.retry` event's payload: who, which retry of how many, how
/// long Tern waits, why, and the error it got. The window shows "Retrying
/// in N s (attempt k/n)" from it.
pub fn retry_payload(provider: &str, n: &tern_providers::RetryNotice) -> Value {
    json!({
        "provider": provider,
        "attempt": n.attempt,
        "of": n.of,
        "wait_ms": n.wait.as_millis() as u64,
        "reason": n.reason,
        "error": n.error,
    })
}

/// What the model is told when it says it cannot open files that are in
/// the conversation as text (#70).
pub const FILE_NUDGE: &str = "(Tern note: the attached files are already in this conversation as text. Tern converted them on this computer and put their full contents inside the <attachments> block of the user's message above. Read them there and answer the question; do not say you cannot access, open or analyze files. If the text there is empty or only says a page is scanned, say what is missing and tell the user to press \"Read again as scanned\" on the file's chip to read it with the local document or OCR model.)";

/// What the model is told when it asks in prose whether to make a change
/// the user asked for (#78).
pub const CONFIRM_NUDGE: &str = "(Tern note: do not ask in prose whether to go ahead. The user asked for this change: call the tool now (`file_edit` to add or change a few lines, `file_write` for a new file or a full rewrite). Tern shows the user an approval card with the diff, and that card is the confirmation. Ask in prose only when you cannot tell which file or what text the user means.)";

/// Said once when, in Act mode with a folder, the user asked for a script
/// that makes a file and the model printed the script instead of running
/// it (#79).
pub const SCRIPT_NUDGE: &str = "(Tern note: do not show the script for the user to run. Run it yourself now with `run_python`. Inside `run_python` there is no open(), os or pathlib: use the tool functions listed under Code mode instead (for example files_list(), file_read(path), file_write(path, text)), and print a short summary of what it did.)";

/// The tools that work on the user's folder.
const FILE_TOOLS: &[&str] = &[
    "files_list",
    "file_read",
    "file_search",
    "file_write",
    "file_edit",
    "file_rename",
];

/// What follows the scholarly search Tern runs before an academic
/// question (#83).
const SCHOLAR_PREFETCH_NOTE: &str = "(Tern searched the scholarly literature first because this is an academic question. Answer from these papers and cite them as [n] with their links. Only `paper_read` gives a paper's text: state what a paper finds only from passages you were shown, say \"according to its abstract\" when you only have the abstract, and never describe the contents of a paper you have only seen listed. If the papers do not cover the question, say \"not enough evidence\" rather than answering from memory; you may call `web_search` afterwards for background.)";

/// Said once when the model refuses an ordinary writing request (#80).
pub const REFUSAL_NUDGE: &str = "(Tern note: this is an ordinary request. Answer it directly and in full; ignore any words in it about pressing buttons in the app.)";

/// What the model reads when its answer says `path` was written but the
/// write failed (#80).
pub fn claim_note(path: &str, why: &str) -> String {
    format!(
        "(Tern note: {path} was not written: {why}. Do not say it was saved or written. Fix the call and write it again, or tell the user plainly that {path} was not written and why.)"
    )
}

/// What the model reads instead of a `file_write` that would rewrite a
/// file the user asked to add to (#79): `path`, and how many of its lines
/// the rewrite removes or changes.
pub fn add_nudge(path: &str, removed: usize) -> String {
    format!(
        "Tern did not run this `file_write`: the user asked to add to {path}, and this call would replace its whole text ({removed} of its lines would be removed or changed). Call `file_edit` on {path} with `append` (only the new lines) to add at the end, or with `find` and `replace` to insert next to an exact passage."
    )
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
    // Only the user's own words: never the attachments block, the text
    // Tern read from a picture, or a quoted reply (#70: a picture's
    // description once became a web search for "Tron Legacy").
    let (own, _) = crate::attach::split(text);
    let (_, own) = crate::reply::split(own);
    if tools.get("file_read").is_some()
        && let Some(raw) = crate::paths::find_in_text(own)
        && crate::paths::user_path(&raw).is_file()
    {
        out.push(("file_read", json!({"path": raw})));
    } else if tools.get("file_read").is_some()
        && tools.get("files_list").is_some()
        && let Some(name) = crate::webintent::file_to_read(own)
    {
        // "read sum.md" in a chat with a folder (#78: the model answered
        // from memory right after a write): the file as it is now.
        out.push(("file_read", json!({"path": name})));
    }
    // An academic question (papers, studies, a named technique such as
    // FLARE) searches the literature first, whatever the model would pick
    // (#83: small models chose `web_search` with `scholar_search` on
    // offer). Never for a folder or file task, nor when the message itself
    // asks for the web ("use the web tool"): that search runs instead.
    // A follow-up that points back at the previous answer ("read the top
    // 5 papers from that list") searches nothing new (#85, A9): it is
    // answered from the papers that answer named.
    let follow_up = previous.is_some()
        && crate::followup::points_back(own)
        && !crate::webintent::asks_for_web(own);
    if follow_up {
        crate::diag::note("prefetch: skipped (follow-up points back at the previous answer)");
    }
    if tools.get("scholar_search").is_some()
        && !follow_up
        && !crate::webintent::asks_for_web(own)
        && crate::intent::classify(own) == crate::intent::Intent::Academic
        && let Some(query) = crate::intent::scholar_query(own)
    {
        out.push(("scholar_search", json!({"query": query})));
    }
    if tools.get("web_search").is_some()
        && !follow_up
        && let Some(query) = web_prefetch(text, previous)
    {
        out.push(("web_search", json!({"query": query})));
    }
    out
}

/// The web search Tern runs before the model answers `text`, if any
/// ([`prefetch_calls`] with web search on): from the user's own words
/// only, never for a question about an attachment (#70).
pub fn web_prefetch(text: &str, previous: Option<&str>) -> Option<String> {
    let (own, chips) = crate::attach::split(text);
    let (_, own) = crate::reply::split(own);
    let has_attachments = !chips.is_empty() || own.len() < text.len();
    if has_attachments && crate::webintent::about_attachment(own) {
        return None;
    }
    let previous = previous.map(|p| crate::reply::split(crate::attach::split(p).0).1);
    crate::webintent::web_query_with(own, previous)
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
        "web_search" | "vault_search" | "scholar_search" => s("query").map(str::to_owned),
        "paper_read" => s("paper").map(str::to_owned),
        "paper_snowball" => s("topic").map(str::to_owned),
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

// Longest tool result the model is shown (head and tail, #72). Errors are
// never cut.
use crate::results::RESULT_LIMIT;
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
    /// Reply text streamed before the user stopped the run (#71): kept in
    /// the log with the stop, so a reopened chat shows it.
    partial: String,
    /// Tool calls numbered so far this turn (#75): each call's events
    /// carry its number, so a card follows one call through its states.
    call_seq: u32,
    /// The number the next `tool_step` uses, set when a reply's calls were
    /// queued.
    next_call: Option<u32>,
    /// The call a script or a task graph runs inside (#77): its calls'
    /// events carry it as `parent`, so their cards nest under it.
    parent: Option<u32>,
    /// The task graph task a call runs for, as `task`.
    task: Option<String>,
    /// The model's plan as it last sent it (#78): reminded after each
    /// round of tools while steps are open, ended when the run ends.
    plan: Option<crate::planner::PlanView>,
    /// What the user sent, for the steers that read it inside a call.
    message: String,
    /// Set when the turn ended because it needs Act mode (#80): the
    /// answer the user sees, with the "Switch to Act and run" card.
    needs_act: Option<String>,
    /// How the last call that reached [`Run::post`] went: whether it
    /// succeeded and its value for a script (`meta.py`). `None` when the
    /// call was refused before it ran.
    last_post: Option<(bool, Value)>,
    /// Writes that failed this turn and were not written since, by path,
    /// with the reason (#80: "saved to counts.md" after the write failed).
    failed_writes: std::collections::BTreeMap<String, String>,
    /// The model's token cap for one request, once the provider said the
    /// request was too large (#80); later requests are made to fit.
    request_cap: Option<tern_providers::retry::RequestCap>,
    /// A code-mode script ran and called no tool, once per turn (#80).
    script_noted: bool,
    /// What the turn already wrote and renamed (#82): the same change
    /// again is skipped without a card.
    redo: crate::redo::Redo,
    /// How the last call was skipped as already done or refused by
    /// [`crate::redo`]: its write is not a failed write.
    redo_skipped: bool,
    /// The folder changes a running code-mode script made, as the lines
    /// its result ends with (#82). `None` outside a script.
    script_actions: Option<Vec<String>>,
    /// The call being dispatched was asked for as a `file_edit` (the
    /// turn queues it as the `file_write` it becomes).
    asked_edit: bool,
    /// The answer Tern ends the turn with when the model keeps redoing a
    /// change it was told is done (#82).
    closing: Option<String>,
    /// Calls that failed (#85): logged with their tokens, never counted
    /// against the budget.
    failed_calls: u32,
    /// Tool results recorded this turn, for the receipt.
    tool_results: u32,
    /// Pages read this turn, by address, for the receipt.
    sources: Vec<String>,
    /// Why the turn paused (#85), when it did: by what, the error class,
    /// and when a daily limit resets.
    pause: Option<(PausedBy, ErrorClass, Option<std::time::SystemTime>)>,
    /// The budget ran out and the next model call is the reserved
    /// wrap-up (#85).
    wrap_pending: bool,
    /// The reserved wrap-up call is under way.
    wrapping: bool,
    /// The model call number the wrap-up is allowed (one past the limit).
    wrap_at: u32,
    /// The step journal (#85): every tool call that ran, by its key, with
    /// the text the model read. A resumed turn replays a step instead of
    /// running it again.
    journal: Vec<crate::checkpoint::JournalStep>,
    /// Steps replayed from the journal this turn.
    journal_hits: u32,
    /// Steps recorded this turn, added to the journal when it is saved.
    recorded: Vec<crate::checkpoint::JournalStep>,
    /// Answer text that streamed before the stop this turn resumes (#85):
    /// sent back as the start of the answer, and kept in front of it.
    resumed_partial: String,
    /// The turn moved to the within-provider fallback model (#85).
    switched: bool,
    /// The top results of a search were read this turn (#85, A13).
    quick_read: bool,
    /// Addresses that failed this turn, with the error (#85, A6): the same
    /// address is not fetched again in the turn (a 403 is fetched once).
    failed_urls: HashMap<String, String>,
    /// Papers `paper_read` returned this turn (#85, A6), by paper and
    /// question, with the call that read them: read once per turn.
    papers_read: HashMap<String, u32>,
    /// `paper_read` calls run for the model's current reply (#85, A12):
    /// at most [`Run::read_batch`] of them, so the next request fits.
    batch_reads: u32,
    /// What the turn's counted calls cost so far, in micro-dollars (#85,
    /// A5); `None` once a call's price was unknown.
    cost_micros: Option<u64>,
}

/// A call checked by the gate and ready to run (#77).
struct Ready<'t> {
    call: u32,
    name: String,
    args: Value,
    tool: &'t dyn Tool,
    target: Option<String>,
    task: Option<String>,
}

/// What [`Run::tool_pre`] found.
enum Pre<'t> {
    /// Answered without running: an unknown tool, bad input, a refusal.
    Done(String),
    Go(Ready<'t>),
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
            partial: String::new(),
            call_seq: 0,
            next_call: None,
            parent: None,
            task: None,
            plan: None,
            message: String::new(),
            needs_act: None,
            last_post: None,
            failed_writes: Default::default(),
            request_cap: None,
            script_noted: false,
            redo: Default::default(),
            redo_skipped: false,
            script_actions: None,
            asked_edit: false,
            closing: None,
            failed_calls: 0,
            tool_results: 0,
            sources: Vec::new(),
            pause: None,
            wrap_pending: false,
            wrapping: false,
            wrap_at: 0,
            journal: Vec::new(),
            journal_hits: 0,
            recorded: Vec::new(),
            resumed_partial: String::new(),
            switched: false,
            quick_read: false,
            failed_urls: HashMap::new(),
            papers_read: HashMap::new(),
            batch_reads: 0,
            cost_micros: Some(0),
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

    fn record(&self, mut e: EventEnvelope) -> Result<(), Stop> {
        if (self.parent.is_some() || self.task.is_some())
            && e.payload.get("call").is_some()
            && let Some(o) = e.payload.as_object_mut()
        {
            if let Some(p) = self.parent {
                o.entry("parent").or_insert(json!(p));
            }
            if let Some(t) = &self.task {
                o.entry("task").or_insert(json!(t));
            }
        }
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

    /// Waits `d` (a rate-limit window, #85), stopping early when the user
    /// stops the run.
    async fn sleep_or_cancel(&self, d: std::time::Duration) -> Result<(), Stop> {
        let until = tokio::time::Instant::now() + d;
        loop {
            self.check_cancel()?;
            let now = tokio::time::Instant::now();
            if now >= until {
                return Ok(());
            }
            tokio::time::sleep((until - now).min(std::time::Duration::from_millis(100))).await;
        }
    }

    /// Whether the turn works through an open plan in Act mode (the file
    /// tools are offered): it gets twice the call limits (#79: a plan
    /// turn stopped at 12 model calls with steps still open).
    fn plan_room(&self) -> bool {
        self.plan.as_ref().is_some_and(|p| p.open()) && self.cfg.tools.get("file_write").is_some()
    }

    /// The model calls this turn may make now.
    fn model_limit(&self) -> u32 {
        let n = self.cfg.budget.max_model_calls;
        if self.plan_room() {
            n.saturating_mul(2)
        } else {
            n
        }
    }

    /// The tool calls this turn may make now.
    fn tool_limit(&self) -> u32 {
        let n = self.cfg.budget.max_tool_calls;
        if self.plan_room() {
            n.saturating_mul(2)
        } else {
            n
        }
    }

    /// Hands the rest of this turn to the within-provider fallback model
    /// (#85), once, with a `model.switched` row. `false` when there is
    /// none, or the turn already switched.
    fn switch_model(&mut self, reason: ErrorClass) -> Result<bool, Stop> {
        let from = self.cfg.model_name;
        let Some(to) = self.cfg.hooks.fallback_model.filter(|m| *m != from) else {
            return Ok(false);
        };
        if self.switched {
            return Ok(false);
        }
        let provider = self.cfg.model.provider_id().to_owned();
        let line = format!(
            "The daily limit for {from} is used up; this turn continues on {to}. New chats and your next message use {from} again."
        );
        crate::diag::note(&format!(
            "model_switch: provider={provider} from={from} to={to} reason={reason:?}"
        ));
        let payload = serde_json::to_value(tern_core::ModelSwitched {
            from_provider: provider.clone(),
            from_model: from.to_owned(),
            to_provider: provider,
            to_model: to.to_owned(),
            reason,
            line: line.clone(),
            chosen_by_user: false,
        })
        .unwrap_or_default();
        self.record(self.event(
            tern_core::run_state::MODEL_SWITCHED,
            EffectClass::Network,
            line,
            payload,
        ))?;
        self.cfg.model_name = to;
        self.switched = true;
        Ok(true)
    }

    /// The model calls the wrap-up may reach: its one reserved call.
    fn model_calls_at_wrap(&self) -> u32 {
        self.wrap_at
    }

    fn over_budget(reason: String) -> Stop {
        Stop {
            state: TurnState::BudgetExceeded,
            reason,
        }
    }

    /// Moves the state machine on to Completed from wherever the turn
    /// stopped (a needs-Act stop can come before a model call, inside a
    /// call, or after a reply).
    fn land_completed(&mut self) {
        for to in [
            TurnState::ModelStreaming,
            TurnState::RecordResult,
            TurnState::Verifying,
            TurnState::Completed,
        ] {
            let at = self.fsm.state();
            if at == TurnState::Completed {
                return;
            }
            // Only the steps that lead on: Preparing goes through a model
            // step, a call through its result.
            let wanted = match to {
                TurnState::ModelStreaming => at == TurnState::Preparing,
                _ => true,
            };
            if wanted && at.can_move_to(to) {
                let _ = self.fsm.move_to(to);
            }
        }
    }

    /// Whether the chat has a folder but is in Chat mode, so its file
    /// changes are left out (#80).
    /// Whether Tern may search the web itself after a refusal or a copied
    /// reply: never for a folder or file task (#83: "count words in every
    /// .md file here and save the counts to counts.md" became a web search
    /// and the answer said it found no relevant results).
    fn safety_net_allowed(&self, message: &str) -> bool {
        let (own, _) = crate::attach::split(message);
        let (_, own) = crate::reply::split(own);
        let file_tools = FILE_TOOLS.iter().any(|t| self.cfg.tools.get(t).is_some());
        let used_file_tools = self.called.iter().any(|c| FILE_TOOLS.contains(&c.as_str()));
        crate::intent::safety_net_allowed(own, file_tools, used_file_tools)
    }

    fn chat_mode_folder(&self) -> bool {
        self.cfg.tools.mode() == Some(crate::workspace::TurnMode::Chat)
            && self.cfg.tools.hidden("file_write")
    }

    /// Ends the turn because it needs Act mode (#80): `tool` is what the
    /// model (or the request) needed, `path` the file when one is known.
    /// The window shows the answer with "Switch to Act and run".
    fn needs_act_stop(&mut self, tool: &str, path: Option<&str>) -> Stop {
        let what = match (tool, path.filter(|p| !p.trim().is_empty())) {
            ("file_write" | "file_edit", Some(p)) => format!("write {p}"),
            ("file_rename", Some(p)) => format!("rename {p}"),
            ("file_rename", None) => "rename files".to_owned(),
            ("run_command", _) => "run commands".to_owned(),
            _ => "change files in this folder".to_owned(),
        };
        let text = format!(
            "This needs Act mode. In Chat mode Tern reads this folder but does not change it. Switch to Act to let Tern {what}; each change still asks you first."
        );
        crate::diag::note(&format!("turn: needs Act mode for `{tool}`"));
        let _ = self.record(self.event(
            "mode.needs_act",
            EffectClass::Analysis,
            format!("This needs Act mode (`{tool}`)"),
            json!({"tool": tool, "path": path, "message": self.message, "mode": "chat"}),
        ));
        self.needs_act = Some(text.clone());
        Stop {
            state: TurnState::Blocked,
            reason: text,
        }
    }

    /// The write this answer claims, when it failed and was not written
    /// since: `(path, why)` (#80).
    fn failed_claim(&self, answer: &str) -> Option<(String, String)> {
        self.failed_writes
            .iter()
            .find(|(p, _)| crate::webintent::claims_written(answer, p))
            .map(|(p, e)| (p.clone(), e.clone()))
    }

    /// Notes how a file write went, for the answer-claim guard.
    fn note_write(&mut self, name: &str, args: &Value, result: &str) {
        // A write skipped as already done, or refused after earlier
        // writes, leaves the file as those writes made it (#82).
        if !matches!(name, "file_write" | "file_edit") || std::mem::take(&mut self.redo_skipped) {
            return;
        }
        let Some(path) = args["path"].as_str().filter(|p| !p.is_empty()) else {
            return;
        };
        match &self.last_post {
            Some((true, _)) => {
                self.failed_writes.remove(path);
            }
            _ => {
                let why = result
                    .strip_prefix(&format!("Tool `{name}` "))
                    .unwrap_or(result)
                    .trim()
                    .trim_end_matches('.')
                    .chars()
                    .take(300)
                    .collect::<String>();
                self.failed_writes.insert(path.to_owned(), why);
            }
        }
    }

    async fn drive(
        &mut self,
        history: Vec<ChatMessage>,
        user_message: ChatMessage,
    ) -> Result<(TurnState, String), Stop> {
        self.go(TurnState::Preparing)?;
        let resume = self.cfg.hooks.resume.cloned();
        // A resumed turn keeps its turn id (#85): its steps, ledger and
        // evidence stay keyed to the one turn however often it resumes.
        if let Some(id) = resume.as_ref().and_then(|c| c.turn.parse::<TurnId>().ok()) {
            self.turn = id;
        }
        let message_text = match &resume {
            Some(c) => c.message.clone(),
            None => user_message.content.clone(),
        };
        self.message = message_text.clone();
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
        // The turn fingerprint (#85, A7): what decides how the model
        // behaves, so two chats can be compared in Plan › Detailed.
        let fp_system = protocol::system_prompt(self.cfg.instructions, self.cfg.tools);
        started["fingerprint"] = json!({
            "prompt_version": protocol::PROMPT_VERSION,
            "prompt_hash": protocol::prompt_hash(&fp_system),
            "tools_hash": self.cfg.tools.tools_hash(),
            "model": self.cfg.model_name,
            "provider": self.cfg.model.provider_id(),
            "native_tools": self.native,
            "temperature": self.cfg.hooks.temperature,
        });
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
        // With native tools the schemas travel in the request, so the
        // system text does not repeat them (#85).
        let full_system = protocol::system_prompt(self.cfg.instructions, self.cfg.tools);
        let native_system = protocol::system_prompt_native(self.cfg.instructions, self.cfg.tools);
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
                self.redo = crate::redo::Redo::from_marks(q.writes);
                self.journal = q.journal;
                // What streamed before the stop goes back as the start of
                // the answer (#85), so the model goes on from there.
                if !q.partial.trim().is_empty() {
                    messages.push(ChatMessage::assistant(q.partial.clone()));
                    messages.push(ChatMessage::user(RESUME_PROMPT));
                    self.resumed_partial = q.partial;
                }
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
        // A folder chat in Chat mode asked to make or change a file (#80:
        // the run planned `file_write`, was refused, and told the user to
        // create the file by hand): said before any model call.
        if resume.is_none() && self.chat_mode_folder() {
            let (own, _) = crate::attach::split(&message_text);
            let (_, own) = crate::reply::split(own);
            if crate::webintent::asks_to_write_files(own) {
                let path = crate::webintent::last_file_name(own);
                return Err(self.needs_act_stop("file_write", path.as_deref()));
            }
        }
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
                let mut result = self.tool_step(name, &args).await?;
                if name == "web_search" {
                    result = self.quick_reads(result).await?;
                }
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
                } else if name == "scholar_search" {
                    SCHOLAR_PREFETCH_NOTE
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
        // "Read the top 5 papers from that list" (#85, A9, A-T11): the
        // previous answer's papers, by id, instead of a new search.
        if resume.is_none()
            && let Some(answer) = previous_answer.as_deref()
        {
            let (own, _) = crate::attach::split(&message_text);
            let (_, own) = crate::reply::split(own);
            if crate::followup::points_back(own) && !crate::webintent::asks_for_web(own) {
                // The previous exchange: the answer first, then what the
                // tools returned on the way to it.
                let start = messages[..base]
                    .iter()
                    .rposition(|m| m.role == tern_providers::Role::User)
                    .unwrap_or(0);
                let earlier: Vec<&str> = std::iter::once(answer)
                    .chain(messages[start..base].iter().map(|m| m.content.as_str()))
                    .collect();
                let ids = crate::followup::paper_ids(earlier);
                crate::diag::note(&format!(
                    "follow-up: points back; previous answer named {} paper id(s)",
                    ids.len()
                ));
                if !ids.is_empty() {
                    messages.push(ChatMessage::user(crate::followup::note(
                        &ids,
                        crate::followup::wanted_count(own),
                    )));
                }
            }
        }
        let mut empty_retried = false;
        let mut file_nudged = false;
        let mut confirm_nudged = false;
        let mut script_nudged = false;
        let mut add_steered = false;
        let mut claim_steered = false;
        let mut refusal_nudged = false;
        // "add a line to sum.md": a whole-file rewrite is steered to
        // `file_edit` once (#79).
        let asks_to_add = crate::webintent::asks_to_add(&message_text);

        'calls: loop {
            self.check_cancel()?;
            self.take_steering(&mut messages)?;
            let b = self.cfg.budget;
            let limit = self.model_limit();
            if self.model_calls >= limit && !self.wrapping {
                if self.cfg.hooks.wrap_up {
                    self.wrap_pending = true;
                } else {
                    return Err(Self::over_budget(format!(
                        "The turn reached its limit of {limit} model calls."
                    )));
                }
            } else if self.wrapping && self.model_calls >= limit.max(self.model_calls_at_wrap()) {
                return Err(Self::over_budget(
                    "The turn's budget ran out before the model wrote its answer.".into(),
                ));
            }
            if std::mem::take(&mut self.wrap_pending) && !self.wrapping {
                self.wrapping = true;
                self.wrap_at = self.model_calls + 1;
                self.record(self.event(
                    "budget.wrap_up",
                    EffectClass::Analysis,
                    "The turn's budget is used up; the model writes its answer from what it has",
                    json!({"model_calls": self.model_calls, "tool_calls": self.tool_calls,
                           "tokens": self.usage.total() + self.prior,
                           "failed_not_counted": self.failed_calls}),
                ))?;
                match messages.last_mut() {
                    Some(last) if last.role == tern_providers::Role::User => {
                        last.content.push_str("\n\n");
                        last.content.push_str(WRAP_UP_PROMPT);
                    }
                    _ => messages.push(ChatMessage::user(WRAP_UP_PROMPT)),
                }
            }
            self.go(TurnState::ModelStreaming)?;
            let mut req = ChatRequest::new(self.cfg.model_name, messages.clone())
                .with_system(if self.native && !self.wrapping {
                    native_system.clone()
                } else {
                    full_system.clone()
                })
                .with_max_tokens(self.cfg.max_output_tokens)
                .with_effort(self.cfg.hooks.effort);
            if self.native && !self.wrapping {
                req = req.with_tools(self.cfg.tools.specs());
            }
            req.temperature = self.cfg.hooks.temperature;
            // The provider said the request was too large for this model
            // (#80: Groq's 413 on an 8000-token cap): every request from
            // here is made to fit, or the turn says plainly why not.
            if let Some(cap) = self.request_cap {
                req = match fit_request(req, base, cap, self.cfg.tools, &self.called) {
                    Ok(r) => r,
                    Err(need) => {
                        return Err(Stop {
                            state: TurnState::Failed,
                            reason: too_large_line(
                                self.cfg.model_name,
                                self.cfg.model.provider_id(),
                                cap,
                                need,
                            ),
                        });
                    }
                };
            }
            // The governor first (#85): wait for the window, make the
            // request fit, or pause for the day; never send it to fail.
            let pid = self.cfg.model.provider_id().to_owned();
            let mut census = request_census(&req);
            let estimator = self
                .cfg
                .hooks
                .estimator
                .unwrap_or(tern_providers::estimate::global());
            let mut est_in = estimator.estimate_census(&pid, self.cfg.model_name, &census);
            if let Some(gov) = self.cfg.hooks.governor {
                let mut prefitted = false;
                loop {
                    let need = est_in + u64::from(req.max_tokens.min(1024));
                    match gov.admit(&pid, self.cfg.model_name, need) {
                        tern_providers::governor::Admission::Now => break,
                        tern_providers::governor::Admission::Wait(d) => {
                            let secs = d.as_secs_f64().ceil() as u64;
                            crate::diag::note(&format!(
                                "governor: provider={pid} model={} wait_ms={} need={need}",
                                self.cfg.model_name,
                                d.as_millis()
                            ));
                            self.record(self.event(
                                "governor.wait",
                                EffectClass::Network,
                                format!("Waiting {secs} s for the rate limit, then continuing"),
                                json!({"provider": pid, "model": self.cfg.model_name,
                                       "wait_ms": d.as_millis() as u64, "tokens": need,
                                       "run_state": tern_core::RunState::Waiting,
                                       "error": ErrorClass::RateLimitShort}),
                            ))?;
                            self.sleep_or_cancel(d).await?;
                            gov.take(&pid, self.cfg.model_name, need);
                            break;
                        }
                        tern_providers::governor::Admission::TooBig { max_tokens }
                            if !prefitted =>
                        {
                            prefitted = true;
                            let cap = tern_providers::retry::RequestCap {
                                limit: max_tokens,
                                requested: Some(need),
                            };
                            self.record(self.event(
                                "context.trimmed",
                                EffectClass::Analysis,
                                format!(
                                    "Tern made the request fit {}'s {max_tokens}-token window before sending it",
                                    self.cfg.model_name
                                ),
                                serde_json::to_value(tern_core::ContextTrimmed {
                                    tier: "prefit".into(),
                                    before_tokens: need,
                                    after_tokens: max_tokens,
                                    items: Vec::new(),
                                    line: format!("Request made to fit {max_tokens} tokens"),
                                })
                                .unwrap_or_default(),
                            ))?;
                            req = match fit_request(req, base, cap, self.cfg.tools, &self.called) {
                                Ok(r) => r,
                                Err(n) => {
                                    return Err(Stop {
                                        state: TurnState::Failed,
                                        reason: too_large_line(self.cfg.model_name, &pid, cap, n),
                                    });
                                }
                            };
                            if self.request_cap.is_none() {
                                self.request_cap = Some(cap);
                            }
                            census = request_census(&req);
                            est_in = estimator.estimate_census(&pid, self.cfg.model_name, &census);
                        }
                        tern_providers::governor::Admission::TooBig { .. } => break,
                        tern_providers::governor::Admission::Daily { resets_at } => {
                            if self.switch_model(ErrorClass::RateLimitDaily)? {
                                self.go(TurnState::Verifying)?;
                                continue 'calls;
                            }
                            self.pause = Some((
                                PausedBy::DailyQuota,
                                ErrorClass::RateLimitDaily,
                                Some(resets_at),
                            ));
                            return Err(Stop {
                                state: TurnState::Paused,
                                reason: daily_line(self.cfg.model_name, resets_at),
                            });
                        }
                    }
                }
            }
            self.model_calls += 1;
            // Dev chaos (#85, A17) sits in front of the model when on.
            let chaos_model;
            let model: &dyn Model = match self.cfg.hooks.chaos.or(crate::chaos::global()) {
                Some(chaos) if chaos.remaining() > 0 => {
                    chaos_model = crate::chaos::ChaosModel {
                        inner: self.cfg.model,
                        chaos,
                    };
                    &chaos_model
                }
                _ => self.cfg.model,
            };
            let thought = Mutex::new((
                String::new(),
                None::<std::time::Instant>,
                None::<std::time::Duration>,
            ));
            let asked = std::time::Instant::now();
            let streamed = Mutex::new(String::new());
            // A passing failure (rate limit, overload, dropped connection)
            // is tried again after a visible wait, but only while nothing
            // of this reply reached the screen: a second reply would repeat
            // it (#72).
            let retry = self.cfg.hooks.retry.unwrap_or_default();
            let (events, session, turn) = (self.cfg.sink, self.cfg.session, self.turn);
            let provider = self.cfg.model.provider_id();
            let notify = move |n: &tern_providers::RetryNotice| {
                let _ = events.record(
                    EventEnvelope::new(
                        session,
                        "provider.retry",
                        EffectClass::Network,
                        n.line(),
                        retry_payload(provider, n),
                    )
                    .with_turn(turn),
                );
            };
            let stop = &self.cfg.cancel;
            let may_retry = || !stop.is_cancelled() && streamed.lock().is_ok_and(|s| s.is_empty());
            // Native tool calls' arguments as they stream, by index, for
            // the Files tab's live preview of a file being written (#75).
            let writes: Mutex<HashMap<usize, (String, String, usize)>> = Mutex::default();
            // What streamed is checkpointed in the log every second or 200
            // characters (#85), so a crash mid-reply keeps it.
            let progress = Mutex::new((0usize, std::time::Instant::now()));
            let call = async {
                match self.cfg.on_text {
                    Some(sink) => {
                        let call = self.model_calls;
                        let cancel = self.cfg.cancel.clone();
                        let on_reasoning = self.cfg.hooks.on_reasoning;
                        let thought = &thought;
                        let streamed = &streamed;
                        let writes = &writes;
                        let progress = &progress;
                        let forward = move |d: tern_providers::Delta<'_>| {
                            match d {
                                tern_providers::Delta::Tool { index, name, args } => {
                                    stream_write_preview(writes, index, name, args);
                                }
                                tern_providers::Delta::Text(t) => {
                                    let mut th = thought.lock().expect("thought lock");
                                    if th.1.is_some() && th.2.is_none() {
                                        th.2 = Some(asked.elapsed());
                                    }
                                    drop(th);
                                    // Nothing after the stop reaches the window.
                                    if cancel.is_cancelled() {
                                        return false;
                                    }
                                    if let Ok(mut s) = streamed.lock() {
                                        s.push_str(t);
                                        // A text-fenced tool call (providers
                                        // without native tools) previews a file
                                        // write as it streams too (#76).
                                        stream_fence_preview(&s, writes);
                                        if let Ok(mut p) = progress.lock()
                                            && (s.len() - p.0 >= PROGRESS_CHARS
                                                || p.1.elapsed() >= PROGRESS_EVERY)
                                        {
                                            let _ = events.record(
                                            EventEnvelope::new(
                                                session,
                                                crate::transcript::ANSWER_PROGRESS,
                                                EffectClass::Analysis,
                                                "",
                                                json!({"call": call, "from": p.0, "text": &s[p.0..]}),
                                            )
                                            .with_turn(turn),
                                        );
                                            *p = (s.len(), std::time::Instant::now());
                                        }
                                    }
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
                        tern_providers::with_retry(retry, &notify, &may_retry, |_| {
                            model.complete_deltas(&req, &forward)
                        })
                        .await
                    }
                    None => {
                        tern_providers::with_retry(retry, &notify, &may_retry, |_| {
                            model.complete(&req)
                        })
                        .await
                    }
                }
            };
            // Stop works within a second even when the stream is silent
            // (#85): the call is dropped, which closes the connection.
            let reply = match or_cancel(&self.cfg.cancel, call).await {
                Some(r) => r,
                None => Err(tern_providers::ProviderError::Network {
                    provider: "Tern",
                    reason: "stopped".into(),
                }),
            };
            // Stopped mid-reply: what streamed up to the stop is kept with
            // the stop (#71), and nothing more is asked.
            if self.cfg.cancel.is_cancelled() {
                self.partial = streamed.into_inner().unwrap_or_default();
                return Err(Stop {
                    state: TurnState::Cancelled,
                    reason: "You stopped the run.".into(),
                });
            }
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
                Err(e) if tern_providers::retry::request_cap(&e).is_some() => {
                    let cap = tern_providers::retry::request_cap(&e).unwrap_or(
                        tern_providers::retry::RequestCap {
                            limit: 0,
                            requested: None,
                        },
                    );
                    if self.request_cap.is_some() {
                        // Already made to fit once; it still does not.
                        let need = cap.requested.unwrap_or(cap.limit + 1);
                        return Err(Stop {
                            state: TurnState::Failed,
                            reason: too_large_line(
                                self.cfg.model_name,
                                self.cfg.model.provider_id(),
                                cap,
                                need,
                            ),
                        });
                    }
                    self.request_cap = Some(cap);
                    self.model_calls -= 1;
                    crate::diag::note(&format!(
                        "provider: request too large for {} (cap {}, asked {:?}); sending a compact request",
                        self.cfg.model_name, cap.limit, cap.requested
                    ));
                    self.record(self.event(
                        "provider.request_cap",
                        EffectClass::Analysis,
                        format!(
                            "{} accepts {} tokens per request; Tern sends a shorter request",
                            self.cfg.model_name, cap.limit
                        ),
                        json!({"limit": cap.limit, "requested": cap.requested}),
                    ))?;
                    self.go(TurnState::Verifying)?;
                    continue;
                }
                r => r,
            };
            let reply = match reply {
                Ok(r) => r,
                Err(e) => {
                    // What streamed before the failure is kept with it
                    // (#85), like a stop's (#71).
                    self.partial = streamed.lock().map(|s| s.clone()).unwrap_or_default();
                    let class = error_class(&e, !self.partial.trim().is_empty());
                    crate::diag::note(&format!(
                        "provider: error class={class:?} provider={pid} model={} streamed_chars={}",
                        self.cfg.model_name,
                        self.partial.len()
                    ));
                    // A passing failure pauses the turn with its work kept;
                    // Continue picks it up (#85). Anything else fails.
                    let by = match class {
                        Some(ErrorClass::RateLimitDaily) => Some(PausedBy::DailyQuota),
                        Some(
                            ErrorClass::StreamCut
                            | ErrorClass::Network
                            | ErrorClass::RateLimitShort
                            | ErrorClass::Overloaded,
                        ) => Some(PausedBy::Network),
                        _ => None,
                    };
                    if class == Some(ErrorClass::RateLimitDaily) {
                        self.cfg
                            .hooks
                            .governor
                            .unwrap_or(tern_providers::governor::global())
                            .close_for_day(&pid, self.cfg.model_name, None);
                        if self.switch_model(ErrorClass::RateLimitDaily)? {
                            // The failed call is not counted (#85).
                            self.model_calls -= 1;
                            self.failed_calls += 1;
                            self.partial.clear();
                            self.go(TurnState::Verifying)?;
                            continue 'calls;
                        }
                    }
                    let (Some(by), Some(class)) = (by, class) else {
                        return Err(Stop {
                            state: TurnState::Failed,
                            reason: e.to_string(),
                        });
                    };
                    let resets = (class == ErrorClass::RateLimitDaily).then(|| {
                        self.cfg
                            .hooks
                            .governor
                            .unwrap_or(tern_providers::governor::global())
                            .close_for_day(&pid, self.cfg.model_name, None)
                    });
                    self.pause = Some((by, class, resets));
                    return Err(Stop {
                        state: TurnState::Paused,
                        reason: match resets {
                            Some(at) => format!("{e} {}", daily_line(self.cfg.model_name, at)),
                            None => e.to_string(),
                        },
                    });
                }
            };
            // A stopped stream returns what arrived; it is not an answer.
            self.check_cancel()?;
            self.cfg.ledger.record(
                self.cfg.model.provider_id(),
                self.cfg.model_name,
                reply.usage,
            );
            self.add_usage(reply.usage);
            // One line per model call (#85, A8): tokens in, out and
            // cached, the estimate it was sent on, and how long it took.
            crate::diag::note(&call_line(
                &pid,
                self.cfg.model_name,
                self.model_calls,
                &reply.usage,
                est_in,
                asked.elapsed(),
                self.failed_calls,
            ));
            if reply.usage.measure == tern_core::Measure::Measured && reply.usage.input_tokens > 0 {
                estimator.observe_census(
                    &pid,
                    self.cfg.model_name,
                    &census,
                    reply.usage.input_tokens,
                );
                if let Some(gov) = self.cfg.hooks.governor {
                    gov.settle(
                        &pid,
                        self.cfg.model_name,
                        est_in + u64::from(req.max_tokens.min(1024)),
                        reply.usage.total(),
                    );
                }
            }
            // Priced from the models.dev snapshot (#85, A5), cache reads
            // and writes apart (A19); unknown prices stay unknown.
            let mut delta = reply.usage.to_delta();
            delta.cost_micros =
                tern_providers::prices::cost_micros(&pid, self.cfg.model_name, &reply.usage);
            self.cost_micros = match (self.cost_micros, delta.cost_micros) {
                (Some(a), Some(c)) => Some(a + c),
                _ => None,
            };
            self.record(
                self.event(
                    "model.replied",
                    EffectClass::Network,
                    format!("{} replied", self.cfg.model_name),
                    {
                        let (text, _, took) = thought.into_inner().unwrap_or_default();
                        let mut p = json!({"finish_reason": reply.finish_reason, "model": reply.model, "chars": reply.text.len(), "estimated_input": est_in,
                            "cached_tokens": reply.usage.cached_tokens, "cache_write_tokens": reply.usage.cache_write_tokens,
                            "reasoning_tokens": reply.usage.reasoning_tokens});
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
                .with_usage(delta),
            )?;
            let over_cost = matches!(
                (self.cfg.hooks.max_cost_micros, self.cost_micros),
                (Some(max), Some(spent)) if spent > max
            );
            if over_cost && !self.wrapping {
                crate::diag::note(&format!(
                    "budget: cost {} micro-USD over the turn's {} ; wrap-up next",
                    self.cost_micros.unwrap_or_default(),
                    self.cfg.hooks.max_cost_micros.unwrap_or_default()
                ));
                if self.cfg.hooks.wrap_up {
                    self.wrap_pending = true;
                } else {
                    return Err(Self::over_budget(format!(
                        "The turn cost {}, over its limit of {}.",
                        dollars(self.cost_micros.unwrap_or_default()),
                        dollars(self.cfg.hooks.max_cost_micros.unwrap_or_default())
                    )));
                }
            }
            if self.usage.total() + self.prior > b.max_tokens && !self.wrapping {
                // The reserved call writes the answer (#85).
                if self.cfg.hooks.wrap_up {
                    self.wrap_pending = true;
                } else {
                    return Err(Self::over_budget(format!(
                        "The turn used {} tokens, over its limit of {}.",
                        self.usage.total() + self.prior,
                        b.max_tokens
                    )));
                }
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
                Reply::Answer(text)
                    if !file_nudged
                        && crate::webintent::is_file_refusal(&text)
                        && messages.iter().any(|m| {
                            m.role == tern_providers::Role::User
                                && m.content.contains("<attachments>")
                        }) =>
                {
                    // "I can't access files" while the file is right there
                    // as text (#70): said once, the model is pointed at it.
                    file_nudged = true;
                    self.record(self.event(
                        "model.file_refusal",
                        EffectClass::Analysis,
                        "The model said it could not open the attached file; Tern pointed it at the file's text",
                        json!({}),
                    ))?;
                    crate::diag::note(
                        "turn: the model said it cannot open files; pointed at the attachment text",
                    );
                    messages.push(ChatMessage::assistant(text));
                    messages.push(ChatMessage::user(FILE_NUDGE.to_owned()));
                    self.go(TurnState::Verifying)?;
                }
                Reply::Answer(text)
                    if self.chat_mode_folder()
                        && crate::webintent::asks_for_change(&message_text)
                        && (crate::webintent::hands_work_to_user(&text)
                            || text.to_lowercase().contains("act mode")) =>
                {
                    // In Chat mode the model handed the change back ("Please
                    // manually create reviews.md", "Would you like me to
                    // execute it?") (#80): the turn says it needs Act mode.
                    let path = crate::webintent::last_file_name(&message_text);
                    return Err(self.needs_act_stop("file_write", path.as_deref()));
                }
                Reply::Answer(text) if !claim_steered && self.failed_claim(&text).is_some() => {
                    // "Saved to counts.md" while that write failed (#80):
                    // told once what really happened.
                    claim_steered = true;
                    let (path, why) = self.failed_claim(&text).unwrap_or_default();
                    self.record(self.event(
                        "model.false_claim",
                        EffectClass::Analysis,
                        format!("The answer said {path} was written, but that write failed; Tern told the model"),
                        json!({"path": path, "error": why}),
                    ))?;
                    crate::diag::note(&format!(
                        "turn: the answer claimed {path} was written after the write failed; steered"
                    ));
                    messages.push(ChatMessage::assistant(text));
                    messages.push(ChatMessage::user(claim_note(&path, &why)));
                    self.go(TurnState::Verifying)?;
                }
                Reply::Answer(text)
                    if !refusal_nudged
                        && crate::webintent::is_flat_refusal(&text)
                        && crate::webintent::is_composition(&message_text) =>
                {
                    // "I'm sorry, but I can't assist with that request" to
                    // "Write a long essay about arrays" (#80): asked once
                    // more, neutrally.
                    refusal_nudged = true;
                    self.record(self.event(
                        "model.refused",
                        EffectClass::Analysis,
                        "The model refused an ordinary writing request; Tern asked once more",
                        json!({"chars": text.len()}),
                    ))?;
                    messages.push(ChatMessage::assistant(text));
                    messages.push(ChatMessage::user(REFUSAL_NUDGE.to_owned()));
                    self.go(TurnState::Verifying)?;
                }
                Reply::Answer(text)
                    if !confirm_nudged
                        && ["file_write", "file_edit", "file_rename"]
                            .iter()
                            .any(|t| self.cfg.tools.get(t).is_some())
                        && !self.called.iter().any(|c| {
                            matches!(c.as_str(), "file_write" | "file_edit" | "file_rename")
                        })
                        && crate::webintent::asks_for_change(&message_text)
                        && crate::webintent::asks_to_proceed(&text) =>
                {
                    // "Would you like me to proceed with adding this
                    // line?" (#78): in Act mode the approval card is the
                    // question, so the model is told once to call the tool.
                    confirm_nudged = true;
                    self.record(self.event(
                        "model.asked_in_prose",
                        EffectClass::Analysis,
                        "The model asked in prose before a change; Tern told it to call the tool so the approval card asks",
                        json!({"chars": text.len()}),
                    ))?;
                    crate::diag::note(
                        "turn: the model asked to proceed in prose; told it to call the tool",
                    );
                    messages.push(ChatMessage::assistant(text));
                    messages.push(ChatMessage::user(CONFIRM_NUDGE.to_owned()));
                    self.go(TurnState::Verifying)?;
                }
                Reply::Answer(text)
                    if !script_nudged
                        && self.cfg.tools.get("run_python").is_some()
                        && self.cfg.tools.get("file_write").is_some()
                        && !self.called.iter().any(|c| c == "run_python")
                        && crate::webintent::asks_for_script(&message_text)
                        && crate::webintent::has_python_fence(&text) =>
                {
                    // "Write one python script that ... saves counts.md"
                    // answered with the script in prose and "How to use
                    // this script" (#79): in Act mode with a folder Tern
                    // runs it, so the model is told once to call
                    // `run_python` with the tool functions.
                    script_nudged = true;
                    self.record(self.event(
                        "model.printed_script",
                        EffectClass::Analysis,
                        "The model printed a script instead of running it; Tern told it to run it with run_python",
                        json!({"chars": text.len()}),
                    ))?;
                    crate::diag::note(
                        "turn: the model printed a script instead of running it; told it to use run_python",
                    );
                    messages.push(ChatMessage::assistant(text));
                    messages.push(ChatMessage::user(SCRIPT_NUDGE.to_owned()));
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
                    // Never for writing that comes from the model itself
                    // (#80: "Write a long essay about arrays" became a web
                    // search and the answer quoted an essay site's ads).
                    if (refused || copied)
                        && !net_used
                        && self.cfg.tools.get("web_search").is_some()
                        && !crate::webintent::is_composition(&message_text)
                        && self.safety_net_allowed(&message_text)
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
                    // A `file_edit` is queued as the `file_write` it
                    // becomes, so its card is the write's from the start.
                    let raw = protocol::parse_calls(&reply.text);
                    // A `file_write` that rewrites a file the user asked to
                    // add to goes back once, toward `file_edit` (#79). One
                    // that keeps the whole old text and adds to its end is
                    // an append already (+N -0) and runs.
                    let steer = if asks_to_add && !add_steered {
                        raw.iter().enumerate().find_map(|(i, (n, a))| {
                            (n == "file_write")
                                .then(|| self.rewrite_note(a))
                                .flatten()
                                .map(|why| (i, why))
                        })
                    } else {
                        None
                    };
                    add_steered |= steer.is_some();
                    let edits: Vec<bool> = raw.iter().map(|(n, _)| n == "file_edit").collect();
                    let calls: Vec<(String, Value)> = raw
                        .into_iter()
                        .map(|(n, a)| self.expand_call(n, a))
                        .collect();
                    let mut results = Vec::with_capacity(calls.len());
                    let first = self.call_seq + 1;
                    self.call_seq += calls.len() as u32;
                    self.record(self.event(
                        "tool.queued",
                        EffectClass::Analysis,
                        format!(
                            "The model asked for {}",
                            calls
                                .iter()
                                .map(|(n, _)| format!("`{n}`"))
                                .collect::<Vec<_>>()
                                .join(", ")
                        ),
                        json!({
                            "step": self.model_calls,
                            "calls": calls.iter().enumerate().map(|(i, (n, a))| json!({
                                "call": first + i as u32, "tool": n, "args": brief_args(a),
                            })).collect::<Vec<_>>(),
                        }),
                    ))?;
                    self.batch_reads = 0;
                    for (i, (name, args)) in calls.iter().enumerate() {
                        let key = format!("{name}:{args}");
                        let n = seen.entry(key).or_default();
                        *n += 1;
                        if *n >= REPEAT_LIMIT {
                            // A change already done, asked for again
                            // after Tern said so (#82): the turn ends with
                            // that, not as a failure.
                            if let Some(skip) = self.redo_for(name, args) {
                                return Err(self.close_redo(skip));
                            }
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
                        if let Some((_, why)) = steer.as_ref().filter(|(at, _)| *at == i) {
                            self.go(TurnState::RecordResult)?;
                            crate::live::clear_preview();
                            self.record(self.event(
                                "tool.rejected",
                                EffectClass::Analysis,
                                "A `file_write` would have rewritten a file the user asked to add to; Tern asked for `file_edit`",
                                json!({"tool": name, "call": first + i as u32, "error": why, "args": brief_args(args)}),
                            ))?;
                            crate::diag::note(
                                "turn: a file_write would rewrite a file the user asked to add to; steered to file_edit",
                            );
                            results.push(why.clone());
                            continue;
                        }
                        self.next_call = Some(first + i as u32);
                        self.asked_edit = edits.get(i).copied().unwrap_or(false);
                        results.push(self.dispatch(name, args).await?);
                    }
                    let mut back = results.join("\n\n");
                    // The plan stays current (#78: a plan sat at 0 of 3
                    // done for a whole run): a line after each round of
                    // tools that did not update it.
                    if let Some(plan) = &self.plan
                        && plan.open()
                        && !calls.iter().any(|(n, _)| n == "plan")
                    {
                        back.push_str("\n\n");
                        back.push_str(&plan.reminder());
                    }
                    messages.push(ChatMessage::assistant(reply.text));
                    messages.push(ChatMessage::user(back));
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
        let writes = self.redo.marks();
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
            writes,
            journal: self.journal.iter().chain(&self.recorded).cloned().collect(),
            partial: String::new(),
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
        self.usage.cached_tokens += u.cached_tokens;
        self.usage.cache_write_tokens += u.cache_write_tokens;
        self.usage.reasoning_tokens += u.reasoning_tokens;
        if u.measure == tern_core::Measure::Estimated {
            self.usage.measure = tern_core::Measure::Estimated;
        }
    }

    fn finish(mut self, result: Result<(TurnState, String), Stop>) -> TurnOutcome {
        // A turn that needs Act mode ends as an answer saying so (#80), not
        // a failure: the window shows "Switch to Act and run".
        // So does one Tern ended because the model kept redoing a change
        // it was told is done (#82).
        let result = match (
            result,
            self.needs_act.clone().or_else(|| self.closing.clone()),
        ) {
            (Err(_), Some(text)) => {
                self.land_completed();
                Ok((TurnState::Completed, text))
            }
            (r, _) => r,
        };
        let (state, answer, reason) = match result {
            // The reserved wrap-up answered (#85): an answer with its gaps
            // listed, not a stop.
            Ok((TurnState::Completed, text)) if self.wrapping => (
                TurnState::CompletedWithGaps,
                Some(text),
                Some(
                    "The turn's budget ran out; the answer lists what was not established."
                        .to_owned(),
                ),
            ),
            Ok((state, text)) => {
                let gap = (state == TurnState::CompletedWithGaps)
                    .then(|| "The model gave an empty answer.".to_owned());
                (state, Some(text), gap)
            }
            Err(stop) => (stop.state, None, Some(stop.reason)),
        };
        // A resumed answer keeps the text that streamed before the stop
        // in front of it (#85).
        let answer = answer.map(|a| {
            if self.resumed_partial.is_empty() {
                a
            } else {
                format!("{}{a}", self.resumed_partial)
            }
        });
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
        // A plan the model never finished says where it ended (#78).
        if let Some(mut plan) = self.plan.take()
            && plan.end()
        {
            let _ = self.plan_event(&plan);
        }
        let answered = answer.is_some();
        if answered && let Some(sink) = self.cfg.hooks.checkpoints {
            let _ = sink.clear(&self.cfg.session.to_string());
        }
        let resumable = !answered && self.cfg.hooks.checkpoints.is_some() && self.ck.is_some();
        // The partial answer joins the checkpoint (#85), so Continue sends
        // it back as the start of the answer.
        if resumable && let (Some(sink), Some(ck)) = (self.cfg.hooks.checkpoints, self.ck.as_mut())
        {
            if let Some(q) = ck.quick.as_mut() {
                q.partial = format!("{}{}", self.resumed_partial, self.partial);
                q.journal = self.journal.iter().chain(&self.recorded).cloned().collect();
            }
            let _ = sink.save(ck);
        }
        // Every turn that stops before its answer leaves a receipt (#85):
        // how it stopped and what it kept.
        if !answered {
            let kept = KeptSummary {
                answer_chars: (self.resumed_partial.len() + self.partial.len()) as u64,
                sources: self.sources.clone(),
                tool_results: self.tool_results,
                failed_not_counted: self.failed_calls,
            };
            let (by, class, resets) = match (self.pause, state) {
                (Some(p), _) => (Some(p.0), Some(p.1), p.2),
                (None, TurnState::Cancelled) => (Some(PausedBy::User), None, None),
                (None, TurnState::BudgetExceeded) => {
                    (Some(PausedBy::Budget), Some(ErrorClass::Budget), None)
                }
                (None, TurnState::Blocked) => (None, Some(ErrorClass::Policy), None),
                _ => (None, None, None),
            };
            let mut receipt = match by {
                Some(by) => TurnReceipt::paused(by, class, kept),
                None => TurnReceipt {
                    state: tern_core::RunState::of_turn(state),
                    paused_by: None,
                    error: class,
                    line: reason.clone().unwrap_or_default(),
                    kept,
                    resumable,
                    resets_at: None,
                },
            };
            receipt.resumable = resumable || receipt.resumable;
            receipt.resets_at = resets.map(rfc3339);
            let line = format!("{} · {}", receipt.line, receipt.kept.line());
            let _ = self.record(self.event(
                tern_core::run_state::TURN_RECEIPT,
                EffectClass::Analysis,
                line,
                serde_json::to_value(&receipt).unwrap_or_default(),
            ));
        }
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
                    "partial": (answer.is_none() && !self.partial.trim().is_empty())
                        .then(|| self.partial.clone()),
                    "resumable": resumable,
                    "failed_not_counted": self.failed_calls,
                    "replayed": self.journal_hits,
                    "cost_micros": self.cost_micros,
                    "cached_tokens": self.usage.cached_tokens,
                    "reasoning_tokens": self.usage.reasoning_tokens,
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

impl<'a> Run<'_, 'a> {
    /// Validates, gates, runs, and records one tool call. Returns the text
    /// the model reads next.
    async fn tool_step(&mut self, name: &str, args: &Value) -> Result<String, Stop> {
        self.last_post = None;
        self.redo_skipped = false;
        let asked = if self.asked_edit { "file_edit" } else { name };
        let pre = self.tool_pre(name, args).await;
        self.asked_edit = false;
        let text = match pre? {
            Pre::Done(text) => text,
            Pre::Go(r) => {
                // A step this turn already ran (recorded in its journal
                // before a stop) is replayed, never run again (#85).
                let key = crate::toolcache::key(&r.name, &r.args);
                if let Some(j) = self.journal.iter().find(|j| j.key == key).cloned() {
                    self.go(TurnState::Executing)?;
                    self.go(TurnState::RecordResult)?;
                    self.journal_hits += 1;
                    self.record(self.event(
                        "tool.replayed",
                        r.tool.effect(),
                        format!("Used `{}`'s recorded result; it was not run again", r.name),
                        json!({"tool": r.name, "call": r.call, "step": j.step, "args": brief_args(&r.args)}),
                    ))?;
                    self.last_post = Some((true, Value::Null));
                    return Ok(j.result);
                }
                // A failed address, a paper already read, or a read past
                // this step's batch is answered without running (#85, A6,
                // A12), and never counted.
                if let Some(text) = self.skip_fetch(&r)? {
                    return Ok(text);
                }
                self.begin(&r)?;
                self.started(&r)?;
                let cancel = self.cfg.cancel.clone();
                let (out, age) = match or_cancel(
                    &cancel,
                    execute(r.tool, &r.name, r.args.clone(), self.cfg.hooks.cache),
                )
                .await
                {
                    Some(done) => done,
                    // Stop kills the running tool (#85): its future is
                    // dropped and the turn stops at the next check.
                    None => (Err(ToolError::Failed("stopped by the user".into())), None),
                };
                let (tool, ran, ran_args, call) = (r.tool, r.name.clone(), r.args.clone(), r.call);
                let failed = out.as_ref().err().map(ToString::to_string);
                let text = self.post(r, out, age)?;
                self.note_fetch(&ran, tool, &ran_args, call, failed);
                if matches!(self.last_post, Some((true, _))) {
                    self.note_done(asked, &ran, tool, &ran_args);
                    self.recorded.push(crate::checkpoint::JournalStep {
                        step: format!("{}:{call}", self.turn),
                        key,
                        tool: ran.clone(),
                        result: text.clone(),
                    });
                }
                text
            }
        };
        self.note_write(name, args, &text);
        Ok(text)
    }

    /// How many `paper_read` results one step returns (#85, A12): 3, or
    /// fewer when the model's token window is small, so the request that
    /// carries them fits it.
    fn read_batch(&self) -> u32 {
        read_batch_for(self.envelope())
    }

    /// The largest request the governor lets one call carry, in tokens.
    fn envelope(&self) -> Option<u64> {
        self.cfg
            .hooks
            .governor
            .unwrap_or(tern_providers::governor::global())
            .envelope(self.cfg.model.provider_id(), self.cfg.model_name)
    }

    /// The characters of one tool result the model reads (#85, A12): a
    /// paper's passages get their share of the window, the full text
    /// stays in the result store.
    fn result_limit(&self, name: &str) -> usize {
        if name != "paper_read" {
            return RESULT_LIMIT;
        }
        paper_share(self.envelope(), self.read_batch()).min(RESULT_LIMIT)
    }

    /// Answers a fetch without running it (#85, A6, A12): an address that
    /// already failed this turn, a paper already read for the same
    /// question, or a `paper_read` past this step's batch. `None` runs it.
    fn skip_fetch(&mut self, r: &Ready<'_>) -> Result<Option<String>, Stop> {
        let Some(key) = fetch_key(&r.name, &r.args) else {
            return Ok(None);
        };
        let (kind, summary, text) = if let Some(err) = self.failed_urls.get(&key) {
            (
                "failed_before",
                format!("`{}` was not run: {key} already failed this turn", r.name),
                format!(
                    "Tool `{}` was not run: {key} already failed in this turn ({err}). Do not ask for it again; use another source.",
                    r.name
                ),
            )
        } else if r.name == "paper_read"
            && let Some(at) = self.papers_read.get(&paper_key(&r.args))
        {
            (
                "already_read",
                format!("`paper_read` was not run: the paper was read in call {at}"),
                format!(
                    "Tool `paper_read` was not run: this paper was already read in this turn (call {at}); its passages are in that result above. Do not read it again."
                ),
            )
        } else if r.name == "paper_read" && self.batch_reads >= self.read_batch() {
            let k = self.read_batch();
            (
                "next_batch",
                format!("`paper_read` waits for the next step ({k} papers per step)"),
                format!(
                    "Tool `paper_read` was not run yet: Tern reads {k} paper{} per step so the next request fits {}'s rate window. Ask for `paper_read` of {key} again in your next reply if you still need it.",
                    if k == 1 { "" } else { "s" },
                    self.cfg.model_name
                ),
            )
        } else {
            if r.name == "paper_read" {
                self.batch_reads += 1;
            }
            return Ok(None);
        };
        self.go(TurnState::Executing)?;
        self.go(TurnState::RecordResult)?;
        crate::diag::note(&format!("fetch: skipped `{}` ({kind}) key={key}", r.name));
        self.record(self.event(
            "tool.skipped",
            r.tool.effect(),
            summary,
            json!({"tool": r.name, "call": r.call, "reason": kind, "counted": false,
                   "args": brief_args(&r.args)}),
        ))?;
        self.last_post = Some((false, Value::Null));
        Ok(Some(text))
    }

    /// Remembers a fetch that ran (#85, A6): a failed address so it is not
    /// fetched again this turn, a paper read so it is read once.
    fn note_fetch(
        &mut self,
        name: &str,
        tool: &dyn Tool,
        args: &Value,
        call: u32,
        failed: Option<String>,
    ) {
        if tool.effect() != EffectClass::Network {
            return;
        }
        let Some(key) = fetch_key(name, args) else {
            return;
        };
        match failed {
            Some(err) => {
                crate::diag::note(&format!("fetch: remembered failure key={key} error={err}"));
                self.failed_urls.insert(key, err);
            }
            None if name == "paper_read" => {
                self.papers_read.entry(paper_key(args)).or_insert(call);
            }
            None => {}
        }
    }

    /// A change that ran, for [`crate::redo`] and a script's result line
    /// (#82). `asked` is the tool the model named: only its own
    /// `file_write` calls count toward the writes per path (a `file_edit`
    /// runs as one; a script's calls are its loop).
    fn note_done(&mut self, asked: &str, name: &str, tool: &dyn Tool, args: &Value) {
        let in_script = self.script_actions.is_some();
        let line = match name {
            "file_write" => {
                let shown = args["path"].as_str().unwrap_or_default();
                if let Some(full) = tool.rule_paths(args).first() {
                    self.redo.wrote(
                        full,
                        shown,
                        args["text"].as_str().unwrap_or_default(),
                        asked == "file_write" && !in_script,
                        in_script,
                    );
                }
                if asked == "file_edit" {
                    format!("edited {shown}")
                } else {
                    format!("wrote {shown}")
                }
            }
            "file_rename" => {
                let (from, to) = (
                    args["from"].as_str().unwrap_or_default(),
                    args["to"].as_str().unwrap_or_default(),
                );
                if let [a, b] = tool.rule_paths(args).as_slice() {
                    self.redo.renamed(a, b, from, to, in_script);
                }
                format!("renamed {from} -> {to}")
            }
            "vault_write" => "saved a vault note".to_owned(),
            "run_command" => format!(
                "ran {}",
                tool.target(args).unwrap_or_else(|| "a command".to_owned())
            ),
            _ => return,
        };
        if let Some(actions) = self.script_actions.as_mut() {
            actions.push(line);
        }
    }

    /// Whether a write or rename repeats what this turn already did
    /// (#82: counts.md written nine times; a script's write and rename
    /// made again directly).
    fn redo_check(
        &self,
        asked: &str,
        name: &str,
        tool: &dyn Tool,
        args: &Value,
    ) -> Option<crate::redo::Skip> {
        match name {
            "file_write" => {
                let full = tool.rule_paths(args).into_iter().next()?;
                self.redo.check_write(
                    &full,
                    args["path"].as_str().unwrap_or_default(),
                    args["text"].as_str().unwrap_or_default(),
                    tool.old_text(args).as_deref(),
                    // A script that writes one file in a loop is not
                    // counted; the model's own repeated calls are.
                    asked == "file_write" && self.script_actions.is_none(),
                )
            }
            "file_rename" => match tool.rule_paths(args).as_slice() {
                [a, b] => self.redo.check_rename(a, b),
                _ => None,
            },
            _ => None,
        }
    }

    /// [`Run::redo_check`] for a queued call, by its tool's name.
    fn redo_for(&self, name: &str, args: &Value) -> Option<crate::redo::Skip> {
        let tool = self.cfg.tools.get(name)?;
        self.redo_check(name, name, tool, args)
    }

    /// Ends the turn as answered: the model kept redoing a change Tern
    /// told it is done (#82).
    fn close_redo(&mut self, skip: crate::redo::Skip) -> Stop {
        crate::diag::note("redo: the model kept redoing a finished change; ended the turn");
        let _ = self.record(self.event(
            "loop.suspected",
            EffectClass::Analysis,
            "The model kept redoing a change that was already done; Tern ended the turn",
            json!({"redo": true, "note": skip.line}),
        ));
        self.closing = Some(skip.end.clone());
        Stop {
            state: TurnState::Blocked,
            reason: skip.end,
        }
    }

    /// A call as it runs: a `file_edit` becomes the `file_write` of the
    /// file's new text; a call that cannot be applied stays as it is (its
    /// step says why).
    fn expand_call(&self, name: String, args: Value) -> (String, Value) {
        let tools = self.cfg.tools;
        match tools.get(&name).and_then(|t| t.expand(&args)) {
            Some(Ok((to, a))) if tools.get(to).is_some() => (to.to_owned(), a),
            _ => (name, args),
        }
    }

    /// Why a `file_write` should be a `file_edit` (#79): the file has
    /// text, and the new text does not keep all of it at the start (an
    /// append would). `None` for a new or empty file, or an append.
    fn rewrite_note(&self, args: &Value) -> Option<String> {
        self.cfg.tools.get("file_edit")?;
        let old = self.cfg.tools.get("file_write")?.old_text(args)?;
        let new = args["text"].as_str().unwrap_or_default();
        let kept = old.trim_end();
        if kept.is_empty()
            || new
                .replace("\r\n", "\n")
                .starts_with(&kept.replace("\r\n", "\n"))
        {
            return None;
        }
        let d = crate::diff::line_diff(Some(&old), new, 0);
        Some(add_nudge(
            args["path"].as_str().unwrap_or_default(),
            d.removed.max(1),
        ))
    }

    /// A call the model asked for: a task graph and a script that calls
    /// tools (code mode) run their own calls; the rest is one call (#77).
    async fn dispatch(&mut self, name: &str, args: &Value) -> Result<String, Stop> {
        let tools: &'a ToolRegistry = self.cfg.tools;
        match name {
            "run_tasks" if tools.get(name).is_some() => {
                let prepared = crate::planner::prepare_args(args);
                // A graph that plans a change the mode leaves out (#80:
                // "calls file_write, which this chat does not have").
                if let Some((tool, path)) = hidden_task(&prepared, tools) {
                    return Err(self.needs_act_stop(&tool, path.as_deref()));
                }
                self.task_step(&prepared).await
            }
            "run_python" if tools.get(name).is_some() => {
                let stubs = crate::python::stubs(tools);
                let code = args["code"].as_str().unwrap_or_default();
                // A script calling a change the mode leaves out (#80).
                let hidden: Vec<&str> = tools.registered().filter(|n| tools.hidden(n)).collect();
                if let Some(tool) = crate::python::called_names(code, &hidden).first() {
                    return Err(self.needs_act_stop(tool, None));
                }
                // A script that defines its own tool functions only
                // pretends (#80): sent back before it runs.
                let shadowed = crate::python::shadowed(code, &stubs);
                if !shadowed.is_empty() {
                    let call = self.next_call.take().unwrap_or_else(|| {
                        self.call_seq += 1;
                        self.call_seq
                    });
                    self.go(TurnState::RecordResult)?;
                    let why = crate::python::shadow_note(&shadowed);
                    crate::diag::note(&format!(
                        "code mode: refused a script that defines {}",
                        shadowed.join(", ")
                    ));
                    self.record(self.event(
                        "tool.rejected",
                        EffectClass::Analysis,
                        "The script defines Tern's tool functions itself; Tern did not run it",
                        json!({"tool": name, "call": call, "error": why, "shadowed": shadowed}),
                    ))?;
                    return Ok(format!("Tool `run_python` was not run: {why}"));
                }
                let code_mode = !stubs.is_empty()
                    && (crate::python::calls_tools(code, &stubs)
                        || crate::python::uses_pathlib(code));
                let text = if code_mode {
                    self.code_step(args, stubs.clone()).await?
                } else {
                    self.tool_step(name, args).await?
                };
                // Plain Python that reached for the disk (#78: Path.iterdir
                // in a script): the error says which functions to use.
                Ok(match crate::python::os_hint(&text, &stubs) {
                    Some(hint) => format!("{text}\n\n{hint}"),
                    None => text,
                })
            }
            "web_search" => {
                let text = self.tool_step(name, args).await?;
                self.quick_reads(text).await
            }
            _ => self.tool_step(name, args).await,
        }
    }

    /// Quick-path auto-read (#85, A13): after the first web search of a
    /// general question, the top three results are read, and the opening
    /// and best passage of each go back with the search, so the answer
    /// quotes pages Tern read rather than snippets. Once per turn; a page
    /// that fails is left out.
    async fn quick_reads(&mut self, search: String) -> Result<String, Stop> {
        if !self.cfg.hooks.quick_reads
            || self.quick_read
            || self.cfg.tools.get("read_url").is_none()
            || !crate::quickread::applies(&self.message)
        {
            return Ok(search);
        }
        let urls = crate::quickread::links(&search, crate::quickread::TOP);
        if urls.is_empty() {
            return Ok(search);
        }
        self.quick_read = true;
        let (own, _) = crate::attach::split(&self.message);
        let (_, question) = crate::reply::split(own);
        let question = question.to_owned();
        let mut read = Vec::new();
        for u in &urls {
            self.check_cancel()?;
            let text = self.tool_step("read_url", &json!({"url": u})).await?;
            if matches!(self.last_post, Some((true, _))) {
                read.push(format!(
                    "\n[{u}]\n{}",
                    crate::quickread::excerpt(&text, &question)
                ));
            }
        }
        crate::diag::note(&format!(
            "quick reads: read {} of the top {} results",
            read.len(),
            urls.len()
        ));
        if read.is_empty() {
            return Ok(search);
        }
        Ok(format!(
            "{search}\n\nTern read the top results for you (each page's opening and its passage that best matches the question). Quote from these and cite them by their numbers above:\n{}",
            read.join("\n")
        ))
    }

    /// Everything before a call runs: is it known, are its arguments
    /// right, does the sandbox refuse it, what the gate says, and the
    /// approval card when it asks.
    async fn tool_pre(&mut self, name: &str, args: &Value) -> Result<Pre<'a>, Stop> {
        let call = self.next_call.take().unwrap_or_else(|| {
            self.call_seq += 1;
            self.call_seq
        });
        let tools: &'a ToolRegistry = self.cfg.tools;
        let asked = if self.asked_edit { "file_edit" } else { name };
        // `file_edit` runs as the `file_write` of the new text (#78): the
        // same card, diff, rules and cache.
        let expanded = match tools.get(name).and_then(|t| t.expand(args)) {
            Some(Ok((to, a))) if tools.get(to).is_some() => Some((to, a)),
            Some(Err(e)) => {
                self.go(TurnState::RecordResult)?;
                self.record(self.event(
                    "tool.rejected",
                    EffectClass::Analysis,
                    format!("`{name}` could not be applied: {e}"),
                    json!({"tool": name, "call": call, "error": e, "args": brief_args(args)}),
                ))?;
                return Ok(Pre::Done(format!("Tool `{name}` failed: {e}.")));
            }
            _ => None,
        };
        let (name, args): (&str, &Value) = match &expanded {
            Some((to, a)) => (*to, a),
            None => (name, args),
        };
        let Some(tool) = tools.get(name) else {
            // A change the chat's mode leaves out (#80: `file_write` in a
            // Chat-mode folder chat): the turn ends with "Switch to Act
            // and run" instead of a failed call.
            if tools.hidden(name) && tools.mode() == Some(crate::workspace::TurnMode::Chat) {
                let path = args["path"]
                    .as_str()
                    .or_else(|| args["from"].as_str())
                    .map(str::to_owned);
                return Err(self.needs_act_stop(name, path.as_deref()));
            }
            self.go(TurnState::RecordResult)?;
            self.record(self.event(
                "tool.rejected",
                EffectClass::Analysis,
                format!("The model asked for `{name}`, which this turn does not have"),
                json!({"tool": name, "call": call}),
            ))?;
            let names = tools.names().collect::<Vec<_>>().join(", ");
            return Ok(Pre::Done(format!(
                "There is no tool called `{name}`. Available: {}.",
                if names.is_empty() { "none" } else { &names }
            )));
        };
        if let Err(e) = validate(&tool.input_schema(), args) {
            self.go(TurnState::RecordResult)?;
            // #79: a refused write never keeps the live preview its
            // streamed arguments opened.
            if name == "file_write" {
                crate::live::clear_preview();
            }
            self.record(self.event(
                "tool.rejected",
                tool.effect(),
                format!("`{name}` was asked for with bad arguments"),
                json!({"tool": name, "call": call, "error": e, "args": brief_args(args)}),
            ))?;
            return Ok(Pre::Done(format!(
                "Tool `{name}` {e}. Check its input schema."
            )));
        }
        if let Some(why) = tool.check(args) {
            // Never asked about (#78: an approval card for an empty new
            // file): the model gets the line and sends it again.
            self.go(TurnState::RecordResult)?;
            if name == "file_write" {
                crate::live::clear_preview();
            }
            self.record(self.event(
                "tool.rejected",
                tool.effect(),
                format!("`{name}` was asked for with bad arguments: {why}"),
                json!({"tool": name, "call": call, "error": why, "args": brief_args(args)}),
            ))?;
            return Ok(Pre::Done(format!("Tool `{name}` bad input: {why}.")));
        }
        // The same change again is not run and asks nothing (#82).
        if let Some(skip) = self.redo_check(asked, name, tool, args) {
            self.go(TurnState::RecordResult)?;
            if name == "file_write" {
                crate::live::clear_preview();
            }
            let done = skip.done;
            crate::diag::note(&format!(
                "redo: {} `{name}` ({})",
                if done { "skipped" } else { "refused" },
                tool.target(args).unwrap_or_default()
            ));
            let summary = if done {
                format!("`{name}` was already done this turn; Tern did not run it again")
            } else {
                format!(
                    "`{name}` was refused: the file was already written {} times this turn",
                    crate::redo::WRITES_PER_PATH
                )
            };
            let payload = json!({
                "tool": name,
                "call": call,
                "error": (!done).then(|| skip.line.clone()),
                "note": skip.line.clone(),
                "args": brief_args(args),
            });
            self.record(self.event(
                if done {
                    "tool.skipped"
                } else {
                    "tool.rejected"
                },
                tool.effect(),
                summary,
                payload,
            ))?;
            // Told once already: the model ignored "answer now".
            if self.redo.ignored(&skip) >= 2 {
                return Err(self.close_redo(skip));
            }
            self.redo_skipped = true;
            if done {
                // Already as the call would leave it: a script reads
                // success.
                self.last_post = Some((true, Value::Null));
            }
            return Ok(Pre::Done(skip.line));
        }
        let limit = self.tool_limit();
        if self.tool_calls >= limit || self.wrap_pending || self.wrapping {
            if name == "file_write" {
                crate::live::clear_preview();
            }
            // With a reserved wrap-up (#85) the call is not run and the
            // model is told to answer; otherwise the turn stops.
            if self.cfg.hooks.wrap_up && !self.wrapping {
                self.wrap_pending = true;
                self.go(TurnState::RecordResult)?;
                self.record(self.event(
                    "tool.skipped",
                    tool.effect(),
                    format!("`{name}` was not run: the turn's budget is used up"),
                    json!({"tool": name, "call": call, "note": WRAP_UP_TOOL, "args": brief_args(args)}),
                ))?;
                return Ok(Pre::Done(WRAP_UP_TOOL.to_owned()));
            }
            return Err(Self::over_budget(if self.wrapping {
                "The model kept calling tools after its budget ran out.".to_owned()
            } else {
                format!("The turn reached its limit of {limit} tool calls.")
            }));
        }

        if name == "file_write" {
            // A write whose arguments did not stream still shows in full
            // before it is approved.
            crate::live::preview_write(
                args["path"].as_str().unwrap_or_default(),
                args["text"].as_str().unwrap_or_default(),
                true,
            );
        }
        if let Some(why) = tool.refuses(args) {
            self.go(TurnState::RecordResult)?;
            if name == "file_write" {
                crate::live::clear_preview();
            }
            let what = tool.target(args).unwrap_or_else(|| name.to_owned());
            if name == "run_command" {
                crate::sandbox::note_denied(&what, std::path::Path::new(""), &why);
            } else {
                crate::diag::note(&format!("sandbox: denied `{name}`: {why}"));
            }
            self.record(self.event(
                "sandbox.denied",
                tool.effect(),
                format!("The sandbox refused `{what}`: {why}"),
                json!({"tool": name, "call": call, "args": brief_args(args), "reason": why}),
            ))?;
            return Ok(Pre::Done(format!(
                "Tool `{name}` was refused by Tern's sandbox: {why}"
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
        let facts = crate::permissions::CallFacts {
            tool: name,
            host: None,
            paths: tool.rule_paths(args),
            detail: crate::policy::call_detail_line(args),
            folder: None,
            home: None,
        };
        let decision = self
            .cfg
            .gate
            .check_full(&view, asks.is_some(), &facts, tool.confined(args));
        let why = match &risk {
            Some(Ok(why)) => Some(why.clone()),
            _ => asks,
        };
        let mut decided = json!({"tool": name, "call": call, "target": target, "decision": decision, "flag": why,
               "detail": call_detail(name, args), "mode": self.cfg.gate.config().approval.id()});
        if self.parent.is_some() {
            // A call inside a script or a graph has no queued event: its
            // card starts here, with its arguments.
            decided["args"] = brief_args(args);
        }
        self.record(self.event(
            "policy.decided",
            tool.effect(),
            match (&decision, &why) {
                (Decision::Allow { because }, _) => format!("`{name}` allowed: {because}"),
                (Decision::Ask, Some(why)) => format!("`{name}` needs your approval: {why}"),
                (Decision::Ask, None) => format!("`{name}` needs your approval"),
                (Decision::Deny { reason }, _) => reason.clone(),
            },
            decided,
        ))?;
        match decision {
            Decision::Allow { .. } => {}
            Decision::Deny { reason } => {
                crate::diag::note(&format!("policy: refused `{name}`: {reason}"));
                if name == "file_write" {
                    crate::live::clear_preview();
                }
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
                let mut asked = serde_json::to_value(&req).unwrap_or_default();
                asked["call"] = json!(call);
                if name == "file_write" {
                    asked["args"] = brief_args(args);
                }
                self.record(self.event(
                    "approval.requested",
                    tool.effect(),
                    format!("Asked you to approve `{name}`"),
                    asked,
                ))?;
                crate::diag::note(&format!(
                    "approval: asked for `{name}` ({}){}",
                    self.cfg.gate.config().approval.id(),
                    why.as_deref().map(|w| format!(": {w}")).unwrap_or_default()
                ));
                let answer = self.cfg.approver.ask(&req).await;
                let allowed = self.cfg.gate.answer(&view, answer);
                crate::diag::note(&format!("approval: `{name}` answered {answer:?}"));
                self.record(self.event(
                    "approval.answered",
                    tool.effect(),
                    if allowed {
                        format!("You approved `{name}`")
                    } else {
                        format!("You declined `{name}`")
                    },
                    json!({"tool": name, "call": call, "answer": answer}),
                ))?;
                if !allowed && name == "file_write" {
                    crate::live::clear_preview();
                }
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
        Ok(Pre::Go(Ready {
            call,
            name: name.to_owned(),
            args: args.clone(),
            tool,
            target,
            task: self.task.clone(),
        }))
    }

    /// The call starts: the state moves on and it counts.
    fn begin(&mut self, _r: &Ready<'_>) -> Result<(), Stop> {
        self.go(TurnState::Executing)?;
        self.tool_calls += 1;
        Ok(())
    }

    /// Logs that the call is running.
    fn started(&mut self, r: &Ready<'_>) -> Result<(), Stop> {
        let task = std::mem::replace(&mut self.task, r.task.clone());
        let mut payload = json!({"tool": r.name, "call": r.call, "args": brief_args(&r.args)});
        if r.name == "file_write" {
            // +N −M against the file as it is before the write (#79), so
            // the card shows it however the write was allowed.
            let old = r.tool.old_text(&r.args);
            let d = crate::diff::line_diff(
                old.as_deref(),
                r.args["text"].as_str().unwrap_or_default(),
                0,
            );
            payload["diff"] = json!({"added": d.added, "removed": d.removed});
        }
        let res = self.record(self.event(
            "tool.started",
            r.tool.effect(),
            format!("Running `{}`", r.name),
            payload,
        ));
        self.task = task;
        res
    }

    /// Records a call's result and gives the text the model reads.
    fn post(
        &mut self,
        r: Ready<'_>,
        out: Result<ToolOutput, ToolError>,
        cached: Option<std::time::Duration>,
    ) -> Result<String, Stop> {
        let task = std::mem::replace(&mut self.task, r.task.clone());
        let res = self.post_inner(&r, out, cached);
        self.task = task;
        res
    }

    fn post_inner(
        &mut self,
        r: &Ready<'_>,
        out: Result<ToolOutput, ToolError>,
        cached: Option<std::time::Duration>,
    ) -> Result<String, Stop> {
        let (name, args, tool, call) = (r.name.as_str(), &r.args, r.tool, r.call);
        let view = CallView {
            tool: name,
            effect: tool.effect(),
            target: r.target.as_deref(),
            args,
        };
        self.cfg.gate.consume(&view);
        if self.fsm.state() != TurnState::RecordResult {
            self.go(TurnState::RecordResult)?;
        }
        if let Ok(o) = &out {
            self.note_provenance(name, o);
            self.tool_results += 1;
            if let Some(u) = o.meta["page"]["url"].as_str()
                && !self.sources.iter().any(|s| s == u)
            {
                self.sources.push(u.to_owned());
            }
        }
        self.last_post = Some(match &out {
            Ok(o) => (true, o.meta.get("py").cloned().unwrap_or(Value::Null)),
            Err(_) => (false, Value::Null),
        });
        Ok(match out {
            Ok(o) => {
                // A long result keeps its head and tail; the whole text is
                // saved for `read_result` (#72). A `read_result` page is
                // never saved again.
                let limit = self.result_limit(name);
                let cut = o.content.len() > limit;
                let saved = match self.cfg.hooks.results {
                    Some(store) if cut && name != "read_result" => {
                        let id = result_id(&self.turn.to_string(), call);
                        store.save(&id, &o.content).ok().map(|_| id)
                    }
                    _ => None,
                };
                let shown = crate::results::head_tail(&o.content, limit, saved.as_deref());
                let mut p = json!({"tool": name, "call": call, "version": tool.version(), "args": args, "chars": o.content.len(), "truncated": cut, "saved_as": saved, "meta": o.meta, "output": output_head_tail(&o.content)});
                if let Some(age) = cached {
                    p["cached"] = json!(true);
                    p["cached_age_s"] = json!(age.as_secs());
                }
                self.record(
                    self.event(
                        "tool.executed",
                        tool.effect(),
                        if cached.is_some() {
                            format!("Ran `{name}` (cached result)")
                        } else {
                            format!("Ran `{name}`")
                        },
                        p,
                    )
                    .with_evidence(o.evidence_refs.clone()),
                )?;
                if name == "plan"
                    && let Ok(plan) =
                        serde_json::from_value::<crate::planner::PlanView>(o.meta["plan"].clone())
                {
                    self.plan_event(&plan)?;
                    self.plan = Some(plan);
                }
                if cut {
                    format!(
                        "Result of `{name}` ({} characters; the middle was cut, see the note in it):\n{shown}",
                        o.content.chars().count()
                    )
                } else {
                    format!("Result of `{name}`:\n{shown}")
                }
            }
            Err(e) => {
                // A failed call is logged but never counted against the
                // turn's budget (#85).
                self.tool_calls = self.tool_calls.saturating_sub(1);
                self.failed_calls += 1;
                self.record(self.event(
                    "tool.failed",
                    tool.effect(),
                    format!("`{name}` {e}"),
                    json!({"tool": name, "call": call, "args": args, "error": e, "retry_safe": tool.idempotent(), "counted": false}),
                ))?;
                format!("Tool `{name}` {e}.")
            }
        })
    }

    /// Logs a plan or a task graph as it stands (`plan.updated`).
    fn plan_event(&self, v: &crate::planner::PlanView) -> Result<(), Stop> {
        self.record(self.event(
            "plan.updated",
            EffectClass::Analysis,
            v.line(),
            serde_json::to_value(v).unwrap_or_default(),
        ))
    }

    /// Code mode (#77): a `run_python` script that calls tools. Each of
    /// its calls is a call of this turn, through the gate (a write still
    /// shows its card), nested under the script's card; only what the
    /// script prints comes back to the model.
    async fn code_step(
        &mut self,
        args: &Value,
        stubs: Vec<crate::python::Stub>,
    ) -> Result<String, Stop> {
        let r = match self.tool_pre("run_python", args).await? {
            Pre::Done(text) => return Ok(text),
            Pre::Go(r) => r,
        };
        self.begin(&r)?;
        self.started(&r)?;
        let code = args["code"].as_str().unwrap_or_default().to_owned();
        if code.len() > 200_000 {
            return self.post(
                r,
                Err(ToolError::BadInput(
                    "the program is longer than 200 KB".into(),
                )),
                None,
            );
        }
        crate::diag::note(&format!(
            "code mode: script with {} lines, {} tools callable",
            code.lines().count(),
            stubs.len()
        ));
        let mut script = crate::python::Script::start(code, stubs);
        let outer = self.parent.replace(r.call);
        let outer_actions = self.script_actions.replace(Vec::new());
        let mut inner = 0u32;
        let ended = loop {
            match script.next().await {
                crate::python::ScriptEvent::Call { tool, args } => {
                    if let Err(stop) = self.check_cancel() {
                        script.stop();
                        break Err(stop);
                    }
                    inner += 1;
                    self.called.push(tool.clone());
                    if let Err(stop) = self.go(TurnState::ToolRequested) {
                        script.stop();
                        break Err(stop);
                    }
                    match self.tool_step(&tool, &args).await {
                        // A call that ran gives its value (a list for
                        // `files_list`) or its text; one that failed or
                        // was refused raises ToolError in the script
                        // (#80: a failed write came back as text and the
                        // script printed "saved").
                        Ok(text) => match self.last_post.take() {
                            Some((true, py)) if !py.is_null() => script.answer(Ok(py)),
                            Some((true, _)) => {
                                script.answer(Ok(Value::String(strip_result_header(&text))))
                            }
                            _ => script.answer(Err(crate::python::tool_error(&text))),
                        },
                        Err(stop) => {
                            script.answer(Err(stop.reason.clone()));
                            script.stop();
                            break Err(stop);
                        }
                    }
                }
                crate::python::ScriptEvent::Done(out) => break Ok(out),
            }
        };
        self.parent = outer;
        let actions =
            std::mem::replace(&mut self.script_actions, outer_actions).unwrap_or_default();
        crate::diag::note(&format!("code mode: {inner} tool calls inside the script"));
        let out = ended?;
        let out = out.map(|mut o| {
            o.meta["inner_calls"] = json!(inner);
            o
        });
        let mut text = self.post(r, out, None)?;
        // What the script changed, said plainly (#82: the model made the
        // script's write and rename again as direct calls).
        if let Some(line) = crate::redo::script_done_line(&actions) {
            text.push_str("\n\n");
            text.push_str(&line);
        }
        // A script that called none of the tools while the user asked for
        // a file action did nothing to the folder (#80: "code mode: 0 tool
        // calls" and an answer saying draft.md was written): said once.
        if inner == 0 && !self.script_noted && crate::webintent::asks_for_file_change(&self.message)
        {
            self.script_noted = true;
            crate::diag::note("code mode: the script called no tools; told the model");
            return Ok(format!("{text}\n\n{}", crate::python::NO_CALLS_NOTE));
        }
        Ok(text)
    }

    /// A task graph (#77): the tasks in waves, a wave's reads and
    /// searches at the same time ([`crate::planner::PARALLEL`] at once),
    /// its changes one by one after them; every task through the gate,
    /// each finished task's result fed to the tasks that wait for it.
    async fn task_step(&mut self, args: &Value) -> Result<String, Stop> {
        use crate::planner::{self, StepStatus};
        let r = match self.tool_pre("run_tasks", args).await? {
            Pre::Done(text) => return Ok(text),
            Pre::Go(r) => r,
        };
        self.begin(&r)?;
        self.started(&r)?;
        let tools: &'a ToolRegistry = self.cfg.tools;
        let graph = match planner::read_graph(&r.args, &|t: &str| tools.get(t).is_some()) {
            Ok(g) => g,
            Err(e) => {
                crate::diag::note(&format!("task graph: refused: {e}"));
                return self.post(r, Err(ToolError::BadInput(e)), None);
            }
        };
        let mut view = graph.view();
        crate::diag::note(&format!(
            "task graph: {} tasks in {} waves{}",
            graph.tasks.len(),
            view.waves,
            if graph.notes.is_empty() {
                String::new()
            } else {
                format!(" (repaired: {})", graph.notes.join("; "))
            }
        ));
        self.plan_event(&view)?;
        let outer = self.parent.replace(r.call);
        let mut results: std::collections::BTreeMap<String, String> = Default::default();
        let ran = self.run_waves(&graph, &mut view, &mut results).await;
        self.parent = outer;
        self.task = None;
        ran?;
        let mut text = format!(
            "Task graph: {} tasks in {} waves; {} done, {} failed, {} skipped.",
            graph.tasks.len(),
            view.waves,
            view.count(StepStatus::Done),
            view.count(StepStatus::Failed),
            view.count(StepStatus::Skipped),
        );
        if !graph.notes.is_empty() {
            text.push_str(&format!(" Tern fixed: {}.", graph.notes.join("; ")));
        }
        for (t, st) in graph.tasks.iter().zip(&view.steps) {
            text.push_str(&format!(
                "\n\n### Task {} · {} · {}\n{}",
                t.id,
                t.tool,
                st.status.label(),
                results.get(&t.id).map(String::as_str).unwrap_or("")
            ));
        }
        self.post(
            r,
            Ok(ToolOutput {
                content: text,
                evidence_refs: Vec::new(),
                meta: json!({"graph": view}),
            }),
            None,
        )
    }

    async fn run_waves(
        &mut self,
        graph: &crate::planner::Graph,
        view: &mut crate::planner::PlanView,
        results: &mut std::collections::BTreeMap<String, String>,
    ) -> Result<(), Stop> {
        use crate::planner::{self, StepStatus};
        let tools: &'a ToolRegistry = self.cfg.tools;
        let cache = self.cfg.hooks.cache;
        let mut failed: std::collections::BTreeSet<String> = Default::default();
        let ids: Vec<String> = graph.tasks.iter().map(|t| t.id.clone()).collect();
        for wave in graph.waves() {
            self.check_cancel()?;
            let mut parallel = Vec::new();
            let mut serial = Vec::new();
            for i in wave {
                let t = &graph.tasks[i];
                if let Some(d) = t.after.iter().find(|d| failed.contains(*d)) {
                    failed.insert(t.id.clone());
                    view.steps[i].status = StepStatus::Skipped;
                    results.insert(
                        t.id.clone(),
                        format!("(skipped: task {d}, which it needs, did not finish)"),
                    );
                    continue;
                }
                match tools.get(&t.tool) {
                    Some(tool) if planner::parallel_safe(tool.effect()) => parallel.push(i),
                    _ => serial.push(i),
                }
            }
            for chunk in parallel.chunks(planner::PARALLEL) {
                let mut ready: Vec<(usize, Ready<'a>)> = Vec::new();
                for &i in chunk {
                    let t = &graph.tasks[i];
                    if let Some(why) = planner::missing_input(&t.args, &ids, results) {
                        failed.insert(t.id.clone());
                        view.steps[i].status = StepStatus::Failed;
                        results.insert(t.id.clone(), format!("(failed: {why})"));
                        continue;
                    }
                    let fed = planner::feed(&t.args, results);
                    self.task = Some(t.id.clone());
                    self.go(TurnState::ToolRequested)?;
                    self.called.push(t.tool.clone());
                    match self.tool_pre(&t.tool, &fed).await? {
                        Pre::Done(text) => {
                            failed.insert(t.id.clone());
                            view.steps[i].status = StepStatus::Failed;
                            results.insert(t.id.clone(), text);
                        }
                        Pre::Go(r) => {
                            self.begin(&r)?;
                            view.steps[i].status = StepStatus::InProgress;
                            view.steps[i].call = Some(r.call);
                            ready.push((i, r));
                        }
                    }
                }
                self.task = None;
                if ready.is_empty() {
                    continue;
                }
                self.plan_event(view)?;
                for (_, r) in &ready {
                    self.started(r)?;
                }
                let outs = futures::future::join_all(
                    ready
                        .iter()
                        .map(|(_, r)| execute(r.tool, &r.name, r.args.clone(), cache)),
                )
                .await;
                for ((i, r), (out, age)) in ready.into_iter().zip(outs) {
                    let id = graph.tasks[i].id.clone();
                    let ok = out.is_ok();
                    let text = self.post(r, out, age)?;
                    view.steps[i].status = if ok {
                        StepStatus::Done
                    } else {
                        failed.insert(id.clone());
                        StepStatus::Failed
                    };
                    results.insert(id, strip_result_header(&text));
                }
                self.plan_event(view)?;
            }
            for i in serial {
                self.check_cancel()?;
                let t = &graph.tasks[i];
                // An unfilled or empty `{{id}}` never reaches a change
                // (#78: a write of an empty counts.md).
                if let Some(why) = planner::missing_input(&t.args, &ids, results) {
                    crate::diag::note(&format!("task graph: {} failed: {why}", t.id));
                    failed.insert(t.id.clone());
                    view.steps[i].status = StepStatus::Failed;
                    results.insert(t.id.clone(), format!("(failed: {why})"));
                    self.plan_event(view)?;
                    continue;
                }
                let fed = planner::feed(&t.args, results);
                self.task = Some(t.id.clone());
                self.go(TurnState::ToolRequested)?;
                self.called.push(t.tool.clone());
                view.steps[i].status = StepStatus::InProgress;
                view.steps[i].call = Some(self.call_seq + 1);
                self.plan_event(view)?;
                let text = self.tool_step(&t.tool, &fed).await?;
                self.task = None;
                let ok = !text.starts_with(&format!("Tool `{}`", t.tool))
                    && !text.starts_with("There is no tool");
                view.steps[i].status = if ok {
                    StepStatus::Done
                } else {
                    failed.insert(t.id.clone());
                    StepStatus::Failed
                };
                results.insert(t.id.clone(), strip_result_header(&text));
                self.plan_event(view)?;
            }
        }
        Ok(())
    }
}

/// Runs a checked call: a kept result when the cache has a fresh one
/// (with its age), else the tool itself within its time limit. Results
/// worth keeping are kept; a change empties the kept file reads (#77).
/// Runs `fut` until it finishes or `cancel` is set (#85: Stop works
/// within a second even while a stream or a tool is silent). `None` when
/// it was stopped; the future is dropped, which closes its connection.
async fn or_cancel<T>(cancel: &CancelFlag, fut: impl std::future::Future<Output = T>) -> Option<T> {
    let watch = async {
        while !cancel.is_cancelled() {
            tokio::time::sleep(std::time::Duration::from_millis(100)).await;
        }
    };
    tokio::select! {
        v = fut => Some(v),
        () = watch => None,
    }
}

/// The [`ErrorClass`] of a provider error. `streamed` says whether any
/// reply text arrived before it, which makes a dropped connection a cut
/// stream.
/// `None` when the error fits no class (a spending cap, a bad request).
pub fn error_class(e: &tern_providers::ProviderError, streamed: bool) -> Option<ErrorClass> {
    use tern_providers::ProviderError as P;
    use tern_providers::retry::Class;
    Some(match (e, tern_providers::retry::classify(e)) {
        (P::Auth { .. } | P::MissingKey { .. }, _) => ErrorClass::Auth,
        (_, Class::Daily) => ErrorClass::RateLimitDaily,
        (P::Network { .. }, _) if streamed => ErrorClass::StreamCut,
        (P::RateLimited { .. }, Class::Transient { .. }) => ErrorClass::RateLimitShort,
        (P::Unavailable { .. }, Class::Transient { .. }) => ErrorClass::Overloaded,
        (P::Network { .. }, Class::Transient { .. }) => ErrorClass::Network,
        _ if tern_providers::retry::request_cap(e).is_some() => ErrorClass::ContextFull,
        _ => return None,
    })
}

/// `paper_read` results per step for a request envelope of `envelope`
/// tokens (#85, A12): 3 by default, 2 under 24k tokens, 1 under 12k.
pub fn read_batch_for(envelope: Option<u64>) -> u32 {
    match envelope {
        Some(e) if e < 12_000 => 1,
        Some(e) if e < 24_000 => 2,
        _ => 3,
    }
}

/// The characters one paper's passages may take in the next request
/// (#85, A12): the envelope shared by the batch and the rest of the
/// request, at least 2,000.
pub fn paper_share(envelope: Option<u64>, batch: u32) -> usize {
    match envelope {
        Some(e) => ((e as usize * CHARS_PER_TOKEN) / (batch as usize + 1)).max(2_000),
        None => usize::MAX,
    }
}

/// The address a network call fetches, for the failed-address memory
/// (#85, A6): its `url` without a fragment or a trailing slash, or the
/// paper id `paper_read` was given. `None` for calls that fetch no
/// address (a search).
pub fn fetch_key(name: &str, args: &Value) -> Option<String> {
    if name == "paper_read" {
        return args["paper"].as_str().map(paper_id);
    }
    let url = args["url"].as_str()?.trim();
    let url = url.split('#').next().unwrap_or(url);
    let url = url.trim_end_matches('/');
    (!url.is_empty()).then(|| url.to_owned())
}

/// A paper id as one form (#85, A6): a DOI or arXiv id without its
/// prefix or link, lower case.
pub fn paper_id(paper: &str) -> String {
    let p = paper.trim().to_ascii_lowercase();
    let p = p.trim_end_matches('/');
    for prefix in [
        "https://doi.org/",
        "http://doi.org/",
        "https://dx.doi.org/",
        "doi:",
        "https://arxiv.org/abs/",
        "http://arxiv.org/abs/",
        "arxiv:",
    ] {
        if let Some(rest) = p.strip_prefix(prefix) {
            return rest.trim().to_owned();
        }
    }
    p.to_owned()
}

/// A `paper_read` call's paper and question, for the read-once memory.
fn paper_key(args: &Value) -> String {
    format!(
        "{}|{}",
        paper_id(args["paper"].as_str().unwrap_or_default()),
        args["question"]
            .as_str()
            .unwrap_or_default()
            .trim()
            .to_lowercase()
    )
}

/// Micro-dollars as dollars, for a budget line ("$0.0123").
pub fn dollars(micros: u64) -> String {
    format!("${:.4}", micros as f64 / 1_000_000.0)
}

/// The log line for one model call (#85, A8).
pub fn call_line(
    provider: &str,
    model: &str,
    call: u32,
    u: &Usage,
    estimated_in: u64,
    took: std::time::Duration,
    failed_so_far: u32,
) -> String {
    format!(
        "call: n={call} provider={provider} model={model} in={} out={} cached={} cache_write={} reasoning={} est_in={estimated_in} measured={} ms={} failed_not_counted={failed_so_far}",
        u.input_tokens,
        u.output_tokens,
        u.cached_tokens,
        u.cache_write_tokens,
        u.reasoning_tokens,
        u.measure == tern_core::Measure::Measured,
        took.as_millis()
    )
}

/// "The daily limit for X is used up; it resets at 05:00 (UTC)."
pub fn daily_line(model: &str, resets_at: std::time::SystemTime) -> String {
    format!(
        "The daily limit for {model} is used up, so Tern did not retry; it resets at {} (UTC). Continue with another model, or continue after the reset.",
        rfc3339(resets_at)
    )
}

/// A wall-clock time as RFC 3339 in UTC, to the second.
fn rfc3339(t: std::time::SystemTime) -> String {
    let secs = t
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0);
    time::OffsetDateTime::from_unix_timestamp(secs)
        .ok()
        .and_then(|d| {
            d.format(&time::format_description::well_known::Rfc3339)
                .ok()
        })
        .unwrap_or_default()
}

/// Characters per script in what a request sends (#85, A3b): the system
/// text, every message and the tool schemas, decoded from any JSON
/// escapes.
pub fn request_census(req: &ChatRequest) -> [u64; 4] {
    use tern_providers::estimate::{census, unescape_json};
    let mut n = [0u64; 4];
    let mut add = |t: &str| {
        for (a, b) in n.iter_mut().zip(census(&unescape_json(t))) {
            *a += b;
        }
    };
    if let Some(s) = &req.system {
        add(s);
    }
    for m in &req.messages {
        add(&m.content);
    }
    for t in &req.tools {
        add(&t.name);
        add(&t.description);
        add(&t.parameters.to_string());
    }
    n
}

async fn execute(
    tool: &dyn Tool,
    name: &str,
    args: Value,
    cache: Option<&crate::toolcache::ToolCache>,
) -> (Result<ToolOutput, ToolError>, Option<std::time::Duration>) {
    if let Some(c) = cache
        && let Some(hit) = c.get(name, &args)
    {
        crate::diag::note(&format!(
            "cache: hit for `{name}` ({} s old)",
            hit.age.as_secs()
        ));
        return (Ok(hit.output), Some(hit.age));
    }
    let out = match tokio::time::timeout(tool.timeout(), tool.call(args.clone())).await {
        Ok(r) => r,
        Err(_) => Err(ToolError::TimedOut),
    };
    if let (Some(c), Ok(o)) = (cache, &out) {
        c.put(name, &args, o);
        if crate::toolcache::changes_files(name) {
            c.forget_local();
        }
    }
    (out, None)
}

/// Characters per token, for the estimates that fit a request under a
/// provider's cap.
const CHARS_PER_TOKEN: usize = 4;

/// Tools a compact request keeps besides the ones the turn already used.
const CORE_TOOLS: &[&str] = &[
    "files_list",
    "file_read",
    "file_write",
    "file_edit",
    "web_search",
    "read_url",
];

/// `text` cut to about `max` characters: its head and tail.
fn squeeze(text: &str, max: usize) -> String {
    let n = text.chars().count();
    if n <= max {
        return text.to_owned();
    }
    let head: String = text.chars().take(max * 2 / 3).collect();
    let tail: String = text.chars().skip(n - max / 3).collect();
    format!("{head}\n[… cut to fit the model's request limit …]\n{tail}")
}

/// A request made to fit a provider's token cap (#80): the earlier chat
/// cut to its last exchange, long messages to their head and tail, the
/// tool list to one line each (and only the tools the turn needs), and
/// the reply's room to what is left. `Err` with the tokens it would need
/// when even that does not fit.
pub fn fit_request(
    mut req: ChatRequest,
    base: usize,
    cap: tern_providers::retry::RequestCap,
    tools: &ToolRegistry,
    called: &[String],
) -> Result<ChatRequest, u64> {
    let keep = |n: &str| CORE_TOOLS.contains(&n) || called.iter().any(|c| c == n);
    // The system prompt: Tern's opening and the chat's instructions stay;
    // the tool list becomes one line per tool.
    if let Some(sys) = req.system.take() {
        let head = sys
            .split("\n\nYou can use these tools:\n")
            .next()
            .unwrap_or(&sys)
            .to_owned();
        let mut out = squeeze(&head, 3_000);
        if !tools.is_empty() {
            out.push_str("\n\nYou can use these tools:\n");
            for n in tools.names().filter(|n| keep(n)) {
                if let Some(t) = tools.get(n) {
                    let first = t.summary().split(". ").next().unwrap_or_default();
                    out.push_str(&format!("- {n}: {}\n", squeeze(first, 120)));
                }
            }
            out.push_str(
                "To use one, reply with only a ```tool block holding {\"tool\": \"<name>\", \"args\": {...}}. Never say a file was saved unless its tool result says so.",
            );
        }
        req.system = Some(out);
    }
    req.tools.retain(|t| keep(&t.name));
    for t in &mut req.tools {
        t.description = squeeze(t.description.split(". ").next().unwrap_or_default(), 120);
    }
    let base = base.min(req.messages.len());
    let turn = req.messages.split_off(base);
    let earlier = req.messages.len();
    let mut messages: Vec<ChatMessage> = req
        .messages
        .drain(earlier.saturating_sub(2)..)
        .map(|mut m| {
            m.content = squeeze(&m.content, 600);
            m
        })
        .collect();
    for (i, mut m) in turn.into_iter().enumerate() {
        m.content = squeeze(&m.content, if i == 0 { 4_000 } else { 2_000 });
        messages.push(m);
    }
    req.messages = messages;
    let chars = req.system.as_deref().map_or(0, |s| s.chars().count())
        + req
            .messages
            .iter()
            .map(|m| m.content.chars().count() + 8)
            .sum::<usize>()
        + req
            .tools
            .iter()
            .map(|t| t.name.len() + t.description.len() + t.parameters.to_string().len())
            .sum::<usize>();
    let input = (chars / CHARS_PER_TOKEN + 64) as u64;
    let floor = 256u64;
    if input + floor > cap.limit {
        return Err(input + floor);
    }
    let room = cap.limit - input - 32;
    req.max_tokens = req.max_tokens.min(room.min(u64::from(u32::MAX)) as u32);
    Ok(req)
}

/// The line for a request that cannot fit a model's cap (#80).
pub fn too_large_line(
    model: &str,
    provider: &str,
    cap: tern_providers::retry::RequestCap,
    need: u64,
) -> String {
    let name = tern_providers::find(provider).map_or(provider, |s| s.name);
    format!(
        "{model} on your {name} plan accepts {} tokens per request; this chat needs about {need}. Pick a model with a larger limit or start a new chat.",
        cap.limit
    )
}

/// The first task of a graph whose tool the turn's mode leaves out, with
/// its file when it names one (#80).
fn hidden_task(args: &Value, tools: &ToolRegistry) -> Option<(String, Option<String>)> {
    let graph =
        crate::planner::read_graph(args, &|t: &str| tools.get(t).is_some() || tools.hidden(t))
            .ok()?;
    graph
        .tasks
        .into_iter()
        .find(|t| tools.hidden(&t.tool))
        .map(|t| {
            let path = t.args["path"].as_str().map(str::to_owned);
            (t.tool, path)
        })
}

/// A call's text without the "Result of `x`:" line the model gets, for
/// a script or a dependent task.
pub fn strip_result_header(text: &str) -> String {
    if text.starts_with("Result of `")
        && let Some((_, rest)) = text.split_once('\n')
    {
        return rest.to_owned();
    }
    text.to_owned()
}

/// A saved result's id: the end of the turn's id and the call's number
/// in the turn (`t3k9qz-2`), letters, digits and `-` only.
pub fn result_id(turn: &str, call: u32) -> String {
    let tail: String = turn
        .chars()
        .rev()
        .filter(char::is_ascii_alphanumeric)
        .take(6)
        .collect::<Vec<_>>()
        .into_iter()
        .rev()
        .collect();
    format!("t{}-{call}", tail.to_ascii_lowercase())
}

/// A call's arguments for the log and the cards: a long `text` or `code`
/// is cut to its first 2,000 characters with its length noted.
pub fn brief_args(args: &Value) -> Value {
    let mut a = args.clone();
    if let Some(obj) = a.as_object_mut() {
        for k in ["text", "code", "content"] {
            if let Some(Value::String(t)) = obj.get(k)
                && t.chars().count() > 2000
            {
                let n = t.chars().count();
                let head: String = t.chars().take(2000).collect();
                obj.insert(
                    k.to_owned(),
                    Value::String(format!("{head}… [{n} characters]")),
                );
            }
        }
    }
    a
}

/// A tool's output for its card (#75): all of it up to 3,000 characters,
/// else the first 2,000 and the last 1,000 with the cut marked.
pub fn output_head_tail(text: &str) -> String {
    let n = text.chars().count();
    if n <= 3000 {
        return text.to_owned();
    }
    let head: String = text.chars().take(2000).collect();
    let tail: String = text.chars().skip(n - 1000).collect();
    format!("{head}\n… [{} characters cut] …\n{tail}", n - 3000)
}

/// Feeds one streamed piece of a native tool call to the live preview of
/// a file being written (`file_write`).
fn stream_write_preview(
    writes: &Mutex<HashMap<usize, (String, String, usize)>>,
    index: usize,
    name: Option<&str>,
    args: &str,
) {
    let Ok(mut w) = writes.lock() else {
        return;
    };
    let e = w.entry(index).or_default();
    if let Some(n) = name {
        e.0 = n.to_owned();
    }
    e.1.push_str(args);
    // Parse again every 200 bytes or so: the preview keeps up without
    // reading the whole text on every piece.
    if e.0 != "file_write" || (e.1.len() < e.2 + 200 && !e.1.ends_with('}')) {
        return;
    }
    e.2 = e.1.len();
    // #79: no preview until there is text to show (an empty write is
    // sent back before it runs, and its preview read "+0 −0 writing…").
    if let (Some(path), Some(text)) = crate::live::partial_write_args(&e.1)
        && !text.is_empty()
    {
        crate::live::preview_write(&path, &text, false);
    }
}

/// The live preview for a `file_write` asked for in a ```` ```tool ````
/// fence in the reply text (providers without native tool calls): the
/// fence's JSON so far is read like native arguments. Uses slot
/// `usize::MAX` of `writes` to remember how far it last parsed.
fn stream_fence_preview(streamed: &str, writes: &Mutex<HashMap<usize, (String, String, usize)>>) {
    let Some(at) = streamed.rfind("```tool") else {
        return;
    };
    let fence = &streamed[at + 7..];
    if !fence.contains("\"file_write\"") {
        return;
    }
    let Ok(mut w) = writes.lock() else {
        return;
    };
    let e = w.entry(usize::MAX).or_default();
    if streamed.len() < e.2 + 200 && !fence.trim_end().ends_with("```") {
        return;
    }
    e.2 = streamed.len();
    if let (Some(path), Some(text)) = crate::live::partial_write_args(fence)
        && !text.is_empty()
    {
        crate::live::preview_write(&path, &text, false);
    }
}

#[cfg(test)]
mod length_tests {
    #[test]
    fn long_arguments_and_outputs_are_cut_for_cards() {
        let long = "x".repeat(5000);
        let a = super::brief_args(&serde_json::json!({"path": "a.md", "text": long}));
        assert!(a["text"].as_str().unwrap().ends_with("[5000 characters]"));
        assert_eq!(a["path"], "a.md");
        let o = super::output_head_tail(&"y".repeat(4000));
        assert!(o.contains("[1000 characters cut]"));
        assert_eq!(super::output_head_tail("short"), "short");
    }

    #[test]
    fn result_ids_are_short_and_safe() {
        assert_eq!(super::result_id("trn_01JBXK3K9QZ", 2), "tk3k9qz-2");
        assert_eq!(super::result_id("", 1), "t-1");
    }

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
