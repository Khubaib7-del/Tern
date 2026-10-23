//! Context compaction against a mock OpenAI-compatible server: every
//! answer reports a prompt of 110k tokens in a 128k window (86 %), and
//! the summary call gets its own reply.

use std::sync::Mutex;

use serde_json::json;
use tern_agent::{CancelFlag, ChatOptions, ChatSession, Compaction, DenyAll};
use tern_store::Store;
use wiremock::matchers::{body_string_contains, method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

async fn server() -> MockServer {
    let s = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/v1/models"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "data": [{"id": "mistral-small"}]
        })))
        .mount(&s)
        .await;
    Mock::given(method("POST"))
        .and(path("/v1/chat/completions"))
        .and(body_string_contains("Summarise the earlier part"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "choices": [{"message": {"role": "assistant", "content": "- RAG retrieves passages first."}, "finish_reason": "stop"}],
            "usage": {"prompt_tokens": 900, "completion_tokens": 40}
        })))
        .with_priority(1)
        .mount(&s)
        .await;
    Mock::given(method("POST"))
        .and(path("/v1/chat/completions"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "choices": [{"message": {"role": "assistant", "content": "An answer."}, "finish_reason": "stop"}],
            "usage": {"prompt_tokens": 110000, "completion_tokens": 50}
        })))
        .with_priority(5)
        .mount(&s)
        .await;
    s
}

fn opts(s: &MockServer, compaction: Compaction) -> ChatOptions {
    ChatOptions {
        provider: Some("ollama".into()),
        model: Some("mistral-small".into()),
        base_url: Some(format!("{}/v1", s.uri())),
        search: Some("off".into()),
        compaction,
        ..Default::default()
    }
}

/// The non-system messages of the last answer request (not a summary call).
async fn last_chat_messages(s: &MockServer) -> Vec<String> {
    let reqs = s.received_requests().await.unwrap();
    let body: serde_json::Value = reqs
        .iter()
        .rev()
        .map(|r| r.body_json::<serde_json::Value>().unwrap())
        .find(|b| !b.to_string().contains("Summarise the earlier part"))
        .unwrap();
    body["messages"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|m| m["role"] != "system")
        .map(|m| m["content"].as_str().unwrap_or_default().to_owned())
        .collect()
}

#[tokio::test]
async fn compacts_past_the_threshold_keeps_turns_logs_and_resumes() {
    let s = server().await;
    let dir = tempfile::tempdir().unwrap();
    let db = dir.path().join("tern.db");
    let settings = Compaction {
        auto: true,
        at: 0.8,
        keep_turns: 1,
    };
    let kinds = Mutex::new(Vec::new());
    let observer = |e: &tern_core::EventEnvelope| kinds.lock().unwrap().push(e.kind.clone());
    let mut chat = ChatSession::open_with(opts(&s, settings), None, Store::open(&db).unwrap())
        .await
        .unwrap();
    let id = chat.session_id();
    assert!(chat.context_window().is_some(), "the window is known");

    // One turn at 86 %: over the threshold, but the one turn is the one
    // kept, so there is nothing to fold yet.
    chat.send("What is RAG?", &DenyAll, CancelFlag::default(), &observer)
        .await;
    assert!(chat.compacted().is_none());

    // The second turn folds the first.
    chat.send("And reranking?", &DenyAll, CancelFlag::default(), &observer)
        .await;
    let c = chat.compacted().cloned().expect("compacted");
    assert!(c.auto);
    assert_eq!(c.turns, [1]);
    assert_eq!(c.kept, 1);
    assert_eq!(c.summary, "- RAG retrieves passages first.");
    assert!(
        kinds
            .lock()
            .unwrap()
            .iter()
            .any(|k| k == tern_agent::compact::EVENT),
        "session.compacted reached the front end"
    );
    // The context is estimated from the shorter history until the next call,
    // and the breakdown has the summary as its own part.
    let ctx = chat.context();
    assert!(ctx.estimated && ctx.parts.summary > 0);
    assert!(ctx.fraction().unwrap() < 0.8);

    // The next call carries the summary, then the kept turn, word for word.
    chat.send("Thanks", &DenyAll, CancelFlag::default(), &observer)
        .await;
    let msgs = last_chat_messages(&s).await;
    assert!(tern_agent::compact::is_pinned(&msgs[0]), "{msgs:?}");
    assert_eq!(&msgs[2..], ["And reranking?", "An answer.", "Thanks"]);

    // Nothing was deleted: the log still has every turn.
    let events = chat.with_store(|st| st.replay(id, 0).unwrap()).unwrap();
    assert_eq!(tern_agent::live_turns(&events).len(), 3);
    let logged = tern_agent::compact::last(&events).unwrap();
    // The third turn passed the threshold again and folded the second.
    assert_eq!(logged.turns, [1, 2]);
    drop(chat);

    // Reopening rebuilds the same history from the last compaction.
    let (mut back, turns) =
        ChatSession::resume_with(opts(&s, settings), None, Store::open(&db).unwrap(), id)
            .await
            .unwrap();
    assert_eq!(turns.len(), 3, "the transcript keeps every turn");
    assert_eq!(back.compacted(), Some(&logged));
    back.send(
        "More?",
        &DenyAll,
        CancelFlag::default(),
        &|_: &tern_core::EventEnvelope| {},
    )
    .await;
    let msgs = last_chat_messages(&s).await;
    assert!(tern_agent::compact::is_pinned(&msgs[0]));
    assert_eq!(&msgs[2..], ["Thanks", "An answer.", "More?"]);
}

#[tokio::test]
async fn off_never_compacts_but_compact_now_does() {
    let s = server().await;
    let off = Compaction {
        auto: false,
        ..Compaction::default()
    };
    let mut chat = ChatSession::open_with(opts(&s, off), None, Store::open_in_memory().unwrap())
        .await
        .unwrap();
    for q in ["one", "two", "three", "four", "five", "six"] {
        chat.send(
            q,
            &DenyAll,
            CancelFlag::default(),
            &|_: &tern_core::EventEnvelope| {},
        )
        .await;
    }
    assert!(chat.compacted().is_none(), "auto is off");
    // Compact now keeps the last four turns.
    let c = chat
        .compact(false, &|_: &tern_core::EventEnvelope| {})
        .await
        .unwrap()
        .unwrap();
    assert!(!c.auto);
    assert_eq!(c.turns, [1, 2]);
    assert!(
        chat.compact(false, &|_: &tern_core::EventEnvelope| {})
            .await
            .unwrap()
            .is_none(),
        "nothing left to fold"
    );
}
