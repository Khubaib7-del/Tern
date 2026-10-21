//! A chat session for front ends: one provider, one model, one stored session.
//!
//! The TUI and the desktop app open a [`ChatSession`] and call
//! [`ChatSession::send`] per message. They never touch providers, the
//! keychain, or the database themselves.

use serde_json::json;
use tern_core::{EffectClass, EventEnvelope, SessionId};
use tern_platform::AppPaths;
use tern_providers::{ChatMessage, ModelKind, Provider, UsageLedger};
use tern_store::{Harness, Store};

pub use tern_providers::Usage;

use crate::engine::{Approver, Budget, CancelFlag, EventSink, StoreSink, TurnLoop, TurnOutcome};
use crate::policy::PolicyGate;
use crate::providers::{self, SecretStore, SetupError};
use crate::tools::ToolRegistry;

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
}

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
    budget: Budget,
    max_output_tokens: u32,
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
            tools: ToolRegistry::new(),
            history: Vec::new(),
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
            instructions: None,
            max_output_tokens: self.max_output_tokens,
        }
        .run(self.history.clone(), text)
        .await;
        if let Some(answer) = &out.answer {
            self.history.push(ChatMessage::user(text));
            self.history.push(ChatMessage::assistant(answer.clone()));
        }
        out
    }
}
