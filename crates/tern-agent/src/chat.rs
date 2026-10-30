//! A chat session for front ends: one provider, one model, one stored session.
//!
//! The TUI and the desktop app open a [`ChatSession`] and call
//! [`ChatSession::send`] per message. They never touch providers, the
//! keychain, or the database themselves.

use std::sync::Arc;

use serde_json::json;
use tern_core::{EffectClass, EventEnvelope, Measure, SessionId};
use tern_platform::AppPaths;
use tern_providers::{ChatMessage, Provider, UsageLedger};
use tern_store::{Harness, SessionSummary, Store};

pub use tern_providers::Usage;

use crate::engine::{Approver, Budget, CancelFlag, EventSink, StoreSink, TurnLoop, TurnOutcome};
use crate::policy::PolicyGate;
use crate::providers::{self, SecretStore, SetupError};
use crate::search;
use crate::tools::ToolRegistry;
use crate::transcript::{PastTurn, live_turns, past_turns, trailing_context};
use crate::web::{ReadUrlTool, WebSearchTool};

/// The project chats with no project are filed under.
pub use tern_store::INBOX;

/// What to open.
#[derive(Clone, Debug, Default)]
pub struct ChatOptions {
    /// Provider id. `None` picks the first remote provider with a saved key.
    pub provider: Option<String>,
    /// Model id. `None` picks the provider's first chat model.
    pub model: Option<String>,
    pub base_url: Option<String>,
    pub budget: Budget,
    pub max_output_tokens: u32,
    /// Search service id. `None` uses the first with a saved key, or none.
    pub search: Option<String>,
    pub search_url: Option<String>,
    /// The notes vault the agent may search, read, and (with approval)
    /// write. `None` leaves the vault tools out.
    pub vault: Option<std::path::PathBuf>,
    /// How answers' claims are checked after each turn.
    pub check_claims: crate::verify::CheckClaims,
    /// The decision model the chat asks (see [`crate::decide`]).
    pub decision: crate::decide::DecisionPrefs,
    /// The project a new chat is filed under (by name; made if missing).
    /// `None` files it in the Inbox.
    pub project: Option<String>,
    /// The folder whose `TERN.md` and `.tern/skills` apply. `None` uses
    /// the project's linked folder, if it has one.
    pub project_root: Option<std::path::PathBuf>,
    /// Leave MCP servers out of this chat.
    pub no_mcp: bool,
    /// Research or Study: which list a new chat is filed in.
    pub harness: Harness,
    /// When the chat folds older turns into a summary (see [`crate::compact`]).
    pub compaction: crate::compact::Compaction,
    /// Tern's own tools turned off (Settings › Tools).
    pub tools_off: Vec<String>,
    /// Per-tool approvals chosen in Settings › Tools.
    pub tool_modes: std::collections::BTreeMap<String, crate::Mode>,
    /// The approval mode (#75).
    pub approval_mode: crate::permissions::ApprovalMode,
    /// Permission rules (#75).
    pub permission_rules: Vec<crate::permissions::PermissionRule>,
}

pub(crate) const WEB_INSTRUCTIONS: &str = "You can search the web and read pages with your tools. Read a page before you rely on it, and cite pages you read inline as [n] with a short list of the links at the end.";

/// One open chat.
#[allow(missing_debug_implementations)]
pub struct ChatSession {
    provider: Provider,
    model: String,
    context_window: Option<u64>,
    session: SessionId,
    sink: StoreSink,
    gate: PolicyGate,
    ledger: UsageLedger,
    tools: ToolRegistry,
    history: Vec<ChatMessage>,
    /// Pictures for the next message (see [`ChatSession::attach_images`]).
    next_images: Vec<tern_providers::ImageInput>,
    /// Whether the chat model sees pictures, as the front end knows from
    /// the provider's listing (`None`: unknown, pictures are sent). A model
    /// the provider refused pictures for (`media::is_not_vision`) never
    /// gets them, whatever this says.
    sees_pictures: Option<bool>,
    /// The chat has its sidebar title (from its first message).
    titled: bool,
    search_name: Option<&'static str>,
    /// The search service, shared with the web tools, for deep research.
    search: Option<Arc<tern_crawl::SearchClient>>,
    budget: Budget,
    max_output_tokens: u32,
    /// The user set the answer length; a model change keeps it.
    output_set: bool,
    /// Research or Study.
    harness: Harness,
    check_claims: crate::verify::CheckClaims,
    /// Where runs save their checkpoints (`<data>/checkpoints`).
    checkpoints: Option<crate::checkpoint::CheckpointDir>,
    /// Steering notes for the running turn, from the front end or `tern steer`.
    steering: crate::steer::Steering,
    /// Ranks sources, hints the first tool, pre-scores claims, and orders work.
    decider: Arc<crate::decide::Decider>,
    /// The project's rules and skills.
    project: crate::rules::ProjectContext,
    /// Some tools return outside text (MCP), so the prompt carries the
    /// untrusted-data rule.
    untrusted: bool,
    /// MCP servers that were left out, and why.
    mcp_notes: Vec<String>,
    /// MCP prompts and resources the active servers offer.
    mcp_offers: Vec<crate::mcp::MenuItem>,
    /// How hard the model thinks, for models that can.
    effort: Option<tern_providers::Effort>,
    /// Receives reasoning as it streams.
    on_reasoning: Option<ReasoningSink>,
    /// What the next turns may do. `None` offers every tool.
    turn_mode: Option<crate::workspace::TurnMode>,
    /// The folder or file this chat works in.
    workspace: Option<Arc<crate::workspace::Workspace>>,
    /// The open workspace is in the log (a draft logs it with its first turn).
    workspace_logged: bool,
    /// When to fold older turns into a summary, the last compaction, and
    /// the model call count when it happened (until the next call the
    /// context is estimated from the shorter history).
    compaction: crate::compact::Compaction,
    compacted: Option<crate::compact::Compacted>,
    /// Why the last compaction folded nothing (for the toast).
    compact_skip: Option<String>,
    calls_at_compaction: Option<usize>,
    /// Full copies of long tool results, which the model sees cut in the
    /// middle and reads in ranges with `read_result` (#72).
    results: Option<Arc<crate::results::ResultStore>>,
    /// Searches, fetches and reads kept for this chat (#77); web results
    /// also on disk, shared by every chat.
    cache: crate::toolcache::ToolCache,
}

/// Where a front end takes a reply's reasoning as it streams: the model
/// call's number in the turn and the new text.
pub type ReasoningSink = Arc<dyn Fn(u32, &str) + Send + Sync>;

/// A file a chat made.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Output {
    pub path: std::path::PathBuf,
    /// What made it: `export`, `save as`, `file_write`.
    pub what: String,
    pub at: time::OffsetDateTime,
}

impl Output {
    /// A file made just now.
    pub fn now(path: std::path::PathBuf, what: &str) -> Self {
        Self {
            path,
            what: what.to_owned(),
            at: time::OffsetDateTime::now_utc(),
        }
    }

    /// How long ago it was made, in seconds.
    pub fn age_secs(&self) -> i64 {
        (time::OffsetDateTime::now_utc() - self.at).whole_seconds()
    }
}

/// The outputs in a chat's events (see [`ChatSession::outputs`]): saves
/// recorded as `session.output`, and `file_write` calls that ran, read
/// against the workspace open when they ran (`workspace` when the log
/// has none before them).
pub fn outputs_in(events: &[EventEnvelope], workspace: Option<&std::path::Path>) -> Vec<Output> {
    let mut out: Vec<Output> = Vec::new();
    let mut ws: Option<std::path::PathBuf> = None;
    for e in events {
        let found = match e.kind.as_str() {
            "session.workspace" => {
                ws = e.payload["path"].as_str().map(std::path::PathBuf::from);
                None
            }
            "session.output" => e.payload["path"].as_str().map(|p| {
                (
                    std::path::PathBuf::from(p),
                    e.payload["what"].as_str().unwrap_or("export").to_owned(),
                )
            }),
            "tool.executed" if e.payload["tool"] == "file_write" => {
                let rel = e.payload["args"]["path"].as_str().unwrap_or_default();
                let base = ws.as_deref().or(workspace);
                match (rel.is_empty(), base) {
                    (false, Some(b)) => Some((join_rel(b, rel), "file_write".to_owned())),
                    _ => None,
                }
            }
            // #77: a rename moves the output to its new name.
            "tool.executed" if e.payload["tool"] == "file_rename" => {
                if let Some(b) = ws.as_deref().or(workspace) {
                    let a = &e.payload["args"];
                    if let (Some(from), Some(to)) = (a["from"].as_str(), a["to"].as_str()) {
                        rename_output(&mut out, &join_rel(b, from), &join_rel(b, to));
                    }
                }
                None
            }
            _ => None,
        };
        if let Some((path, what)) = found {
            let path = crate::projects::plain_path(&path);
            out.retain(|o| !same_file(&o.path, &path));
            out.push(Output {
                path,
                what,
                at: e.ts,
            });
        }
    }
    out
}

/// `rel` under the chat's folder, as the user reads it (no `\\?\`).
fn join_rel(base: &std::path::Path, rel: &str) -> std::path::PathBuf {
    let rel = rel.trim();
    let rel = rel.strip_prefix("./").unwrap_or(rel);
    crate::projects::plain_path(&base.join(rel))
}

/// Whether two spellings name one file (`\\?\` or not, `/` or `\`, and
/// case on Windows).
pub fn same_file(a: &std::path::Path, b: &std::path::Path) -> bool {
    crate::projects::folder_key(a) == crate::projects::folder_key(b)
}

/// After `from` was renamed to `to` (#77): the output listed at `from`
/// now lists at `to`, and a stale entry for `to` goes. Returns whether an
/// output moved.
pub fn rename_output(
    outputs: &mut Vec<Output>,
    from: &std::path::Path,
    to: &std::path::Path,
) -> bool {
    let to = crate::projects::plain_path(to);
    if !outputs.iter().any(|o| same_file(&o.path, from)) {
        return false;
    }
    outputs.retain(|o| same_file(&o.path, from) || !same_file(&o.path, &to));
    for o in outputs.iter_mut().filter(|o| same_file(&o.path, from)) {
        o.path = to.clone();
    }
    true
}

/// Drops the outputs whose file is no longer there (moved, renamed or
/// deleted outside Tern) (#79: they stayed greyed "Moved or deleted"
/// forever and were counted). Returns how many went.
pub fn prune_outputs(outputs: &mut Vec<Output>) -> usize {
    let before = outputs.len();
    outputs.retain(|o| o.path.is_file());
    before - outputs.len()
}

