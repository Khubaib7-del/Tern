//! The throwaway side chat (`docs/02-ui-definition.md`): quick questions
//! that read the thread but write nothing back. It opens the chat's log
//! to read, asks the model with the thread as history, and keeps its own
//! exchanges in memory only. Nothing is appended to the parent's log, its
//! title, or its checkpoints.

use tern_core::SessionId;
use tern_platform::AppPaths;
use tern_providers::{ChatMessage, ChatRequest};
use tern_store::Store;

use crate::chat::{ChatOptions, history_of};
use crate::engine::Model;
use crate::providers::{self, SecretStore};
use crate::transcript::{live_turns, trailing_context};

pub(crate) const SYSTEM: &str = "You are answering a quick side question about the conversation above. Answer briefly and plainly. This side chat is not saved to the conversation.";

/// One side question and its answer.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SideExchange {
    pub question: String,
    pub answer: String,
}

/// The thread a side chat reads: the chat's live turns as model history.
pub fn thread(store: &Store, session: SessionId) -> Result<Vec<ChatMessage>, String> {
    if store.session(session).map_err(|e| e.to_string())?.is_none() {
        return Err("That chat is no longer in the database.".into());
    }
    let events = store.replay(session, 0).map_err(|e| e.to_string())?;
    Ok(history_of(
        &live_turns(&events),
        trailing_context(&events).as_deref(),
    ))
}

/// Asks `question` with `thread` and the side chat's own earlier
/// exchanges as history. `focus` is text the user highlighted.
pub async fn ask_with(
    model: &dyn Model,
    model_name: &str,
    thread: &[ChatMessage],
    earlier: &[SideExchange],
    question: &str,
    focus: Option<&str>,
) -> Result<SideExchange, String> {
    let question = question.trim();
    if question.is_empty() {
        return Err("Ask something.".into());
    }
    let mut messages = thread.to_vec();
    for x in earlier {
        messages.push(ChatMessage::user(x.question.clone()));
        messages.push(ChatMessage::assistant(x.answer.clone()));
    }
    let text = match focus.map(str::trim).filter(|f| !f.is_empty()) {
        Some(f) => format!("About this part of the conversation:\n> {f}\n\n{question}"),
        None => question.to_owned(),
    };
    messages.push(ChatMessage::user(text));
    let req = ChatRequest::new(model_name, messages)
        .with_system(SYSTEM)
        .with_max_tokens(900);
    let reply = model.complete(&req).await.map_err(|e| e.to_string())?;
    Ok(SideExchange {
        question: question.to_owned(),
        answer: reply.text.trim().to_owned(),
    })
}

/// A side chat over one chat: it reads the thread once and keeps its own
/// exchanges in memory.
pub struct SideChat {
    model: Box<dyn Model>,
    model_name: String,
    thread: Vec<ChatMessage>,
    pub exchanges: Vec<SideExchange>,
}

impl std::fmt::Debug for SideChat {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SideChat")
            .field("model", &self.model_name)
            .field("thread", &self.thread.len())
            .field("exchanges", &self.exchanges.len())
            .finish()
    }
}

impl SideChat {
    pub fn with_model(model: Box<dyn Model>, model_name: &str, thread: Vec<ChatMessage>) -> Self {
        Self {
            model,
            model_name: model_name.to_owned(),
            thread,
            exchanges: Vec::new(),
        }
    }

    /// Opens a side chat over `session` with the user's saved choices. The
    /// database is read once and closed.
    pub async fn open(
        opts: ChatOptions,
        secrets: Option<&dyn SecretStore>,
        session: SessionId,
    ) -> Result<Self, String> {
        let paths = AppPaths::discover().map_err(|e| e.to_string())?;
        let db = paths.database();
        if !db.exists() {
            return Err("There are no chats yet.".into());
        }
        let thread = {
            let store = Store::open(&db).map_err(|e| e.to_string())?;
            thread(&store, session)?
        };
        let (provider, model, _) = crate::chat::connect_model(&opts, secrets).await?;
        Ok(Self::with_model(Box::new(provider), &model, thread))
    }

    /// [`SideChat::open`] with the OS keychain.
    pub async fn open_default(opts: ChatOptions, session: SessionId) -> Result<Self, String> {
        let keychain = providers::keychain();
        let secrets: Option<&dyn SecretStore> = keychain.as_ref().ok().map(|b| b.as_ref());
        Self::open(opts, secrets, session).await
    }

    pub fn model_name(&self) -> &str {
        &self.model_name
    }

    /// Messages of the thread it read.
    pub fn thread_len(&self) -> usize {
        self.thread.len()
    }

    pub async fn ask(&mut self, question: &str, focus: Option<&str>) -> Result<String, String> {
        let x = ask_with(
            self.model.as_ref(),
            &self.model_name,
            &self.thread,
            &self.exchanges,
            question,
            focus,
        )
        .await?;
        let answer = x.answer.clone();
        self.exchanges.push(x);
        Ok(answer)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::chat::INBOX;
    use futures::future::BoxFuture;
    use serde_json::json;
    use std::sync::Mutex;
    use tern_core::{EffectClass, EventEnvelope};
    use tern_providers::{ChatResponse, ProviderResult};
    use tern_store::Harness;

    struct Echo(Mutex<Vec<ChatRequest>>);

    impl Model for Echo {
        fn provider_id(&self) -> &str {
            "echo"
        }
        fn complete<'a>(
            &'a self,
            req: &'a ChatRequest,
        ) -> BoxFuture<'a, ProviderResult<ChatResponse>> {
            self.0.lock().unwrap().push(req.clone());
            Box::pin(async move {
                Ok(ChatResponse {
                    text: format!("side answer {}", req.messages.len()),
                    usage: tern_providers::Usage::measured(0, 0),
                    finish_reason: None,
                    model: None,
                })
            })
        }
    }

    #[tokio::test]
    async fn the_side_chat_reads_the_thread_and_writes_nothing_back() {
        let mut store = Store::open_in_memory().unwrap();
        let p = store.ensure_project(INBOX).unwrap();
        let s = store.create_session(p, Harness::Research, "RAG").unwrap();
        for (kind, payload) in [
            ("turn.started", json!({"message": "What is RAG?"})),
            (
                "turn.finished",
                json!({"state": "completed", "answer": "Retrieval-augmented generation."}),
            ),
        ] {
            store
                .append(&EventEnvelope::new(
                    s,
                    kind,
                    EffectClass::Analysis,
                    kind,
                    payload,
                ))
                .unwrap();
        }
        let before = store.replay(s, 0).unwrap();
        let title = store.session(s).unwrap().unwrap().title;

        let echo = Echo(Mutex::new(Vec::new()));
        let t = thread(&store, s).unwrap();
        let x = ask_with(&echo, "m", &t, &[], "Shorter?", Some("Retrieval-augmented"))
            .await
            .unwrap();
        let y = ask_with(&echo, "m", &t, std::slice::from_ref(&x), "And?", None)
            .await
            .unwrap();
        assert_eq!(y.answer, "side answer 5");
        let reqs = echo.0.lock().unwrap().clone();
        assert_eq!(reqs[0].messages[0].content, "What is RAG?");
        assert!(
            reqs[0].messages[2]
                .content
                .contains("> Retrieval-augmented")
        );
        assert_eq!(reqs[1].messages[3].content, "side answer 3");

        // The parent is exactly as it was.
        assert_eq!(store.replay(s, 0).unwrap(), before);
        assert_eq!(store.session(s).unwrap().unwrap().title, title);
        assert!(ask_with(&echo, "m", &t, &[], " ", None).await.is_err());
    }
}
