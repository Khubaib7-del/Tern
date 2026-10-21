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
    let kinds = seen.lock().unwrap().clone();
    assert_eq!(kinds, ["turn.started", "model.replied", "turn.finished"]);

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