/// What the model is told about the chat's workspace. A folder's name is
/// said with how paths work inside it (#79: in a folder named `New` the
/// model listed `New` and wrote `New/review.md`).
pub fn workspace_line(ws: &crate::workspace::Workspace) -> String {
    let name = ws.name();
    if ws.is_file() {
        return format!(
            "The user opened the file `{name}` as this chat's workspace. Use files_list, file_read and file_search to look at it; paths are relative to its folder."
        );
    }
    format!(
        "The user opened the folder `{name}` as this chat's workspace, and you are already inside it. Use files_list, file_read and file_search to look at it. Every path you give a file tool is relative to this folder: \"\" (or \".\") is `{name}` itself and \"notes.md\" is a file directly in it. Do not start a path with `{name}/`: that would mean a folder named `{name}` inside it. `file_write` makes any folders a new path needs."
    )
}

/// What a folder chat in Chat mode is told (#80): it can read the folder
/// but not change it, and a change is Act mode's job.
pub const CHAT_FOLDER_NOTE: &str = "Chat mode: you can read this folder but not change it. There is no file_write, file_edit, file_rename or run_command in this chat, so never plan or call them (not in run_tasks, not inside run_python), never write a script for the user to run, and never tell the user to create, paste or edit a file by hand. When the user asks for a change to files, reply in one short sentence that it needs Act mode; Tern shows a button that switches to Act and runs the request.";

/// The path of the last `session.workspace` event, unless it closed it.
pub fn last_workspace(events: &[EventEnvelope]) -> Option<String> {
    events
        .iter()
        .rev()
        .find(|e| e.kind == "session.workspace")
        .and_then(|e| e.payload["path"].as_str().map(str::to_owned))
}

/// A sidebar title from the first message: one line, about 60 characters.
pub fn title_from(text: &str) -> String {
    // The title is the user's words, never the `<attachments>` block Tern
    // adds to the message; a message of attachments alone is titled by
    // their names.
    let (body, block) = match text.find("<attachments>") {
        Some(i) => (&text[..i], &text[i..]),
        None => (text, ""),
    };
    let mut line = body.split_whitespace().collect::<Vec<_>>().join(" ");
    if line.is_empty() {
        let names: Vec<&str> = block
            .lines()
            .filter_map(|l| {
                let rest = l.trim_start().strip_prefix("<attachment name=\"")?;
                rest.split_once('"').map(|(n, _)| n)
            })
            .filter(|n| !n.trim().is_empty())
            .collect();
        line = names.join(", ");
    }
    if line.chars().count() <= 60 {
        return line;
    }
    let cut: String = line.chars().take(60).collect();
    let cut = cut
        .rsplit_once(' ')
        .map(|(a, _)| a.to_owned())
        .unwrap_or(cut);
    format!("{cut}…")
}

/// Recent chats for a sidebar, newest first, without opening a provider.
pub fn recent_sessions(limit: usize) -> Result<Vec<SessionSummary>, String> {
    let paths = AppPaths::discover().map_err(|e| e.to_string())?;
    let db = paths.database();
    if !db.exists() {
        return Ok(Vec::new());
    }
    let store = Store::open(&db).map_err(|e| e.to_string())?;
    store.recent_sessions(limit).map_err(|e| e.to_string())
}

/// A past chat's turns for the transcript, read without opening a
/// provider, so a front end can show the chat before its model connects.
pub fn stored_turns(id: SessionId) -> Result<Vec<PastTurn>, String> {
    let paths = AppPaths::discover().map_err(|e| e.to_string())?;
    let store = Store::open(&paths.database()).map_err(|e| e.to_string())?;
    if store.session(id).map_err(|e| e.to_string())?.is_none() {
        return Err("That chat is no longer in the database.".into());
    }
    let events = store.replay(id, 0).map_err(|e| e.to_string())?;
    Ok(live_turns(&events))
}

/// Renames a chat without opening a provider. Blank titles are refused.
pub fn rename_session(id: SessionId, title: &str) -> Result<String, String> {
    let title = clean_title(title).ok_or("A chat needs a title.")?;
    let paths = AppPaths::discover().map_err(|e| e.to_string())?;
    let store = Store::open(&paths.database()).map_err(|e| e.to_string())?;
    store
        .rename_session(id, &title)
        .map_err(|e| e.to_string())?;
    Ok(title)
}

/// A typed title on one line, at most 120 characters, or `None` if blank.
pub fn clean_title(text: &str) -> Option<String> {
    let line = text.split_whitespace().collect::<Vec<_>>().join(" ");
    if line.is_empty() {
        return None;
    }
    Some(line.chars().take(120).collect())
}

/// Where the desktop keeps the chat it reopens on launch, per harness.
fn last_chat_file(paths: &AppPaths) -> std::path::PathBuf {
    paths.config_dir.join("last-chat.json")
}

fn harness_key(h: Harness) -> &'static str {
    match h {
        Harness::Research => "research",
        Harness::Study => "study",
    }
}

/// Remembers `id` as the chat to reopen next time for `harness`.
pub fn remember_chat(harness: Harness, id: SessionId) -> Result<(), String> {
    let paths = AppPaths::discover().map_err(|e| e.to_string())?;
    let path = last_chat_file(&paths);
    let mut v: serde_json::Value = std::fs::read_to_string(&path)
        .ok()
        .and_then(|t| serde_json::from_str(&t).ok())
        .filter(serde_json::Value::is_object)
        .unwrap_or_else(|| json!({}));
    v[harness_key(harness)] = json!(id.to_string());
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir).map_err(|e| e.to_string())?;
    }
    std::fs::write(&path, v.to_string()).map_err(|e| e.to_string())
}

/// The chat to reopen for `harness`: the remembered one while it still
/// has messages and is not archived, else the newest such chat of that
/// harness that is not a sub-session. `None` means start a draft.
pub fn pick_reopen(
    remembered: Option<SessionId>,
    recent: &[SessionSummary],
    harness: Harness,
) -> Option<SessionId> {
    let ok = |r: &&SessionSummary| r.harness == harness && r.events > 0;
    if let Some(id) = remembered
        && recent.iter().filter(ok).any(|r| r.id == id)
    {
        return Some(id);
    }
    recent
        .iter()
        .filter(ok)
        .find(|r| r.parent.is_none())
        .map(|r| r.id)
}

/// [`pick_reopen`] from the saved file and the database.
pub fn chat_to_reopen(harness: Harness) -> Option<SessionId> {
    let paths = AppPaths::discover().ok()?;
    let remembered = std::fs::read_to_string(last_chat_file(&paths))
        .ok()
        .and_then(|t| serde_json::from_str::<serde_json::Value>(&t).ok())
        .and_then(|v| v[harness_key(harness)].as_str().map(str::to_owned))
        .and_then(|s| s.parse().ok());
    let recent = recent_sessions(200).ok()?;
    pick_reopen(remembered, &recent, harness)
}

/// Hides a chat from the list without opening a provider. Its log stays.
pub fn archive_session(id: SessionId) -> Result<(), String> {
    let paths = AppPaths::discover().map_err(|e| e.to_string())?;
    let store = Store::open(&paths.database()).map_err(|e| e.to_string())?;
    store.archive_session(id).map_err(|e| e.to_string())
}

/// How long opening a chat waits for the model listing (only used for the
/// context window and to refuse a non-chat model) before going on.
const LISTING_WAIT: std::time::Duration = std::time::Duration::from_secs(4);

/// The provider and model `opts` names, or the defaults: the first
/// provider with a saved key and its first chat model. Also gives the
/// model's context window when the listing has it.
pub(crate) async fn connect_model(
    opts: &ChatOptions,
    secrets: Option<&dyn SecretStore>,
) -> Result<(Provider, String, Option<u64>), String> {
    let id = match opts.provider.clone() {
            Some(id) => id,
            None => providers::list(secrets)
                .into_iter()
                .find(|r| r.key_saved)
                .map(|r| r.id.to_owned())
                .ok_or_else(|| {
                    "No provider has a key yet. Add one with `tern provider add <id>` (Groq and Google AI Studio have free keys), or pick a local server with `--provider ollama`.".to_owned()
                })?,
        };
    let provider = providers::connect_by_id(&id, secrets, opts.base_url.as_deref())
        .map_err(|e: SetupError| e.to_string())?;
    let (model, context_window) = match opts.model.clone() {
        Some(m) => {
            // Look the window up when the listing has it; a failed
            // listing is not fatal, since the model id may still work.
            // The listing is reused for a while, and a slow or unreachable
            // provider does not hold the chat up: after a few seconds the
            // chat opens without it.
            let listed = tokio::time::timeout(LISTING_WAIT, provider.list_models_cached())
                .await
                .ok()
                .and_then(Result::ok)
                .and_then(|ms| ms.into_iter().find(|x| x.id == m || x.aliases.contains(&m)));
            // A voice, OCR, image, or embedding model cannot be the chat
            // model: say so instead of failing on the first message.
            if let Some(reason) = listed.as_ref().and_then(|x| x.not_chat_reason()) {
                return Err(reason);
            }
            let window = listed
                .and_then(|x| x.context_window)
                .or_else(|| tern_providers::known_context_window(&m));
            (m, window)
        }
        None => {
            let models = provider
                .list_models_cached()
                .await
                .map_err(|e| e.to_string())?;
            let m = models
                .into_iter()
                .find(|m| m.can_chat() && !m.is_retired())
                .ok_or_else(|| {
                    format!(
                        "{} lists no chat models. Pick one with `--model`.",
                        provider.spec().name
                    )
                })?;
            (m.id, m.context_window)
        }
    };
    Ok((provider, model, context_window))
}

/// The chat model these options pick, for one-off calls such as the
/// reasoning-model row of `tern decide bench`.
pub async fn connect_chat_model(
    opts: &ChatOptions,
    secrets: Option<&dyn SecretStore>,
) -> Result<(Provider, String), String> {
    let (p, m, _) = connect_model(opts, secrets).await?;
    Ok((p, m))
}

/// The model's history for a reopened chat: carried-over context, each
/// finished exchange, and sub-sessions' reports back, in order. Turns
/// removed by ID range are not in `turns` (see [`live_turns`]).
pub fn history_of(turns: &[PastTurn], trailing: Option<&str>) -> Vec<ChatMessage> {
    let mut out = Vec::new();
    let context = |out: &mut Vec<ChatMessage>, c: &str| {
        out.push(ChatMessage::user(format!(
            "Context carried over from an earlier session:\n\n{c}"
        )));
        out.push(ChatMessage::assistant(
            "Understood. I will continue from this context.",
        ));
    };
    for t in turns {
        if let Some(c) = &t.context {
            context(&mut out, c);
        }
        if let Some((m, a)) = t.exchange() {
            out.push(ChatMessage::user(m));
            out.push(ChatMessage::assistant(a));
        }
        for r in &t.reports {
            out.push(ChatMessage::user(format!(
                "Sub-session {} reported back on: {}\n\n{}",
                r.refs
                    .first()
                    .map(String::as_str)
                    .unwrap_or("(sub-session)"),
                r.question,
                r.summary
            )));
            out.push(ChatMessage::assistant("Noted."));
        }
    }
    if let Some(c) = trailing {
        context(&mut out, c);
    }
    out
}

