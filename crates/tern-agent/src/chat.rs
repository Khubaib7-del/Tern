//! A chat session for front ends: one provider, one model, one stored session.
//!
//! The TUI and the desktop app open a [`ChatSession`] and call
//! [`ChatSession::send`] per message. They never touch providers, the
//! keychain, or the database themselves.

use std::sync::Arc;

use serde_json::json;
use tern_core::{EffectClass, EventEnvelope, Measure, SessionId};
use tern_platform::AppPaths;
use tern_providers::{ChatMessage, ModelKind, Provider, UsageLedger};
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
}

const WEB_INSTRUCTIONS: &str = "You can search the web and read pages with your tools. Read a page before you rely on it, and cite pages you read inline as [n] with a short list of the links at the end.";

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
    /// The chat has its sidebar title (from its first message).
    titled: bool,
    search_name: Option<&'static str>,
    /// The search service, shared with the web tools, for deep research.
    search: Option<Arc<tern_crawl::SearchClient>>,
    budget: Budget,
    max_output_tokens: u32,
    /// The user set the answer length; a model change keeps it.
    output_set: bool,
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
}

/// A sidebar title from the first message: one line, about 60 characters.
pub fn title_from(text: &str) -> String {
    let line = text.split_whitespace().collect::<Vec<_>>().join(" ");
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
            let window = provider
                .list_models()
                .await
                .ok()
                .and_then(|ms| ms.into_iter().find(|x| x.id == m))
                .and_then(|x| x.context_window);
            (m, window)
        }
        None => {
            let models = provider.list_models().await.map_err(|e| e.to_string())?;
            let m = models
                .into_iter()
                .find(|m| m.kind == ModelKind::Chat)
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
    let mut e =
        crate::rules::effective(web.then_some(WEB_INSTRUCTIONS), &project, &tools, untrusted);
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
        s.history = history_of(&turns, trailing_context(&events).as_deref());
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
        let mut gate = PolicyGate::default();
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
        let untrusted = tools.names().any(|n| n.starts_with("mcp__"));
        let paths = AppPaths::discover().ok();
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
            ledger: UsageLedger::default(),
            tools,
            history: Vec::new(),
            next_images: Vec::new(),
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

    pub fn model(&self) -> &str {
        &self.model
    }

    /// Whether the chat is still a draft: nothing has been sent, so it has
    /// no row in the chat list.
    pub fn is_draft(&self) -> bool {
        self.sink.is_draft(self.session)
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
            }
            None => {
                self.tools.unregister("web_search");
                self.tools.unregister("read_url");
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
        match self.ledger.last() {
            Some(u) => ContextUse {
                prompt: u.input_tokens,
                reply: u.output_tokens,
                estimated: u.measure == Measure::Estimated,
                ..base
            },
            None => {
                let chars: usize = self.history.iter().map(|m| m.content.len()).sum::<usize>()
                    + crate::protocol::system_prompt(
                        self.instructions(None).as_deref(),
                        &self.tools,
                    )
                    .len();
                ContextUse {
                    prompt: Usage::estimated(chars, 0).input_tokens,
                    estimated: true,
                    ..base
                }
            }
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
        crate::rules::instructions(harness, &self.project, self.untrusted)
    }

    /// Exactly what the model is told on this chat's next turn, piece by
    /// piece (see [`crate::rules::effective`]).
    pub fn effective_instructions(&self) -> crate::rules::Effective {
        let harness = self.search_name.map(|_| WEB_INSTRUCTIONS);
        let mut e = crate::rules::effective(harness, &self.project, &self.tools, self.untrusted);
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

    pub async fn send_streaming(
        &mut self,
        text: &str,
        approver: &dyn Approver,
        cancel: CancelFlag,
        observer: &(dyn Fn(&EventEnvelope) + Send + Sync),
        on_text: Option<crate::OnText<'_>>,
    ) -> TurnOutcome {
        let images = std::mem::take(&mut self.next_images);
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
        let hooks = crate::engine::RunHooks {
            checkpoints: self
                .checkpoints
                .as_ref()
                .map(|c| c as &dyn crate::checkpoint::CheckpointSink),
            steering: Some(&self.steering),
            resume: None,
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
            cancel,
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
        if !self.titled {
            self.titled = true;
            let title = title_from(text);
            let _ = self
                .sink
                .with_store(|s| s.rename_session(self.session, &title));
        }
        if let Some(answer) = &out.answer {
            // Pictures stay in the history so follow-up questions can
            // still see them.
            self.history
                .push(ChatMessage::user(text).with_images(images));
            self.history.push(ChatMessage::assistant(answer.clone()));
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
        }
        out
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
        let answer = last.answer.as_deref().filter(|_| last.stop.is_none())?;
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
        // The judge's tokens stay out of the chat's ledger, which the
        // context ring reads; the report and the event carry them.
        let mut turn = last.clone();
        turn.run.checks = checked.checks.clone();
        let provider = self.provider.spec().id;
        let mut report =
            crate::report::from_turn(self.session, n, &turn, provider, Some(self.budget));
        if let Some((i, o)) = checked.judge_tokens {
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
        });
        if let Some(e) = &checked.judge_error {
            payload["judge_error"] = json!(e);
        }
        if !checked.decisions.is_empty() {
            payload["decisions"] = json!(checked.decisions);
        }
        if let Some((i, o)) = checked.judge_tokens {
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
}
