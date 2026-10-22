//! `ChatSession` against a mock OpenAI-compatible local server.

use std::sync::Mutex;

use serde_json::json;
use tern_agent::{CancelFlag, ChatOptions, ChatSession, DenyAll};
use tern_core::TurnState;
use tern_store::Store;
use wiremock::matchers::{method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

async fn server() -> MockServer {
    let s = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/v1/models"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "data": [{"id": "nomic-embed-text"}, {"id": "llama3.2"}]
        })))
        .mount(&s)
        .await;
    Mock::given(method("POST"))
        .and(path("/v1/chat/completions"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "choices": [{"message": {"role": "assistant", "content": "Hello from the mock."}, "finish_reason": "stop"}],
            "usage": {"prompt_tokens": 12, "completion_tokens": 5}
        })))
        .mount(&s)
        .await;
    s
}

#[tokio::test]
async fn opens_picks_a_chat_model_and_keeps_history() {
    let s = server().await;
    let opts = ChatOptions {
        provider: Some("ollama".into()),
        base_url: Some(format!("{}/v1", s.uri())),
        ..Default::default()
    };
    let mut chat = ChatSession::open_with(opts, None, Store::open_in_memory().unwrap())
        .await
        .unwrap();
    assert_eq!(chat.model(), "llama3.2", "embedding models are skipped");
    assert_eq!(chat.provider_name(), "Ollama");
    let seen = Mutex::new(Vec::new());
    let observer = |e: &tern_core::EventEnvelope| seen.lock().unwrap().push(e.kind.clone());
    let out = chat
        .send("hi", &DenyAll, CancelFlag::default(), &observer)
        .await;
    assert_eq!(out.state, TurnState::Completed);
    assert_eq!(out.answer.as_deref(), Some("Hello from the mock."));
    assert_eq!(chat.usage().total(), 17);
    let ctx = chat.context();
    assert_eq!((ctx.used(), ctx.calls, ctx.estimated), (17, 1, false));
    let kinds = seen.lock().unwrap().clone();
    assert_eq!(kinds, ["turn.started", "model.replied", "turn.finished"]);
    let recent = chat.recent_sessions(5).unwrap();
    assert_eq!(recent.len(), 1);
    assert_eq!(recent[0].title, "hi", "the first message names the chat");
    assert_eq!(recent[0].id, chat.session_id());

    chat.send("again", &DenyAll, CancelFlag::default(), &observer)
        .await;
    let reqs = s.received_requests().await.unwrap();
    let last: serde_json::Value = reqs.last().unwrap().body_json().unwrap();
    let msgs = last["messages"].as_array().unwrap();
    let contents: Vec<_> = msgs
        .iter()
        .filter(|m| m["role"] != "system")
        .map(|m| m["content"].as_str().unwrap())
        .collect();
    assert_eq!(contents, ["hi", "Hello from the mock.", "again"]);
}

#[tokio::test]
async fn no_saved_key_gives_a_plain_next_step() {
    let err = ChatSession::open_with(
        ChatOptions::default(),
        None,
        Store::open_in_memory().unwrap(),
    )
    .await
    .err()
    .unwrap();
    assert!(err.contains("tern provider add"), "{err}");
}

#[tokio::test]
async fn a_remote_provider_without_a_key_says_so() {
    let opts = ChatOptions {
        provider: Some("openai".into()),
        model: Some("gpt-4.1-mini".into()),
        ..Default::default()
    };
    let err = ChatSession::open_with(opts, None, Store::open_in_memory().unwrap())
        .await
        .err()
        .unwrap();
    assert!(err.contains("needs an API key"), "{err}");
}

#[tokio::test]
async fn reopened_chat_brings_back_turns_and_history() {
    let s = server().await;
    let dir = std::env::temp_dir().join(format!("tern-resume-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let db = dir.join("tern.db");
    let opts = || ChatOptions {
        provider: Some("ollama".into()),
        base_url: Some(format!("{}/v1", s.uri())),
        ..Default::default()
    };
    let observer = |_: &tern_core::EventEnvelope| {};
    let mut chat = ChatSession::open_with(opts(), None, Store::open(&db).unwrap())
        .await
        .unwrap();
    let id = chat.session_id();
    chat.send("What is RAG?", &DenyAll, CancelFlag::default(), &observer)
        .await;
    drop(chat);

    let (mut again, turns) = ChatSession::resume_with(opts(), None, Store::open(&db).unwrap(), id)
        .await
        .unwrap();
    assert_eq!(again.session_id(), id);
    assert_eq!(turns.len(), 1);
    let ctx = again.context();
    assert!(ctx.estimated, "no call yet in the reopened window");
    assert_eq!((ctx.prompt, ctx.calls), (8, 0), "32 chars of history");
    assert_eq!(
        turns[0].exchange(),
        Some(("What is RAG?", "Hello from the mock."))
    );
    again
        .send("And GraphRAG?", &DenyAll, CancelFlag::default(), &observer)
        .await;
    let reqs = s.received_requests().await.unwrap();
    let last: serde_json::Value = reqs.last().unwrap().body_json().unwrap();
    let contents: Vec<_> = last["messages"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|m| m["role"] != "system")
        .map(|m| m["content"].as_str().unwrap().to_owned())
        .collect();
    assert_eq!(
        contents,
        ["What is RAG?", "Hello from the mock.", "And GraphRAG?"]
    );
    let recent = again.recent_sessions(5).unwrap();
    assert_eq!(recent.len(), 1, "reopening adds no new chat");
    assert_eq!(recent[0].title, "What is RAG?", "the title stays");

    again.archive(id).unwrap();
    assert!(again.recent_sessions(5).unwrap().is_empty());
    drop(again);
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn typed_titles_are_cleaned() {
    use tern_agent::clean_title;
    assert_eq!(
        clean_title("  Attention \n paper  ").as_deref(),
        Some("Attention paper")
    );
    assert_eq!(clean_title(" \t\n"), None);
    assert_eq!(clean_title(&"a".repeat(300)).unwrap().chars().count(), 120);
}