/// The effective instructions a chat opened with `opts` would get, without
/// opening a provider or a session: the same tools (web, vault, skills,
/// MCP), rules, and skills, and the same prompt builder. For
/// `tern instructions` and the desktop panel before a chat is open.
pub async fn preview_instructions(opts: &ChatOptions) -> Result<crate::rules::Effective, String> {
    let keychain = providers::keychain();
    let secrets: Option<&dyn SecretStore> = keychain.as_ref().ok().map(|b| b.as_ref());
    let mut tools = ToolRegistry::new();
    let search = search::connect(opts.search.as_deref(), secrets, opts.search_url.as_deref()).ok();
    let web = search.is_some();
    if let Some(c) = search {
        tools.register(WebSearchTool(Arc::new(c), None));
        tools.register(ReadUrlTool::default());
        crate::scholar::register(&mut tools, Arc::new(crate::scholar::Scholar::new(secrets)));
    }
    if let Some(root) = &opts.vault {
        let v = crate::vault::Vault::at(root);
        if v.ensure().is_ok() {
            crate::vault::register(&mut tools, Arc::new(v));
        }
    }
    let root = match &opts.project_root {
        Some(r) => Some(r.clone()),
        None => match opts.project.as_deref() {
            Some(name) => {
                let store = crate::open_store()?;
                crate::projects::find(&store, name)?.root
            }
            None => None,
        },
    };
    let project = crate::rules::ProjectContext::load(root.as_deref());
    project.register(&mut tools);
    crate::workspace::register_local_read(&mut tools);
    let mut gate = PolicyGate::default();
    let mut notes = Vec::new();
    let mut offers = Vec::new();
    if !opts.no_mcp {
        match crate::mcp::McpConfig::load() {
            Ok(cfg) if cfg.servers.values().any(|s| s.active()) => {
                let a = crate::mcp::attach(&cfg, &mut tools, &mut gate).await;
                notes = a.notes;
                offers = crate::mcp::catalog::lines(&a.offers);
            }
            Ok(_) => {}
            Err(e) => notes.push(e),
        }
    }
    let untrusted = tools.names().any(|n| n.starts_with("mcp__"));
    let mut e = crate::rules::effective_with_user(
        web.then_some(WEB_INSTRUCTIONS),
        crate::prompts::user_block(opts.harness).as_deref(),
        &project,
        &tools,
        untrusted,
    );
    e.notes.extend(notes);
    e.offers = offers;
    Ok(e)
}

/// Forwards each stored event to the front end as well.
struct Tee<'a> {
    inner: &'a StoreSink,
    observer: &'a (dyn Fn(&EventEnvelope) + Send + Sync),
}

impl EventSink for Tee<'_> {
    fn record(&self, event: EventEnvelope) -> Result<EventEnvelope, String> {
        let stored = self.inner.record(event)?;
        (self.observer)(&stored);
        Ok(stored)
    }
}

impl ChatSession {
    /// Opens the app database and the provider, picking defaults the user did not give.
    pub async fn open(opts: ChatOptions) -> Result<Self, String> {
        let keychain = providers::keychain();
        let secrets: Option<&dyn SecretStore> = keychain.as_ref().ok().map(|b| b.as_ref());
        let paths = AppPaths::discover().map_err(|e| e.to_string())?;
        paths.ensure().map_err(|e| e.to_string())?;
        let store = Store::open(&paths.database()).map_err(|e| e.to_string())?;
        Self::open_with(opts, secrets, store).await
    }

    /// Reopens a past chat: same session id, its finished exchanges back in
    /// the model's history, and its turns for the transcript. The provider
    /// and model come from `opts` (the current choices), not the old chat.
    pub async fn resume(
        opts: ChatOptions,
        session: SessionId,
    ) -> Result<(Self, Vec<PastTurn>), String> {
        let keychain = providers::keychain();
        let secrets: Option<&dyn SecretStore> = keychain.as_ref().ok().map(|b| b.as_ref());
        let paths = AppPaths::discover().map_err(|e| e.to_string())?;
        let store = Store::open(&paths.database()).map_err(|e| e.to_string())?;
        Self::resume_with(opts, secrets, store, session).await
    }

    /// [`ChatSession::resume`] with the keychain and database given.
    pub async fn resume_with(
        opts: ChatOptions,
        secrets: Option<&dyn SecretStore>,
        store: Store,
        session: SessionId,
    ) -> Result<(Self, Vec<PastTurn>), String> {
        if store.session(session).map_err(|e| e.to_string())?.is_none() {
            return Err("That chat is no longer in the database.".into());
        }
        let events = store.replay(session, 0).map_err(|e| e.to_string())?;
        let turns = live_turns(&events);
        let mut s = Self::open_inner(opts, secrets, store, Some(session)).await?;
        // A compacted chat reopens with its summary pinned in place of the
        // turns it folded (the log keeps them all).
        s.compacted = crate::compact::last(&events);
        s.history = crate::compact::history(
            &turns,
            s.compacted.as_ref(),
            trailing_context(&events).as_deref(),
        );
        // The workspace the chat last had open, if it still exists.
        if let Some(path) = last_workspace(&events)
            && let Ok(ws) = crate::workspace::Workspace::open(std::path::Path::new(&path))
        {
            let ws = Arc::new(ws);
            ws.register(&mut s.tools);
            s.gate.set_folder(Some(ws.root()));
            s.workspace = Some(ws);
            s.workspace_logged = true;
        }
        Ok((s, turns))
    }

    /// Reads the chat's database (for sub-sessions, ranges, the Library).
    pub fn with_store<T>(&self, f: impl FnOnce(&Store) -> T) -> Result<T, String> {
        self.sink.with_store(f)
    }

    /// Writes to the chat's database.
    pub fn with_store_mut<T>(&self, f: impl FnOnce(&mut Store) -> T) -> Result<T, String> {
        self.sink.with_store_mut(f)
    }

    /// Hides a chat from the list. Its log stays.
    pub fn archive(&self, session: SessionId) -> Result<(), String> {
        self.sink
            .with_store(|s| s.archive_session(session))?
            .map_err(|e| e.to_string())
    }

    /// [`ChatSession::open`] with the keychain and database given, for tests.
    pub async fn open_with(
        opts: ChatOptions,
        secrets: Option<&dyn SecretStore>,
        store: Store,
    ) -> Result<Self, String> {
        Self::open_inner(opts, secrets, store, None).await
    }

    async fn open_inner(
        opts: ChatOptions,
        secrets: Option<&dyn SecretStore>,
        store: Store,
        resume: Option<SessionId>,
    ) -> Result<Self, String> {
        let (provider, model, context_window) = connect_model(&opts, secrets).await?;
        let mut tools = ToolRegistry::new();
        tools.turn_off(opts.tools_off.iter().map(String::as_str));
        let search = match opts.search.as_deref() {
            Some("off" | "none") => None,
            id => match search::connect(id, secrets, opts.search_url.as_deref()) {
                Ok(c) => Some(c),
                Err(e) if opts.search.is_some() => return Err(e),
                Err(_) => None,
            },
        };
        let search_name = search.as_ref().map(|c| c.spec().name);
        let search = search.map(Arc::new);
        let decider = Arc::new(crate::decide::decider_from(&opts.decision, secrets));
        if let Some(c) = &search {
            tools.register(WebSearchTool(c.clone(), Some(decider.clone())));
            tools.register(ReadUrlTool::default());
            // Scholarly search (#82) goes with web search: same switch.
            crate::scholar::register(&mut tools, Arc::new(crate::scholar::Scholar::new(secrets)));
        }
        // A vault that cannot be opened only leaves its tools out.
        if let Some(root) = &opts.vault {
            let v = crate::vault::Vault::at(root);
            if v.ensure().is_ok() {
                crate::vault::register(&mut tools, Arc::new(v));
            }
        }
        // A new chat is a draft: its row is written with its first event
        // that is not just the opening, so a chat opened and left empty
        // leaves nothing behind.
        let (session, draft) = match resume {
            Some(id) => (id, None),
            None => {
                let name = opts
                    .project
                    .as_deref()
                    .map(str::trim)
                    .filter(|n| !n.is_empty())
                    .unwrap_or(INBOX);
                let project = match store.project_named(name).map_err(|e| e.to_string())? {
                    Some(p) => p,
                    None => store.ensure_project(name).map_err(|e| e.to_string())?,
                };
                (SessionId::new(), Some(project))
            }
        };
        // The project's folder gives the chat its TERN.md rules and skills.
        let root = opts.project_root.clone().or_else(|| {
            let project = match draft {
                Some(p) => Some(p),
                None => store.session_project(session).ok().flatten(),
            };
            project
                .and_then(|p| store.project(p).ok().flatten())
                .and_then(|p| p.root)
        });
        let project = crate::rules::ProjectContext::load(root.as_deref());
        project.register(&mut tools);
        // A file the user names by its full path can be read with no
        // workspace open (each one approved).
        crate::workspace::register_local_read(&mut tools);
        crate::workbench::register_always(&mut tools);
        let mut gate = PolicyGate::default();
        for (name, mode) in &opts.tool_modes {
            gate.set_tool_mode(name, *mode);
        }
        gate.set_approval(opts.approval_mode, opts.permission_rules.clone());
        let mut mcp_notes = Vec::new();
        let mut mcp_servers = Vec::new();
        let mut mcp_offers = Vec::new();
        if !opts.no_mcp {
            match crate::mcp::McpConfig::load() {
                Ok(cfg) if cfg.servers.values().any(|s| s.active()) => {
                    let a = crate::mcp::attach(&cfg, &mut tools, &mut gate).await;
                    mcp_notes = a.notes;
                    mcp_servers = a.servers;
                    mcp_offers = a.offers;
                }
                Ok(_) => {}
                Err(e) => mcp_notes.push(e),
            }
        }
        let paths = AppPaths::discover().ok();
        // Long tool results: the whole text is kept for the chat, the model
        // reads past the cut with `read_result` (#72).
        let results = paths.as_ref().map(|p| {
            Arc::new(crate::results::ResultStore::for_session(
                &p.data_dir,
                session,
            ))
        });
        if let Some(r) = &results {
            tools.register(crate::results::ReadResultTool(r.clone()));
        }
        let untrusted = tools.names().any(|n| n.starts_with("mcp__"));
        let checkpoints = paths
            .as_ref()
            .map(|p| crate::checkpoint::CheckpointDir::new(p.checkpoints()));
        let steering = match &paths {
            Some(p) => {
                crate::steer::Steering::with_file(crate::steer::inbox_path(p, &session.to_string()))
            }
            None => crate::steer::Steering::new(),
        };
        let provider_id = provider.spec().id;
        let model_id = model.clone();
        let harness = match resume {
            Some(id) => store
                .session(id)
                .ok()
                .flatten()
                .map(|r| r.harness)
                .unwrap_or(opts.harness),
            None => opts.harness,
        };
        let s = Self {
            decider,
            checkpoints,
            steering,
            provider,
            model,
            context_window,
            session,
            sink: StoreSink::new(store),
            gate,
            project,
            untrusted,
            mcp_notes,
            mcp_offers,
            effort: None,
            on_reasoning: None,
            turn_mode: None,
            workspace: None,
            workspace_logged: false,
            compaction: opts.compaction,
            compacted: None,
            compact_skip: None,
            calls_at_compaction: None,
            cache: match &paths {
                Some(p) => crate::toolcache::ToolCache::with_dir(
                    crate::toolcache::ToolCache::dir_in(&p.data_dir),
                ),
                None => crate::toolcache::ToolCache::memory(),
            },
            results,
            ledger: UsageLedger::default(),
            tools,
            history: Vec::new(),
            next_images: Vec::new(),
            sees_pictures: None,
            titled: resume.is_some(),
            search_name,
            search,
            budget: opts.budget,
            check_claims: opts.check_claims,
            max_output_tokens: if opts.max_output_tokens == 0 {
                tern_providers::default_max_output(provider_id, &model_id)
            } else {
                opts.max_output_tokens
            },
            output_set: opts.max_output_tokens != 0,
            harness,
        };
        let opened = EventEnvelope::new(
            session,
            if resume.is_some() {
                "session.resumed"
            } else {
                "session.opened"
            },
            EffectClass::Analysis,
            format!(
                "{} a chat with {} on {}",
                if resume.is_some() {
                    "Reopened"
                } else {
                    "Opened"
                },
                s.model,
                s.provider.spec().name
            ),
            json!({
                "provider": s.provider.spec().id,
                "model": s.model,
                "project_root": s.project.root,
                "rules": s.project.rules.iter().map(|r| r.path.display().to_string()).collect::<Vec<_>>(),
                "skills": s.project.skills.enabled().map(|k| k.name.clone()).collect::<Vec<_>>(),
                "mcp": mcp_servers,
                "mcp_notes": s.mcp_notes,
            }),
        );
        match draft {
            Some(project) => s
                .sink
                .defer_session(session, project, opts.harness, "Chat", opened),
            None => {
                s.sink.record(opened)?;
            }
        }
        Ok(s)
    }

