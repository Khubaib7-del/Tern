//! A chat session for front ends: one provider, one model, one stored session.
//!
//! The TUI and the desktop app open a [`ChatSession`] and call
//! [`ChatSession::send`] per message. They never touch providers, the
//! keychain, or the database themselves.

use std::sync::Arc;

use serde_json::json;
use tern_core::{EffectClass, EventEnvelope, SessionId};
use tern_platform::AppPaths;
use tern_providers::{ChatMessage, ModelKind, Provider, UsageLedger};
use tern_store::{Harness, SessionSummary, Store};

pub use tern_providers::Usage;

use crate::engine::{Approver, Budget, CancelFlag, EventSink, StoreSink, TurnLoop, TurnOutcome};
use crate::policy::PolicyGate;
use crate::providers::{self, SecretStore, SetupError};
use crate::search;
use crate::tools::ToolRegistry;
use crate::web::{ReadUrlTool, WebSearchTool};

/// The project quick chats are filed under until projects have a UI.
pub const INBOX: &str = "Inbox";

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
    search_name: Option<&'static str>,
    budget: Budget,
    max_output_tokens: u32,
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

    /// [`ChatSession::open`] with the keychain and database given, for tests.
    pub async fn open_with(
        opts: ChatOptions,
        secrets: Option<&dyn SecretStore>,
        store: Store,
    ) -> Result<Self, String> {
        let id = match opts.provider {
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
        let (model, context_window) = match opts.model {
            Some(m) => (m, None),
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
        let mut tools = ToolRegistry::new();
        let search =
            match search::connect(opts.search.as_deref(), secrets, opts.search_url.as_deref()) {
                Ok(c) => Some(c),
                Err(e) if opts.search.is_some() => return Err(e),
                Err(_) => None,
            };
        let search_name = search.as_ref().map(|c| c.spec().name);
        if let Some(c) = search {
            tools.register(WebSearchTool(Arc::new(c)));
            tools.register(ReadUrlTool::default());
        }
        let project = store.ensure_project(INBOX).map_err(|e| e.to_string())?;
        let session = store
            .create_session(project, Harness::Research, "Chat")
            .map_err(|e| e.to_string())?;
        let s = Self {
            provider,
            model,
            context_window,
            session,
            sink: StoreSink::new(store),
            gate: PolicyGate::default(),
            ledger: UsageLedger::default(),
            tools,
            history: Vec::new(),
            search_name,
            budget: opts.budget,
            max_output_tokens: if opts.max_output_tokens == 0 {
                2048
            } else {
                opts.max_output_tokens
            },
        };
        s.sink.record(EventEnvelope::new(
            session,
            "session.opened",
            EffectClass::Analysis,
            format!(
                "Opened a chat with {} on {}",
                s.model,
                s.provider.spec().name
            ),
            json!({"provider": s.provider.spec().id, "model": s.model}),
        ))?;
        Ok(s)
    }

    pub fn provider_name(&self) -> &'static str {
        self.provider.spec().name
    }

    pub fn model(&self) -> &str {
        &self.model
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

    /// Runs one turn. Completed exchanges join the history for the next turn.
    pub async fn send(
        &mut self,
        text: &str,
        approver: &dyn Approver,
        cancel: CancelFlag,
        observer: &(dyn Fn(&EventEnvelope) + Send + Sync),
    ) -> TurnOutcome {
        let tee = Tee {
            inner: &self.sink,
            observer,
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
            instructions: self.search_name.map(|_| WEB_INSTRUCTIONS),
            max_output_tokens: self.max_output_tokens,
        }
        .run(self.history.clone(), text)
        .await;
        if self.history.is_empty() {
            let title = title_from(text);
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
