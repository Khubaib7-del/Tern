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
        search: Some("off".into()),
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
        search: Some("off".into()),
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
    // 32 characters of history and the 97-character system prompt, at
    // four characters a token.
    assert_eq!(tern_agent::protocol::BASE.len(), 97);
    assert_eq!((ctx.prompt, ctx.calls), (33, 0));
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

#[tokio::test]
async fn a_new_chat_is_a_draft_until_its_first_message() {
    let s = server().await;
    let opts = ChatOptions {
        provider: Some("ollama".into()),
        base_url: Some(format!("{}/v1", s.uri())),
        search: Some("off".into()),
        ..Default::default()
    };
    let mut chat = ChatSession::open_with(opts, None, Store::open_in_memory().unwrap())
        .await
        .unwrap();
    assert!(chat.is_draft(), "opening a chat writes nothing");
    assert!(chat.recent_sessions(5).unwrap().is_empty());
    let n = chat
        .with_store(|st| st.all_sessions(10).unwrap().len())
        .unwrap();
    assert_eq!(n, 0, "no empty row in the Library either");
    let observer = |_: &tern_core::EventEnvelope| {};
    chat.send("hi", &DenyAll, CancelFlag::default(), &observer)
        .await;
    assert!(!chat.is_draft());
    let recent = chat.recent_sessions(5).unwrap();
    assert_eq!(recent.len(), 1);
    assert_eq!(recent[0].title, "hi");
    let kinds: Vec<String> = chat
        .with_store(|st| st.replay(chat.session_id(), 0).unwrap())
        .unwrap()
        .into_iter()
        .map(|e| e.kind)
        .collect();
    assert_eq!(kinds[0], "session.opened", "the opening is logged first");
}

#[tokio::test]
async fn changing_the_model_keeps_the_chat_and_its_history() {
    let s = server().await;
    let opts = |model: &str| ChatOptions {
        provider: Some("ollama".into()),
        model: Some(model.into()),
        base_url: Some(format!("{}/v1", s.uri())),
        search: Some("off".into()),
        ..Default::default()
    };
    let mut chat = ChatSession::open_with(opts("llama3.2"), None, Store::open_in_memory().unwrap())
        .await
        .unwrap();
    let id = chat.session_id();
    let observer = |_: &tern_core::EventEnvelope| {};
    chat.send("hi", &DenyAll, CancelFlag::default(), &observer)
        .await;
    for m in ["qwen3", "mistral", "llama3.2-vision"] {
        chat.set_model_with(&opts(m), None).await.unwrap();
        assert_eq!(chat.model(), m);
        assert_eq!(chat.session_id(), id, "same chat");
    }
    chat.send("again", &DenyAll, CancelFlag::default(), &observer)
        .await;
    let reqs = s.received_requests().await.unwrap();
    let last: serde_json::Value = reqs.last().unwrap().body_json().unwrap();
    assert_eq!(
        last["model"], "llama3.2-vision",
        "the next answer comes from the new model"
    );
    let contents: Vec<_> = last["messages"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|m| m["role"] != "system")
        .map(|m| m["content"].as_str().unwrap().to_owned())
        .collect();
    assert_eq!(contents, ["hi", "Hello from the mock.", "again"]);
    assert_eq!(chat.recent_sessions(5).unwrap().len(), 1, "no new chat");
    let changes = chat
        .with_store(|st| st.replay(id, 0).unwrap())
        .unwrap()
        .into_iter()
        .filter(|e| e.kind == "session.model_changed")
        .count();
    assert_eq!(changes, 3);
    assert_eq!(
        chat.max_output_tokens(),
        8192,
        "answers get room by default"
    );
}

#[tokio::test]
async fn a_failed_model_change_keeps_the_old_model() {
    let s = server().await;
    let opts = ChatOptions {
        provider: Some("ollama".into()),
        base_url: Some(format!("{}/v1", s.uri())),
        search: Some("off".into()),
        ..Default::default()
    };
    let mut chat = ChatSession::open_with(opts, None, Store::open_in_memory().unwrap())
        .await
        .unwrap();
    let bad = ChatOptions {
        provider: Some("openai".into()),
        model: Some("gpt-4.1-mini".into()),
        ..Default::default()
    };
    let err = chat.set_model_with(&bad, None).await.unwrap_err();
    assert!(err.contains("needs an API key"), "{err}");
    assert_eq!(chat.model(), "llama3.2");
    assert_eq!(chat.provider_name(), "Ollama");
}