    pub fn provider_name(&self) -> &'static str {
        self.provider.spec().name
    }

    /// The provider's catalog id (`openai`).
    pub fn provider_id(&self) -> &'static str {
        self.provider.spec().id
    }

    pub fn model(&self) -> &str {
        &self.model
    }

    /// Research or Study.
    pub fn harness(&self) -> Harness {
        self.harness
    }

    /// Whether the chat is still a draft: nothing has been sent, so it has
    /// no row in the chat list.
    pub fn is_draft(&self) -> bool {
        self.sink.is_draft(self.session)
    }

    /// Sets how hard the model thinks on the next turns. Models without a
    /// thinking knob ignore it.
    pub fn set_effort(&mut self, effort: Option<tern_providers::Effort>) {
        self.effort = effort;
    }

    pub fn effort(&self) -> Option<tern_providers::Effort> {
        self.effort
    }

    /// What the current model offers for thinking.
    pub fn thinking(&self) -> tern_providers::Thinking {
        tern_providers::thinking_support(self.provider.spec().id, &self.model)
    }

    /// Sends the model's reasoning to `sink` as it streams, on every turn
    /// until changed.
    pub fn set_reasoning_sink(&mut self, sink: Option<ReasoningSink>) {
        self.on_reasoning = sink;
    }

    /// Chat, Plan, or Act: which tools the next turns may use, and the
    /// line the model is told.
    pub fn set_turn_mode(&mut self, mode: crate::workspace::TurnMode) {
        self.turn_mode = Some(mode);
        self.tools.set_mode(mode);
    }

    /// Sets the approval mode and permission rules for the next calls (#75).
    pub fn set_approval(
        &mut self,
        mode: crate::permissions::ApprovalMode,
        rules: Vec<crate::permissions::PermissionRule>,
    ) {
        self.gate.set_approval(mode, rules);
    }

    /// The approval mode in force.
    pub fn approval(&self) -> crate::permissions::ApprovalMode {
        self.gate.config().approval
    }

    pub fn turn_mode(&self) -> Option<crate::workspace::TurnMode> {
        self.turn_mode
    }

    /// Opens a folder or file as this chat's workspace: the file tools are
    /// scoped to it, and the choice is logged so a reopened chat keeps it.
    /// `None` closes it.
    pub fn set_workspace(&mut self, path: Option<&std::path::Path>) -> Result<(), String> {
        let ws = path
            .map(crate::workspace::Workspace::open)
            .transpose()?
            .map(Arc::new);
        crate::workspace::Workspace::unregister(&mut self.tools);
        self.workspace = None;
        self.gate.set_folder(ws.as_ref().map(|w| w.root()));
        match ws {
            Some(ws) => {
                ws.register(&mut self.tools);
                self.workspace = Some(ws);
            }
            None => crate::workspace::register_local_read(&mut self.tools),
        }
        self.workspace_logged = false;
        if !self.is_draft() {
            self.log_workspace();
            self.workspace_logged = true;
        }
        Ok(())
    }

    fn log_workspace(&self) {
        let path = self
            .workspace
            .as_ref()
            .map(|w| w.path().display().to_string());
        let _ = self.sink.record(EventEnvelope::new(
            self.session,
            "session.workspace",
            EffectClass::Analysis,
            match &path {
                Some(p) => format!("Workspace: {p}"),
                None => "Workspace closed".into(),
            },
            serde_json::json!({"path": path}),
        ));
    }

    /// Files a chat that has no messages yet under `project`, whose folder
    /// (`root`) gives it its TERN.md rules and skills from now on. Refused
    /// once the chat has started: its folder is fixed then.
    pub fn set_draft_project(
        &mut self,
        project: tern_core::ProjectId,
        root: Option<&std::path::Path>,
    ) -> Result<(), String> {
        if !self.is_draft() || !self.sink.set_draft_project(project) {
            return Err(
                "This chat has started; its folder is fixed. Start a new chat to use another folder."
                    .into(),
            );
        }
        self.project = crate::rules::ProjectContext::load(root);
        self.project.register(&mut self.tools);
        Ok(())
    }

    /// Records a file this chat saved (a block exported, a Save as…), so
    /// its Outputs list shows it when the chat is opened again.
    pub fn record_output(&mut self, path: &std::path::Path, what: &str) -> Result<(), String> {
        let size = std::fs::metadata(path).map(|m| m.len()).ok();
        self.sink.record(EventEnvelope::new(
            self.session,
            "session.output",
            EffectClass::Analysis,
            format!("Saved {}", path.display()),
            serde_json::json!({"path": path.display().to_string(), "what": what, "bytes": size}),
        ))?;
        Ok(())
    }

    /// The files this chat made, oldest first: what it saved
    /// ([`Self::record_output`]) and what `file_write` wrote in its
    /// workspace. One path appears once, at its latest write.
    pub fn outputs(&self) -> Vec<Output> {
        let events = self
            .sink
            .with_store(|s| s.replay(self.session, 0).unwrap_or_default())
            .unwrap_or_default();
        outputs_in(&events, self.workspace().map(|w| w.path()))
    }

    /// The chat's workspace, if one is open.
    pub fn workspace(&self) -> Option<&crate::workspace::Workspace> {
        self.workspace.as_deref()
    }

    /// The most output tokens an answer may use.
    pub fn max_output_tokens(&self) -> u32 {
        self.max_output_tokens
    }

    /// Switches this chat to another provider, model, or search service in
    /// place: the session id, history, and context stay, and the next
    /// answer comes from the new model. Nothing changes if it fails.
    pub async fn set_model(&mut self, opts: &ChatOptions) -> Result<(), String> {
        let keychain = providers::keychain();
        let secrets: Option<&dyn SecretStore> = keychain.as_ref().ok().map(|b| b.as_ref());
        self.set_model_with(opts, secrets).await
    }

    /// [`ChatSession::set_model`] with the keychain given, for tests.
    pub async fn set_model_with(
        &mut self,
        opts: &ChatOptions,
        secrets: Option<&dyn SecretStore>,
    ) -> Result<(), String> {
        let (provider, model, context_window) = connect_model(opts, secrets).await?;
        let search = match opts.search.as_deref() {
            Some("off" | "none") => None,
            id => match search::connect(id, secrets, opts.search_url.as_deref()) {
                Ok(c) => Some(Arc::new(c)),
                Err(e) if opts.search.is_some() => return Err(e),
                Err(_) => None,
            },
        };
        let from = format!("{} · {}", self.provider.spec().name, self.model);
        self.provider = provider;
        self.model = model;
        self.context_window = context_window;
        // What the last model could see says nothing about this one; the
        // front end tells the session again before the next message, and a
        // model known not to see (`media::is_not_vision`) never gets one.
        self.sees_pictures = None;
        if !self.output_set {
            self.max_output_tokens =
                tern_providers::default_max_output(self.provider.spec().id, &self.model);
        }
        self.search_name = search.as_ref().map(|c| c.spec().name);
        match &search {
            Some(c) => {
                self.tools
                    .register(WebSearchTool(c.clone(), Some(self.decider.clone())));
                self.tools.register(ReadUrlTool::default());
                crate::scholar::register(
                    &mut self.tools,
                    Arc::new(crate::scholar::Scholar::new(secrets)),
                );
            }
            None => {
                self.tools.unregister("web_search");
                self.tools.unregister("read_url");
                crate::scholar::unregister(&mut self.tools);
            }
        }
        self.search = search;
        if !self.is_draft() {
            self.sink.record(EventEnvelope::new(
                self.session,
                "session.model_changed",
                EffectClass::Analysis,
                format!(
                    "Switched from {from} to {} · {}",
                    self.provider.spec().name,
                    self.model
                ),
                json!({"provider": self.provider.spec().id, "model": self.model,
                       "search": self.search_name}),
            ))?;
        }
        Ok(())
    }

    /// The search service the chat's web tools use, if any.
    pub fn search_name(&self) -> Option<&'static str> {
        self.search_name
    }

    pub fn context_window(&self) -> Option<u64> {
        self.context_window
    }

    pub fn session_id(&self) -> SessionId {
        self.session
    }

    /// How full the model's context window is: the latest call's prompt and
    /// reply as the provider counted them, or, before any call in this
    /// window (a fresh or reopened chat), an estimate of the history the
    /// next call will carry.
    pub fn context(&self) -> ContextUse {
        let session = self.ledger.total();
        let base = ContextUse {
            window: self.context_window,
            session_tokens: session.total(),
            calls: self.ledger.calls(),
            ..ContextUse::default()
        };
        let instructions = self.instructions(None);
        let system = crate::protocol::system_prompt(
            instructions.as_deref(),
            &crate::tools::ToolRegistry::new(),
        )
        .len();
        let with_tools = crate::protocol::system_prompt(instructions.as_deref(), &self.tools).len();
        let (mut attached, mut talk, mut summary) = (0usize, 0usize, 0usize);
        for m in &self.history {
            if crate::compact::is_pinned(&m.content) {
                summary += m.content.len();
                continue;
            }
            let a = attached_chars(&m.content);
            attached += a;
            talk += m.content.len().saturating_sub(a);
        }
        let parts =
            ContextParts::estimate(system, with_tools.saturating_sub(system), attached, talk)
                .with_summary(summary);
        // Right after a compaction the last call's count is stale: estimate
        // the shorter history until the next call counts it.
        let last = self
            .ledger
            .last()
            .filter(|_| self.calls_at_compaction != Some(self.ledger.calls()));
        match last {
            Some(u) => ContextUse {
                prompt: u.input_tokens,
                reply: u.output_tokens,
                estimated: u.measure == Measure::Estimated,
                parts: parts.scaled_to(u.input_tokens, u.output_tokens),
                ..base
            },
            None => ContextUse {
                prompt: parts.total(),
                estimated: true,
                parts,
                ..base
            },
        }
    }

    /// Tokens used by every turn so far.
    pub fn usage(&self) -> Usage {
        self.ledger.total()
    }

    /// Tools for later turns (web search arrives with `tern ask`).
    /// Recent chats from this session's database, newest first.
    pub fn recent_sessions(&self, limit: usize) -> Result<Vec<SessionSummary>, String> {
        self.sink
            .with_store(|s| s.recent_sessions(limit))?
            .map_err(|e| e.to_string())
    }

    pub fn tools_mut(&mut self) -> &mut ToolRegistry {
        &mut self.tools
    }

    /// The harness's instructions with the project's rules, skills, and
    /// the untrusted-data rule, as the turn loop gets them.
    fn instructions(&self, harness: Option<&str>) -> Option<String> {
        let mut extra: Vec<String> = harness.map(str::to_owned).into_iter().collect();
        if let Some(line) = self.turn_mode.and_then(|m| m.instructions()) {
            extra.push(line.to_owned());
        }
        if let Some(ws) = &self.workspace {
            extra.push(workspace_line(ws));
            // A folder chat in Chat mode reads but never changes (#80: the
            // model planned `file_write`, then told the user to create the
            // file by hand).
            if self.turn_mode == Some(crate::workspace::TurnMode::Chat)
                && self.tools.hidden("file_write")
            {
                extra.push(CHAT_FOLDER_NOTE.to_owned());
            }
        }
        if let Some(u) = crate::prompts::user_block(self.harness) {
            extra.push(u);
        }
        let joined = (!extra.is_empty()).then(|| extra.join("\n\n"));
        crate::rules::instructions(joined.as_deref(), &self.project, self.untrusted)
    }

    /// Exactly what the model is told on this chat's next turn, piece by
    /// piece (see [`crate::rules::effective`]).
    pub fn effective_instructions(&self) -> crate::rules::Effective {
        let harness = self.search_name.map(|_| WEB_INSTRUCTIONS);
        let user = crate::prompts::user_block(self.harness);
        let mut e = crate::rules::effective_with_user(
            harness,
            user.as_deref(),
            &self.project,
            &self.tools,
            self.untrusted,
        );
        e.notes.extend(self.mcp_notes.iter().cloned());
        e.offers = crate::mcp::catalog::lines(&self.mcp_offers);
        if self.decider.is_on() {
            e.notes.push(
                "The decision model may add a one-line hint about the first tool to each turn's instructions.".into(),
            );
        }
        e
    }

    /// The project folder this chat reads rules and skills from.
    pub fn project_root(&self) -> Option<&std::path::Path> {
        self.project.root.as_deref()
    }

    /// MCP prompts and resources the chat's servers offer (for `/` and
    /// `@` menus).
    pub fn mcp_offers(&self) -> &[crate::mcp::MenuItem] {
        &self.mcp_offers
    }

    /// MCP servers left out of this chat, and why.
    pub fn mcp_notes(&self) -> &[String] {
        &self.mcp_notes
    }

    /// Runs one turn. Completed exchanges join the history for the next turn.
    pub async fn send(
        &mut self,
        text: &str,
        approver: &dyn Approver,
        cancel: CancelFlag,
        observer: &(dyn Fn(&EventEnvelope) + Send + Sync),
    ) -> TurnOutcome {
        self.send_streaming(text, approver, cancel, observer, None)
            .await
    }

    /// [`ChatSession::send`] that streams reply text to `on_text` as it
    /// arrives (see [`crate::OnText`]).
    /// Pictures to send with the next message, for models that can see.
    pub fn attach_images(&mut self, images: Vec<tern_providers::ImageInput>) {
        self.next_images = images;
    }

    /// Whether pictures wait to go with the next message.
    pub fn has_attached_images(&self) -> bool {
        !self.next_images.is_empty()
    }

    /// Drops the pictures from every earlier message (and any waiting to
    /// go), for a chat model that cannot see: a provider refuses the whole
    /// request when any message carries one. Each dropped picture leaves a
    /// short note in its message. Returns how many were dropped.
    pub fn drop_pictures(&mut self) -> usize {
        let n = self.next_images.len();
        self.next_images.clear();
        n + strip_pictures(&mut self.history)
    }

    /// Tells the chat whether its model sees pictures (from the provider's
    /// listing). When it does not, earlier pictures stay out of every
    /// request and pictures attached anyway go as notes.
    pub fn set_sees_pictures(&mut self, sees: Option<bool>) {
        self.sees_pictures = sees;
    }

    /// The chat model cannot take pictures: the front end says so, or the
    /// provider refused them before.
    pub fn is_blind(&self) -> bool {
        self.sees_pictures == Some(false)
            || crate::media::is_not_vision(self.provider_id(), &self.model)
    }

    pub async fn send_streaming(
        &mut self,
        text: &str,
        approver: &dyn Approver,
        cancel: CancelFlag,
        observer: &(dyn Fn(&EventEnvelope) + Send + Sync),
        on_text: Option<crate::OnText<'_>>,
    ) -> TurnOutcome {
        let mut images = std::mem::take(&mut self.next_images);
        // A model that cannot see never gets a picture: earlier ones leave
        // the history, and any attached anyway go as notes.
        let with_notes;
        let text = if self.is_blind() {
            strip_pictures(&mut self.history);
            let notes: Vec<String> = images
                .drain(..)
                .map(|i| {
                    format!(
                        "[image: {} (not sent: this model cannot see pictures)]",
                        i.name
                    )
                })
                .collect();
            if notes.is_empty() {
                text
            } else {
                with_notes = format!("{text}\n\n{}", notes.join("\n"));
                with_notes.as_str()
            }
        } else {
            text
        };
        // An attached MCP resource is untrusted data: from now on the
        // prompt carries the rule for it, as it does for MCP tools.
        if text.contains(&format!("<{} source=", crate::untrusted::TAG)) {
            self.untrusted = true;
        }
        // `@Q12` and `@Q12.3` quote those messages for the model.
        let with_refs;
        let text = if text.contains("@Q") || text.contains("@q") {
            let extra = self
                .sink
                .with_store(|s| crate::refs::mention_attachments(s, self.session, text))
                .unwrap_or_default();
            with_refs = crate::attach::append(text, &extra);
            with_refs.as_str()
        } else {
            text
        };
        let tee = Tee {
            inner: &self.sink,
            observer,
        };
        self.steering.clear();
        // The decision model's hint for the first step, logged just before
        // the turn starts (the run picks it up with the turn).
        let mut harness = self.search_name.map(|_| WEB_INSTRUCTIONS.to_owned());
        let names: Vec<&str> = self.tools.names().collect();
        let (_, question) = crate::reply::split(text);
        let attachments = images.len() + question.matches("<attachment ").count();
        let question: String = question
            .split("<attachments>")
            .next()
            .unwrap_or(question)
            .chars()
            .take(2000)
            .collect();
        if let Some((hint, d)) =
            crate::decide::next_tool_hint(&self.decider, &question, &names, attachments).await
        {
            let _ = tee.record(crate::decide::event(self.session, None, &d));
            if let Some(h) = hint {
                harness = Some(match harness {
                    Some(i) => format!("{i}\n\n{h}"),
                    None => h,
                });
            }
        }
        let instructions = self.instructions(harness.as_deref());
        let mut retried = false;
        let out = loop {
            let hooks = crate::engine::RunHooks {
                checkpoints: self
                    .checkpoints
                    .as_ref()
                    .map(|c| c as &dyn crate::checkpoint::CheckpointSink),
                steering: Some(&self.steering),
                resume: None,
                effort: self.effort,
                on_reasoning: self.on_reasoning.as_deref(),
                native_tools: crate::engine::native_tools_ok(self.provider.spec().id, &self.model),
                prefetch: true,
                retry: None,
                results: self.results.as_deref(),
                cache: Some(&self.cache),
            };
            let out = TurnLoop {
                session: self.session,
                model: &self.provider,
                model_name: &self.model,
                tools: &self.tools,
                gate: &mut self.gate,
                approver,
                sink: &tee,
                ledger: &self.ledger,
                budget: self.budget,
                cancel: cancel.clone(),
                instructions: instructions.as_deref(),
                max_output_tokens: self.max_output_tokens,
                on_text,
                hooks,
            }
            .run_message(
                self.history.clone(),
                ChatMessage::user(text).with_images(images.clone()),
            )
            .await;
            // The provider refused pictures that came from earlier messages
            // (this message has none): the model cannot see. Remember it,
            // drop them, and run the turn once more without them.
            if !retried
                && images.is_empty()
                && !cancel.is_cancelled()
                && out.answer.is_none()
                && out.state == tern_core::TurnState::Failed
                && out
                    .reason
                    .as_deref()
                    .is_some_and(crate::media::rejects_images)
                && self.history.iter().any(|m| !m.images.is_empty())
            {
                crate::media::mark_not_vision(self.provider.spec().id, &self.model);
                strip_pictures(&mut self.history);
                retried = true;
                continue;
            }
            break out;
        };
        if self.workspace.is_some() && !self.workspace_logged && !self.is_draft() {
            self.log_workspace();
            self.workspace_logged = true;
        }
        if !self.titled {
            self.titled = true;
            let title = title_from(text);
            let _ = self
                .sink
                .with_store(|s| s.rename_session(self.session, &title));
        }
        // An empty answer never joins the history: providers (Mistral,
        // Anthropic, Gemini) refuse every later request that carries an
        // empty assistant message, so one empty reply used to fail each
        // turn after it at the first model call.
        if let Some(answer) = out.answer.as_ref().filter(|a| !a.trim().is_empty()) {
            // Pictures stay in the history so follow-up questions can
            // still see them.
            self.history
                .push(ChatMessage::user(text).with_images(images));
            self.history.push(ChatMessage::assistant(answer.clone()));
            self.auto_compact(observer).await;
        }
        out
    }
}

impl ChatSession {
    /// Runs one deep research turn: the model drafts a plan, `approver`
    /// shows it to the user, then the plan's workers search and read in
    /// parallel and one cited answer is written from their notes (see
    /// [`crate::deep`]). Events reach `observer` as they are logged;
    /// `on_text` gets the answer as it streams. Call
    /// [`ChatSession::check_last_turn`] afterwards for the claim checks and
    /// run report, as after [`ChatSession::send`].
    pub async fn deep_research(
        &mut self,
        text: &str,
        opts: &crate::deep::DeepOptions,
        approver: &dyn crate::deep::PlanApprover,
        control: &crate::deep::DeepControl,
        observer: &(dyn Fn(&EventEnvelope) + Send + Sync),
        on_text: Option<crate::OnText<'_>>,
    ) -> TurnOutcome {
        // Deep research reads the web; pictures are not sent.
        self.next_images.clear();
        let Some(search) = self.search.clone() else {
            return TurnOutcome {
                turn_id: tern_core::TurnId::new(),
                state: tern_core::TurnState::Failed,
                answer: None,
                reason: Some(
                    "Deep research needs web search. Add a key with `tern search add tavily` (or `brave`), or use your own SearXNG."
                        .into(),
                ),
                states: vec![tern_core::TurnState::Failed],
                usage: Usage::measured(0, 0),
                model_calls: 0,
                tool_calls: 0,
            };
        };
        let reader = crate::web::shared_reader();
        let tee = Tee {
            inner: &self.sink,
            observer,
        };
        let deps = crate::deep::DeepDeps {
            session: self.session,
            model: &self.provider,
            model_name: &self.model,
            search: search.as_ref(),
            reader: &reader,
            sink: &tee,
            ledger: &self.ledger,
            gate: Some(&mut self.gate),
            on_text,
            checkpoints: self
                .checkpoints
                .as_ref()
                .map(|c| c as &dyn crate::checkpoint::CheckpointSink),
            steering: Some(&self.steering),
            decider: Some(&self.decider),
        };
        self.steering.clear();
        let mut opts = *opts;
        opts.max_output_tokens = opts.max_output_tokens.max(self.max_output_tokens);
        let out = crate::deep::run_deep(deps, text, &opts, approver, control).await;
        if !self.titled {
            self.titled = true;
            let title = title_from(&crate::deep::question_of(text));
            let _ = self
                .sink
                .with_store(|s| s.rename_session(self.session, &title));
        }
        if let Some(answer) = &out.answer {
            self.history.push(ChatMessage::user(text));
            self.history.push(ChatMessage::assistant(answer.clone()));
            self.auto_compact(observer).await;
        }
        out
    }

    /// When the chat compacts on its own.
    pub fn compaction(&self) -> crate::compact::Compaction {
        self.compaction
    }

    pub fn set_compaction(&mut self, c: crate::compact::Compaction) {
        self.compaction = c;
    }

    /// Why the last compaction found nothing to fold, in plain words.
    pub fn compact_skip(&self) -> Option<&str> {
        self.compact_skip.as_deref()
    }

    /// The chat's last compaction, if it has one.
    pub fn compacted(&self) -> Option<&crate::compact::Compacted> {
        self.compacted.as_ref()
    }

    /// Compacts when the context has passed the threshold. A failed
    /// summary leaves the history as it was; the next turn tries again.
    async fn auto_compact(&mut self, observer: &(dyn Fn(&EventEnvelope) + Send + Sync)) {
        if self.compaction.due(self.context().fraction()) {
            let _ = self.compact(true, observer).await;
        }
    }

    /// Folds the older turns into one pinned summary written by the chat's
    /// model, keeping the last turns and those with attachments word for
    /// word. Logs `session.compacted` (the turns themselves stay in the
    /// log). `Ok(None)` when there is nothing to fold yet.
    pub async fn compact(
        &mut self,
        auto: bool,
        observer: &(dyn Fn(&EventEnvelope) + Send + Sync),
    ) -> Result<Option<crate::compact::Compacted>, String> {
        if self.is_draft() {
            return Ok(None);
        }
        let events = self
            .sink
            .with_store(|s| s.replay(self.session, 0))?
            .map_err(|e| e.to_string())?;
        let turns = live_turns(&events);
        let prior = crate::compact::last(&events);
        let ctx = self.context();
        let pressure = auto
            || ctx
                .fraction()
                .is_some_and(|f| f >= crate::compact::PRESSURE);
        let plan = crate::compact::plan_for(
            &turns,
            prior.as_ref(),
            self.compaction.keep_turns,
            self.context_window,
            pressure,
        );
        let keep = plan.kept;
        let fold = plan.fold;
        if fold.is_empty() {
            let overhead = ctx
                .window
                .filter(|w| *w > 0)
                .map(|w| (ctx.parts.system + ctx.parts.tools) as f32 / w as f32);
            self.compact_skip = plan.skip.map(|s| s.message(overhead));
            return Ok(None);
        }
        self.compact_skip = None;
        let folding: Vec<&PastTurn> = turns.iter().filter(|t| fold.contains(&t.number)).collect();
        let req = crate::compact::request(&self.model, prior.as_ref(), &folding);
        let reply = self.provider.chat(&req).await.map_err(|e| e.to_string())?;
        let summary = reply.text.trim().to_owned();
        if summary.is_empty() {
            return Err("The model wrote an empty summary.".into());
        }
        let done = crate::compact::Compacted {
            summary,
            turns: crate::compact::merged(prior.as_ref(), &fold),
            kept: keep,
            auto,
        };
        let tee = Tee {
            inner: &self.sink,
            observer,
        };
        tee.record(done.event(self.session))?;
        self.history =
            crate::compact::history(&turns, Some(&done), trailing_context(&events).as_deref());
        self.calls_at_compaction = Some(self.ledger.calls());
        self.compacted = Some(done.clone());
        Ok(Some(done))
    }

    /// The inbox for steering notes; send to a clone of it while a turn
    /// runs (the run takes notes in at its next safe point).
    pub fn steering(&self) -> crate::steer::Steering {
        self.steering.clone()
    }

    /// This chat's unfinished run, if it has one to resume.
    pub fn checkpoint(&self) -> Option<crate::checkpoint::RunCheckpoint> {
        self.checkpoints
            .as_ref()?
            .load(&self.session.to_string())
            .ok()
            .flatten()
    }

    /// The turn limits this chat runs with.
    pub fn budget(&self) -> Budget {
        self.budget
    }

    /// Raises the chat's turn limits by `factor` (at least 1), and the
    /// unfinished run's checkpoint with them (a deep run's plan budget and
    /// each worker's slice), so a run stopped at its limit resumes with
    /// more room. Returns the run's new token limit.
    pub fn raise_budget(&mut self, factor: f64) -> Result<u64, String> {
        use crate::checkpoint::CheckpointSink;
        let f = if factor.is_finite() {
            factor.max(1.0)
        } else {
            1.0
        };
        let up = |n: u64| (n as f64 * f).round() as u64;
        let up32 = |n: u32| (f64::from(n) * f).ceil() as u32;
        self.budget.max_tokens = up(self.budget.max_tokens);
        self.budget.max_model_calls = up32(self.budget.max_model_calls);
        self.budget.max_tool_calls = up32(self.budget.max_tool_calls);
        let mut limit = self.budget.max_tokens;
        if let (Some(mut ck), Some(dir)) = (self.checkpoint(), self.checkpoints.as_ref()) {
            if let Some(deep) = ck.deep.as_mut() {
                let b = &mut deep.plan.budget;
                b.tokens = up(b.tokens);
                b.pages = up32(b.pages);
                b.seconds = up(b.seconds);
                limit = b.tokens;
                for w in &mut deep.workers {
                    w.budget.tokens = up(w.budget.tokens);
                    w.budget.pages = up32(w.budget.pages);
                    w.budget.seconds = up(w.budget.seconds);
                }
            }
            dir.save(&ck)?;
        }
        Ok(limit)
    }

    /// Forgets this chat's unfinished run.
    pub fn discard_checkpoint(&self) -> Result<(), String> {
        use crate::checkpoint::CheckpointSink;
        match &self.checkpoints {
            Some(c) => c.clear(&self.session.to_string()),
            None => Ok(()),
        }
    }

    /// Resumes this chat's unfinished run from its checkpoint: a quick turn
    /// with its conversation so far, or a deep run with its approved plan,
    /// finished workers, and pages already read (see
    /// [`crate::deep::resume_deep`]). Nothing done before the stop is paid
    /// for again. Call [`ChatSession::check_last_turn`] afterwards, as after
    /// any turn.
    pub async fn resume_run(
        &mut self,
        opts: &crate::deep::DeepOptions,
        approver: &dyn Approver,
        control: &crate::deep::DeepControl,
        observer: &(dyn Fn(&EventEnvelope) + Send + Sync),
        on_text: Option<crate::OnText<'_>>,
    ) -> Result<TurnOutcome, String> {
        use crate::checkpoint::RunMode;
        let ck = self
            .checkpoint()
            .ok_or("This chat has no unfinished run to resume.")?;
        let tee = Tee {
            inner: &self.sink,
            observer,
        };
        let sink = self
            .checkpoints
            .as_ref()
            .map(|c| c as &dyn crate::checkpoint::CheckpointSink);
        let message = ck.message.clone();
        let out = match ck.mode {
            RunMode::Deep => {
                let Some(search) = self.search.clone() else {
                    return Err("Resuming deep research needs web search. Add a key with `tern search add tavily` (or `brave`), or use your own SearXNG.".into());
                };
                let reader = crate::web::shared_reader();
                let deps = crate::deep::DeepDeps {
                    session: self.session,
                    model: &self.provider,
                    model_name: &self.model,
                    search: search.as_ref(),
                    reader: &reader,
                    sink: &tee,
                    ledger: &self.ledger,
                    gate: Some(&mut self.gate),
                    on_text,
                    checkpoints: sink,
                    steering: Some(&self.steering),
                    decider: Some(&self.decider),
                };
                let mut opts = *opts;
                opts.max_output_tokens = opts.max_output_tokens.max(self.max_output_tokens);
                crate::deep::resume_deep(deps, ck, &opts, control).await
            }
            RunMode::Quick => {
                let instructions = self.instructions(self.search_name.map(|_| WEB_INSTRUCTIONS));
                let hooks = crate::engine::RunHooks {
                    checkpoints: sink,
                    steering: Some(&self.steering),
                    resume: Some(&ck),
                    effort: self.effort,
                    on_reasoning: self.on_reasoning.as_deref(),
                    native_tools: crate::engine::native_tools_ok(
                        self.provider.spec().id,
                        &self.model,
                    ),
                    prefetch: false,
                    retry: None,
                    results: self.results.as_deref(),
                    cache: Some(&self.cache),
                };
                TurnLoop {
                    session: self.session,
                    model: &self.provider,
                    model_name: &self.model,
                    tools: &self.tools,
                    gate: &mut self.gate,
                    approver,
                    sink: &tee,
                    ledger: &self.ledger,
                    budget: self.budget,
                    cancel: control.flag(),
                    instructions: instructions.as_deref(),
                    max_output_tokens: self.max_output_tokens,
                    on_text,
                    hooks,
                }
                .run(self.history.clone(), &message)
                .await
            }
        };
        if let Some(answer) = &out.answer {
            self.history.push(ChatMessage::user(message));
            self.history.push(ChatMessage::assistant(answer.clone()));
        }
        Ok(out)
    }

    /// Checks the latest turn's claims against the pages it read and
    /// writes its run report (`runs/<chat>/turn-<n>.md|json` in the data
    /// folder). Logs `claims.checking`, then `claims.checked` with the
    /// verdicts and the report's paths; both reach `observer`, so a front
    /// end's run shows them. Returns the report's files, or `None` when
    /// the turn has no answer or checks are off.
    pub async fn check_last_turn(
        &self,
        observer: &(dyn Fn(&EventEnvelope) + Send + Sync),
    ) -> Option<crate::report::ReportFiles> {
        use crate::verify::{self, CheckClaims, Judge};
        if self.check_claims == CheckClaims::Off {
            return None;
        }
        let tee = Tee {
            inner: &self.sink,
            observer,
        };
        let events = self
            .sink
            .with_store(|s| s.replay(self.session, 0))
            .ok()?
            .ok()?;
        let turns = past_turns(&events);
        let n = turns.len();
        let last = turns.last()?;
        let raw_answer = last.answer.as_deref().filter(|_| last.stop.is_none())?;
        // Quote tokens become `[n]` (or are stripped when Tern never minted
        // them) before anything reads the claims.
        let minted = verify::minted_quotes(&events, &last.pages);
        let rendered = tern_research::evidence::render(raw_answer, &minted);
        let answer = rendered.text.as_str();
        let claims = verify::extract_claims(answer).len();
        if claims > 0 {
            let _ = tee.record(EventEnvelope::new(
                self.session,
                "claims.checking",
                EffectClass::Analysis,
                format!(
                    "Checking {claims} claim{} against their sources",
                    if claims == 1 { "" } else { "s" }
                ),
                json!({"claims": claims}),
            ));
        }
        let judge = (self.check_claims == CheckClaims::Model && claims > 0).then(|| Judge {
            model: &self.provider,
            model_name: &self.model,
        });
        let checked = verify::check_answer_with(
            answer,
            &last.pages,
            &crate::web::shared_reader(),
            judge,
            Some(&self.decider),
        )
        .await;
        // The evidence chain: every page the turn read, each one's stance
        // on each claim, then counter-searches for key claims (with the
        // model judge only: word-overlap checks stay offline).
        let budget = verify::EvidenceBudget::default();
        let reader = crate::web::shared_reader();
        let sources = verify::all_source_texts(answer, &last.pages, &reader, budget.sources).await;
        let mut chain = tern_research::evidence::build(raw_answer, &sources, &minted, &budget);
        tern_research::evidence::align(&mut chain, &checked.checks);
        let mut judge_tokens = checked.judge_tokens;
        if self.check_claims == CheckClaims::Model
            && let Some(search) = self.search.as_deref()
        {
            for n in tern_research::evidence::plan_counter(&mut chain, &budget) {
                let Some((query, trigger)) = chain
                    .claim(n)
                    .and_then(|c| c.counter.as_ref())
                    .map(|cs| (cs.query.clone(), cs.trigger))
                else {
                    continue;
                };
                let _ = tee.record(EventEnvelope::new(
                    self.session,
                    "claims.countering",
                    EffectClass::Network,
                    format!("Searching for evidence against claim {n}"),
                    json!({"claim": n, "query": query, "trigger": trigger}),
                ));
                let found = verify::counter_pages(
                    search,
                    &reader,
                    &query,
                    &last.pages,
                    budget.counter_reads,
                )
                .await;
                let note = match found {
                    Ok(pages) => {
                        tern_research::evidence::add_counter(&mut chain, n, &pages);
                        chain
                            .claim(n)
                            .and_then(|c| c.counter.as_ref())
                            .map(|cs| cs.note.clone())
                            .unwrap_or_default()
                    }
                    Err(e) => {
                        let note = format!("The search failed: {e}");
                        if let Some(cs) = chain
                            .claims
                            .iter_mut()
                            .find(|c| c.claim == n)
                            .and_then(|c| c.counter.as_mut())
                        {
                            cs.note = note.clone();
                        }
                        note
                    }
                };
                let (read, disagree) = chain
                    .claim(n)
                    .and_then(|c| c.counter.as_ref())
                    .map(|cs| (cs.read, cs.disagree))
                    .unwrap_or_default();
                let _ = tee.record(EventEnvelope::new(
                    self.session,
                    "claims.countered",
                    EffectClass::Network,
                    format!("Counter-searched claim {n}: {note}"),
                    json!({"claim": n, "query": query, "read": read, "disagree": disagree, "note": note}),
                ));
            }
        }
        if self.check_claims == CheckClaims::Model && claims > 0 {
            let judge = Judge {
                model: &self.provider,
                model_name: &self.model,
            };
            if let Ok(Some((i, o))) = verify::judge_stances(&mut chain, &judge, &budget).await {
                let (pi, po) = judge_tokens.unwrap_or_default();
                judge_tokens = Some((pi + i, po + o));
            }
        }
        // The judge's tokens stay out of the chat's ledger, which the
        // context ring reads; the report and the event carry them.
        let mut turn = last.clone();
        turn.answer = Some(answer.to_owned());
        turn.run.checks = checked.checks.clone();
        turn.run.evidence = Some(Box::new(chain.clone()));
        let provider = self.provider.spec().id;
        let mut report =
            crate::report::from_turn(self.session, n, &turn, provider, Some(self.budget));
        if let Some((i, o)) = judge_tokens {
            report.usage.judge_tokens_in = i;
            report.usage.judge_tokens_out = o;
        }
        let written = AppPaths::discover()
            .map_err(|e| e.to_string())
            .and_then(|p| {
                crate::report::write(&crate::report::runs_dir(&p, self.session), &report)
            });
        let tally = crate::verify::Tally::of(&checked.checks);
        let summary = if checked.checks.is_empty() {
            "Wrote the run report; the answer makes no cited claims".to_owned()
        } else {
            format!("Checked {} claims: {}", checked.checks.len(), tally.line())
        };
        let mut payload = json!({
            "checks": checked.checks,
            "tally": tally,
            "report": written.as_ref().ok(),
            "evidence": chain,
        });
        if !rendered.forged.is_empty() {
            payload["forged"] = json!(rendered.forged.iter().map(|(_, t)| t).collect::<Vec<_>>());
        }
        if let Some(e) = &checked.judge_error {
            payload["judge_error"] = json!(e);
        }
        if !checked.decisions.is_empty() {
            payload["decisions"] = json!(checked.decisions);
        }
        if let Some((i, o)) = judge_tokens {
            payload["judge_tokens"] = json!({"input": i, "output": o});
        }
        if let Err(e) = &written {
            payload["report_error"] = json!(e);
        }
        let _ = tee.record(EventEnvelope::new(
            self.session,
            "claims.checked",
            EffectClass::Analysis,
            summary,
            payload,
        ));
        written.ok()
    }
}

/// How full a chat's context window is (see [`ChatSession::context`]).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct ContextUse {
    /// Tokens the model read on its latest call.
    pub prompt: u64,
    /// Tokens it wrote on that call.
    pub reply: u64,
    /// The model's window, when the catalog knows it.
    pub window: Option<u64>,
    /// Counted at four characters a token rather than by the provider.
    pub estimated: bool,
    /// Every call in this window summed, which is what providers bill.
    pub session_tokens: u64,
    pub calls: usize,
    /// What the context holds, split by kind. Always an estimate (four
    /// characters a token), scaled to the provider's count when there is one.
    pub parts: ContextParts,
}

/// The context split by kind, in tokens.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct ContextParts {
    /// Tern's system prompt and the user's rules (TERN.md, project).
    pub system: u64,
    /// Tool descriptions and the tool protocol.
    pub tools: u64,
    /// Attached files, sources, and MCP resources in the history.
    pub attachments: u64,
    /// Messages and replies.
    pub conversation: u64,
    /// The pinned summary of compacted turns.
    pub summary: u64,
}

impl ContextParts {
    /// From character counts, at four characters a token.
    pub fn estimate(system: usize, tools: usize, attachments: usize, conversation: usize) -> Self {
        let t = |c: usize| Usage::estimated(c, 0).input_tokens;
        Self {
            system: t(system),
            tools: t(tools),
            attachments: t(attachments),
            conversation: t(conversation),
            summary: 0,
        }
    }

    /// The same, with a pinned summary of `chars` characters.
    pub fn with_summary(self, chars: usize) -> Self {
        Self {
            summary: Usage::estimated(chars, 0).input_tokens,
            ..self
        }
    }

    pub fn total(&self) -> u64 {
        self.system + self.tools + self.attachments + self.conversation + self.summary
    }

    /// The same shares of a measured prompt, with the reply added to the
    /// conversation.
    pub fn scaled_to(self, prompt: u64, reply: u64) -> Self {
        let total = self.total();
        if total == 0 {
            return Self {
                conversation: prompt + reply,
                ..Self::default()
            };
        }
        let share = |v: u64| ((v as u128 * prompt as u128) / total as u128) as u64;
        let (system, tools, attachments, summary) = (
            share(self.system),
            share(self.tools),
            share(self.attachments),
            share(self.summary),
        );
        Self {
            system,
            tools,
            attachments,
            summary,
            conversation: prompt.saturating_sub(system + tools + attachments + summary) + reply,
        }
    }
}

/// Characters of a message inside attachment or untrusted-data blocks.
fn attached_chars(text: &str) -> usize {
    let mut n = 0;
    for (open, close) in [
        ("<attachment", "</attachment>"),
        ("<untrusted-data", "</untrusted-data>"),
    ] {
        let mut rest = text;
        while let Some(i) = rest.find(open) {
            let after = &rest[i..];
            match after.find(close) {
                Some(j) => {
                    n += j + close.len();
                    rest = &after[j + close.len()..];
                }
                None => {
                    n += after.len();
                    break;
                }
            }
        }
    }
    n.min(text.len())
}

impl ContextUse {
    /// Tokens in the context: the latest prompt plus its reply.
    pub fn used(&self) -> u64 {
        self.prompt + self.reply
    }

    /// From 0.0 to 1.0, when the window is known.
    pub fn fraction(&self) -> Option<f32> {
        let w = self.window.filter(|w| *w > 0)?;
        Some((self.used() as f32 / w as f32).clamp(0.0, 1.0))
    }

    /// Tokens still free, when the window is known.
    pub fn left(&self) -> Option<u64> {
        self.window.map(|w| w.saturating_sub(self.used()))
    }
}

/// Takes out every exchange whose assistant message is empty (with the
/// user message it answered), as a reopened chat's history already leaves
/// them out ([`crate::transcript::PastTurn::exchange`]). Providers refuse a
/// request with an empty assistant message. Returns how many went.
pub fn tidy_history(messages: &mut Vec<ChatMessage>) -> usize {
    let before = messages.len();
    let mut out: Vec<ChatMessage> = Vec::with_capacity(messages.len());
    for m in messages.drain(..) {
        let empty = m.role == tern_providers::Role::Assistant
            && m.content.trim().is_empty()
            && m.images.is_empty();
        if empty {
            if out
                .last()
                .is_some_and(|p| p.role == tern_providers::Role::User)
            {
                out.pop();
            }
            continue;
        }
        out.push(m);
    }
    *messages = out;
    before - messages.len()
}

/// Takes the pictures out of `messages`, leaving "[image: name]" in each
/// message that had one. Returns how many were taken.
pub fn strip_pictures(messages: &mut [ChatMessage]) -> usize {
    let mut n = 0;
    for m in messages {
        if m.images.is_empty() {
            continue;
        }
        n += m.images.len();
        let names: Vec<String> = m.images.drain(..).map(|i| i.name).collect();
        for name in names {
            m.content.push_str(&format!(
                "\n\n[image: {name} (not sent: this model cannot see pictures)]"
            ));
        }
    }
    n
}

#[cfg(test)]
mod context_tests {
    use super::ContextUse;

    #[test]
    fn context_fraction_and_room() {
        let c = ContextUse {
            prompt: 90_000,
            reply: 6_000,
            window: Some(128_000),
            ..ContextUse::default()
        };
        assert_eq!(c.used(), 96_000);
        assert_eq!(c.left(), Some(32_000));
        assert!((c.fraction().unwrap() - 0.75).abs() < 1e-6);
        let over = ContextUse {
            prompt: 200,
            window: Some(100),
            ..ContextUse::default()
        };
        assert_eq!(over.fraction(), Some(1.0));
        assert_eq!(over.left(), Some(0));
        assert_eq!(ContextUse::default().fraction(), None);
    }
}

#[cfg(test)]
mod title_tests {
    use super::title_from;

    #[test]
    fn titles_are_one_short_line() {
        assert_eq!(title_from("  What is\nattention?  "), "What is attention?");
        let long = "Compare the evaluation methods used in retrieval augmented generation papers from 2023 and 2024";
        let t = title_from(long);
        assert!(t.ends_with('…'));
        assert!(t.chars().count() <= 61);
    }

    #[test]
    fn titles_stop_before_the_attachments() {
        use crate::attach::{Attachment, compose};
        let a = Attachment::paste("The meeting is on Tuesday.");
        let msg = compose("Summarise this", std::slice::from_ref(&a));
        assert!(msg.contains("<attachments>"));
        assert_eq!(title_from(&msg), "Summarise this");
        // Attachments alone: titled by their names.
        let alone = compose("", std::slice::from_ref(&a));
        assert_eq!(title_from(&alone), a.name);
        let raw = "<attachments>\nAttachments from the user.\n\
                   <attachment name=\"Q3.pdf\" role=\"context\" files=\"1\">\nx\n</attachment>\n\
                   <attachment name=\"notes.md\" role=\"prompt\" files=\"1\">\ny\n</attachment>\n</attachments>";
        assert_eq!(title_from(raw), "Q3.pdf, notes.md");
    }
}

#[cfg(test)]
mod context_parts_tests {
    use super::*;

    #[test]
    fn attached_text_is_counted_apart_from_the_talk() {
        let msg = "Summarise this <attachment name=\"a.md\">0123456789</attachment> please";
        let a = attached_chars(msg);
        assert_eq!(a, "<attachment name=\"a.md\">0123456789</attachment>".len());
        assert_eq!(attached_chars("no files here"), 0);
        // An unclosed block counts to the end.
        assert_eq!(
            attached_chars("x<untrusted-data src>abc"),
            "<untrusted-data src>abc".len()
        );
    }

    #[test]
    fn parts_scale_to_the_provider_count() {
        let p = ContextParts::estimate(400, 400, 800, 1_600);
        assert_eq!(p.total(), 800);
        let m = p.scaled_to(1_600, 50);
        assert_eq!(m.system, 200);
        assert_eq!(m.tools, 200);
        assert_eq!(m.attachments, 400);
        assert_eq!(m.conversation, 800 + 50);
        assert_eq!(m.total(), 1_650);
        assert_eq!(ContextParts::default().scaled_to(10, 2).conversation, 12);
    }
}

#[cfg(test)]
mod output_tests {
    use super::*;

    #[test]
    fn outputs_are_saves_and_file_writes_newest_last() {
        let ses = SessionId::new();
        let ev = |kind: &str, payload: serde_json::Value| {
            EventEnvelope::new(ses, kind, EffectClass::Analysis, "x", payload)
        };
        let events = vec![
            ev("session.workspace", json!({"path": "/w"})),
            ev(
                "tool.executed",
                json!({"tool": "file_write", "args": {"path": "notes/a.md"}}),
            ),
            ev(
                "tool.executed",
                json!({"tool": "file_read", "args": {"path": "b.md"}}),
            ),
            ev(
                "session.output",
                json!({"path": "/elsewhere/table-1.csv", "what": "export"}),
            ),
            ev(
                "tool.executed",
                json!({"tool": "file_write", "args": {"path": "notes/a.md"}}),
            ),
        ];
        let got = outputs_in(&events, None);
        let paths: Vec<_> = got.iter().map(|o| o.path.clone()).collect();
        assert_eq!(
            paths,
            [
                std::path::PathBuf::from("/elsewhere/table-1.csv"),
                std::path::Path::new("/w").join("notes/a.md")
            ]
        );
        assert_eq!(got[1].what, "file_write");
    }

    /// #77: renamed outputs follow their file; Windows' `\\?\` never
    /// shows in an output's path.
    #[test]
    fn a_rename_moves_the_output_and_paths_are_plain() {
        let ses = SessionId::new();
        let ev = |kind: &str, payload: serde_json::Value| {
            EventEnvelope::new(ses, kind, EffectClass::Analysis, "x", payload)
        };
        let events = vec![
            ev(
                "session.workspace",
                json!({"path": r"\\?\D:\Codex\Tern testing"}),
            ),
            ev(
                "tool.executed",
                json!({"tool": "file_write", "args": {"path": "notes.md"}}),
            ),
            ev(
                "tool.executed",
                json!({"tool": "file_rename", "args": {"from": "notes.md", "to": "summary.md"}}),
            ),
            ev(
                "tool.executed",
                json!({"tool": "file_rename", "args": {"from": "summary.md", "to": "sum.md"}}),
            ),
        ];
        let got = outputs_in(&events, None);
        assert_eq!(got.len(), 1);
        let shown = got[0].path.to_string_lossy().replace('\\', "/");
        assert!(!shown.contains('?'), "{shown}");
        assert!(shown.ends_with("Tern testing/sum.md"), "{shown}");
        // The window's own list moves the same way.
        let mut outs = vec![Output::now("/w/a.md".into(), "file_write")];
        assert!(rename_output(
            &mut outs,
            std::path::Path::new("/w/a.md"),
            std::path::Path::new("/w/b.md")
        ));
        assert_eq!(outs[0].path, std::path::PathBuf::from("/w/b.md"));
        assert!(!rename_output(
            &mut outs,
            std::path::Path::new("/w/zzz.md"),
            std::path::Path::new("/w/c.md")
        ));
    }

    #[test]
    fn the_workspace_line_names_the_folder_and_how_paths_work() {
        let d = tempfile::tempdir().unwrap();
        let root = d.path().join("New");
        std::fs::create_dir_all(&root).unwrap();
        let ws = crate::workspace::Workspace::open(&root).unwrap();
        let line = workspace_line(&ws);
        assert!(line.contains("the folder `New`"), "{line}");
        assert!(line.contains("Do not start a path with `New/`"), "{line}");
        assert!(line.contains("`file_write` makes any folders"), "{line}");
    }

    #[test]
    fn outputs_whose_file_is_gone_are_pruned() {
        let d = tempfile::tempdir().unwrap();
        let kept = d.path().join("hello.md");
        std::fs::write(&kept, "hello").unwrap();
        let gone = d.path().join("counts.md");
        std::fs::write(&gone, "1").unwrap();
        let mut outs = vec![
            Output::now(gone.clone(), "file_write"),
            Output::now(d.path().join("review.md"), "file_write"),
            Output::now(kept.clone(), "file_write"),
        ];
        std::fs::remove_file(&gone).unwrap();
        assert_eq!(prune_outputs(&mut outs), 2);
        assert_eq!(outs.len(), 1);
        assert_eq!(outs[0].path, kept);
        assert_eq!(prune_outputs(&mut outs), 0);
    }
}
