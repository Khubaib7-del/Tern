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
async fn a_voice_model_cannot_be_the_chat_model() {
    let s = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/v1/models"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "data": [{"id": "whisper-large-v3"}, {"id": "llama3.2"}]
        })))
        .mount(&s)
        .await;
    let opts = ChatOptions {
        provider: Some("ollama".into()),
        model: Some("whisper-large-v3".into()),
        base_url: Some(format!("{}/v1", s.uri())),
        search: Some("off".into()),
        ..Default::default()
    };
    let err = ChatSession::open_with(opts, None, Store::open_in_memory().unwrap())
        .await
        .err()
        .unwrap();
    assert!(
        err.starts_with("whisper-large-v3 only transcribes speech, so it cannot chat"),
        "{err}"
    );
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
    // 32 characters of history and the system prompt (the 97-character
    // opening line, then `file_read` for full paths, which every chat has
    // since #64, and how to call a tool), at four characters a token.
    assert_eq!(tern_agent::protocol::BASE.len(), 97);
    assert_eq!((ctx.prompt, ctx.calls), (170, 0));
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

/// A chat model that turns out not to see: the provider's refusal reads in
/// plain words and says so, and the pictures leave the history so the next
/// request goes without them.
#[tokio::test]
async fn pictures_leave_the_history_of_a_model_that_cannot_see() {
    use wiremock::matchers::body_string_contains;
    let s = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/v1/models"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "data": [{"id": "llama3.2"}]
        })))
        .mount(&s)
        .await;
    // Mistral's refusal, for a request that carries a picture.
    Mock::given(method("POST"))
        .and(path("/v1/chat/completions"))
        .and(body_string_contains("image_url"))
        .respond_with(ResponseTemplate::new(400).set_body_json(json!({
            "object": "error",
            "message": "Image input is not enabled for this model",
            "type": "invalid_request_invalid_args",
            "code": "3051"
        })))
        .with_priority(1)
        .mount(&s)
        .await;
    Mock::given(method("POST"))
        .and(path("/v1/chat/completions"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "choices": [{"message": {"role": "assistant", "content": "ok"}, "finish_reason": "stop"}],
            "usage": {"prompt_tokens": 3, "completion_tokens": 1}
        })))
        .with_priority(5)
        .mount(&s)
        .await;
    let opts = ChatOptions {
        provider: Some("ollama".into()),
        base_url: Some(format!("{}/v1", s.uri())),
        search: Some("off".into()),
        ..Default::default()
    };
    let mut chat = ChatSession::open_with(opts, None, Store::open_in_memory().unwrap())
        .await
        .unwrap();
    let observer = |_: &tern_core::EventEnvelope| {};
    let picture = tern_providers::ImageInput {
        name: "img.jpg".into(),
        media_type: "image/png".into(),
        data: "iVBORw0KGgo=".into(),
    };
    chat.attach_images(vec![picture]);
    assert!(chat.has_attached_images());
    let out = chat
        .send(
            "what does it say?",
            &DenyAll,
            CancelFlag::default(),
            &observer,
        )
        .await;
    assert_eq!(out.state, TurnState::Failed);
    let reason = out.reason.unwrap_or_default();
    assert!(
        tern_agent::media::rejects_images(&reason),
        "not read as a refusal: {reason}"
    );
    let plain = tern_agent::media::plain_error(&reason);
    assert!(
        plain.contains("Image input is not enabled for this model (code 3051)"),
        "{plain}"
    );
    assert!(!plain.contains("{\""), "raw JSON left: {plain}");
    // The refused message never joined the history: nothing to drop.
    assert_eq!(chat.drop_pictures(), 0);
    let out = chat
        .send(
            "what does it say?\n\n[read text]",
            &DenyAll,
            CancelFlag::default(),
            &observer,
        )
        .await;
    assert_eq!(out.state, TurnState::Completed);
}

#[tokio::test]
async fn dropping_pictures_strips_them_from_earlier_messages() {
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
    let observer = |_: &tern_core::EventEnvelope| {};
    chat.attach_images(vec![tern_providers::ImageInput {
        name: "chart.png".into(),
        media_type: "image/png".into(),
        data: "iVBORw0KGgo=".into(),
    }]);
    chat.send("look", &DenyAll, CancelFlag::default(), &observer)
        .await;
    assert_eq!(chat.drop_pictures(), 1);
    assert_eq!(chat.drop_pictures(), 0, "already gone");
    chat.send("again", &DenyAll, CancelFlag::default(), &observer)
        .await;
    let reqs = s.received_requests().await.unwrap();
    let last = String::from_utf8_lossy(&reqs.last().unwrap().body).into_owned();
    assert!(!last.contains("image_url"), "{last}");
    assert!(last.contains("chart.png"), "the note names the picture");
}

fn chart() -> tern_providers::ImageInput {
    tern_providers::ImageInput {
        name: "chart.png".into(),
        media_type: "image/png".into(),
        data: "iVBORw0KGgo=".into(),
    }
}

async fn open_on(s: &MockServer) -> ChatSession {
    let opts = ChatOptions {
        provider: Some("ollama".into()),
        base_url: Some(format!("{}/v1", s.uri())),
        search: Some("off".into()),
        ..Default::default()
    };
    ChatSession::open_with(opts, None, Store::open_in_memory().unwrap())
        .await
        .unwrap()
}

async fn last_body(s: &MockServer) -> String {
    let reqs = s.received_requests().await.unwrap();
    String::from_utf8_lossy(&reqs.last().unwrap().body).into_owned()
}

/// Regression (#63): after an image turn, a text-only turn to a model that
/// cannot see carries no image part, in any message.
#[tokio::test]
async fn a_text_turn_after_an_image_turn_sends_no_picture_to_a_model_that_cannot_see() {
    let s = server().await;
    let mut chat = open_on(&s).await;
    let observer = |_: &tern_core::EventEnvelope| {};
    chat.attach_images(vec![chart()]);
    let out = chat
        .send("look", &DenyAll, CancelFlag::default(), &observer)
        .await;
    assert_eq!(out.state, TurnState::Completed);
    assert!(
        last_body(&s).await.contains("image_url"),
        "a seeing model gets it"
    );
    // The front end learns the model cannot see (the provider's listing).
    chat.set_sees_pictures(Some(false));
    assert!(chat.is_blind());
    let out = chat
        .send(
            "give me 3 words",
            &DenyAll,
            CancelFlag::default(),
            &observer,
        )
        .await;
    assert_eq!(out.state, TurnState::Completed);
    let body = last_body(&s).await;
    assert!(!body.contains("image_url"), "{body}");
    assert!(!body.contains("iVBORw0KGgo"), "no picture data: {body}");
    assert!(body.contains("[image: chart.png"), "{body}");
    // A picture attached anyway goes as a note, never as an image part.
    chat.attach_images(vec![chart()]);
    chat.send("and this?", &DenyAll, CancelFlag::default(), &observer)
        .await;
    assert!(!last_body(&s).await.contains("image_url"));
}

/// Regression (#63): the provider refuses a text-only turn because an
/// earlier message still carries a picture (Mistral 3051). The chat
/// remembers the model cannot see, drops the pictures and runs the turn
/// again, so the turn completes and no later request has one.
#[tokio::test]
async fn a_refusal_caused_by_earlier_pictures_reruns_the_turn_without_them() {
    use wiremock::matchers::body_string_contains;
    let s = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/v1/models"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "data": [{"id": "blind-text-2508"}]
        })))
        .mount(&s)
        .await;
    // The first picture gets through (as when the chat began on a model
    // that sees, or before the provider changed its mind) ...
    Mock::given(method("POST"))
        .and(path("/v1/chat/completions"))
        .and(body_string_contains("image_url"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "choices": [{"message": {"role": "assistant", "content": "a chart"}, "finish_reason": "stop"}],
            "usage": {"prompt_tokens": 3, "completion_tokens": 1}
        })))
        .up_to_n_times(1)
        .with_priority(1)
        .mount(&s)
        .await;
    // ... then every request with a picture is refused.
    Mock::given(method("POST"))
        .and(path("/v1/chat/completions"))
        .and(body_string_contains("image_url"))
        .respond_with(ResponseTemplate::new(400).set_body_json(json!({
            "object": "error",
            "message": "Image input is not enabled for this model",
            "type": "invalid_request_invalid_args",
            "code": "3051"
        })))
        .with_priority(2)
        .mount(&s)
        .await;
    Mock::given(method("POST"))
        .and(path("/v1/chat/completions"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "choices": [{"message": {"role": "assistant", "content": "ok"}, "finish_reason": "stop"}],
            "usage": {"prompt_tokens": 3, "completion_tokens": 1}
        })))
        .with_priority(5)
        .mount(&s)
        .await;
    let mut chat = open_on(&s).await;
    assert_eq!(chat.model(), "blind-text-2508");
    let observer = |_: &tern_core::EventEnvelope| {};
    chat.attach_images(vec![chart()]);
    let out = chat
        .send("look", &DenyAll, CancelFlag::default(), &observer)
        .await;
    assert_eq!(out.state, TurnState::Completed);
    let out = chat
        .send(
            "give me 3 words",
            &DenyAll,
            CancelFlag::default(),
            &observer,
        )
        .await;
    assert_eq!(out.state, TurnState::Completed, "{:?}", out.reason);
    assert_eq!(out.answer.as_deref(), Some("ok"));
    let body = last_body(&s).await;
    assert!(!body.contains("image_url"), "{body}");
    assert!(chat.is_blind(), "remembered as not seeing");
    assert_eq!(chat.drop_pictures(), 0, "no picture left anywhere");
    // A new chat with the same model never sends one either.
    let mut fresh = open_on(&s).await;
    assert!(fresh.is_blind());
    fresh.attach_images(vec![chart()]);
    let out = fresh
        .send("hello", &DenyAll, CancelFlag::default(), &observer)
        .await;
    assert_eq!(out.state, TurnState::Completed);
    assert!(!last_body(&s).await.contains("image_url"));
}

#[test]
fn strip_pictures_leaves_their_names() {
    let mut msgs = vec![
        tern_providers::ChatMessage::user("see").with_images(vec![chart()]),
        tern_providers::ChatMessage::assistant("ok"),
    ];
    assert_eq!(tern_agent::chat::strip_pictures(&mut msgs), 1);
    assert!(msgs[0].images.is_empty());
    assert!(msgs[0].content.contains("[image: chart.png"));
    assert_eq!(tern_agent::chat::strip_pictures(&mut msgs), 0);
}

/// Replies to chat completions with each scripted text in turn.
struct Replies(Mutex<std::collections::VecDeque<String>>);

impl wiremock::Respond for Replies {
    fn respond(&self, _: &wiremock::Request) -> ResponseTemplate {
        let text = self
            .0
            .lock()
            .unwrap()
            .pop_front()
            .unwrap_or_else(|| "out of script".into());
        ResponseTemplate::new(200).set_body_json(json!({
            "choices": [{"message": {"role": "assistant", "content": text}, "finish_reason": "stop"}],
            "usage": {"prompt_tokens": 12, "completion_tokens": 5}
        }))
    }
}

/// A mock provider plus a SearXNG-style search on the same server.
async fn web_server(replies: &[&str]) -> MockServer {
    let s = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/v1/models"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "data": [{"id": "llama3.2"}]
        })))
        .mount(&s)
        .await;
    Mock::given(method("GET"))
        .and(path("/search"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({"results": [
            {"title": "LangChain - Wikipedia", "url": "https://en.wikipedia.org/wiki/LangChain",
             "content": "LangChain was launched in October 2022 by Harrison Chase."}
        ]})))
        .mount(&s)
        .await;
    Mock::given(method("POST"))
        .and(path("/v1/chat/completions"))
        .respond_with(Replies(Mutex::new(
            replies.iter().map(|r| r.to_string()).collect(),
        )))
        .mount(&s)
        .await;
    s
}

async fn open_with_search(s: &MockServer) -> ChatSession {
    let opts = ChatOptions {
        provider: Some("ollama".into()),
        base_url: Some(format!("{}/v1", s.uri())),
        search: Some("searxng".into()),
        search_url: Some(s.uri()),
        no_mcp: true,
        ..Default::default()
    };
    let mut chat = ChatSession::open_with(opts, None, Store::open_in_memory().unwrap())
        .await
        .unwrap();
    // As the desktop does before every message in Chat mode.
    chat.set_turn_mode(tern_agent::workspace::TurnMode::Chat);
    chat.set_sees_pictures(Some(false));
    chat.drop_pictures();
    chat
}

/// Regression (#64): a Chat-mode turn that asks for a web search runs the
/// web tool, and the answer written from its result comes back. Since the
/// prefetch, "use the web" runs the search before the first model call, so
/// the turn makes one model call, which already sees the result.
#[tokio::test]
async fn a_chat_turn_asking_for_the_web_runs_web_search_and_answers() {
    let s = web_server(&["Harrison Chase created LangChain [1]."]).await;
    let mut chat = open_with_search(&s).await;
    let seen = Mutex::new(Vec::new());
    let observer = |e: &tern_core::EventEnvelope| seen.lock().unwrap().push(e.kind.clone());
    let out = chat
        .send(
            "who is the creator of LangChain? use the web tool",
            &DenyAll,
            CancelFlag::default(),
            &observer,
        )
        .await;
    assert_eq!(out.state, TurnState::Completed, "{:?}", out.reason);
    assert_eq!(
        out.answer.as_deref(),
        Some("Harrison Chase created LangChain [1].")
    );
    assert_eq!(out.tool_calls, 1);
    let kinds = seen.lock().unwrap().clone();
    assert!(kinds.iter().any(|k| k == "tool.executed"), "{kinds:?}");
    // The model was offered the web tool, and saw the search result.
    let reqs = s.received_requests().await.unwrap();
    let chats: Vec<String> = reqs
        .iter()
        .filter(|r| r.url.path() == "/v1/chat/completions")
        .map(|r| String::from_utf8_lossy(&r.body).into_owned())
        .collect();
    assert_eq!(chats.len(), 1, "one model call: the search ran first");
    assert!(chats[0].contains("web_search"), "{}", chats[0]);
    assert!(chats[0].contains("Harrison Chase"), "{}", chats[0]);
    assert_eq!(out.model_calls, 1);
}

/// Regression (#64): consecutive turns each get their own new answer,
/// never the one before.
#[tokio::test]
async fn consecutive_turns_return_new_answers() {
    let s = web_server(&["Paris.", "Islamabad.", "Canberra."]).await;
    let mut chat = open_with_search(&s).await;
    let observer = |_: &tern_core::EventEnvelope| {};
    let mut answers = Vec::new();
    for q in [
        "capital of France?",
        "capital of Pakistan?",
        "capital of Australia?",
    ] {
        chat.set_sees_pictures(Some(false));
        chat.drop_pictures();
        let out = chat
            .send(q, &DenyAll, CancelFlag::default(), &observer)
            .await;
        assert_eq!(out.state, TurnState::Completed, "{:?}", out.reason);
        answers.push(out.answer.unwrap());
    }
    assert_eq!(answers, ["Paris.", "Islamabad.", "Canberra."]);
    let body = last_body(&s).await;
    let v: serde_json::Value = serde_json::from_str(&body).unwrap();
    let msgs: Vec<_> = v["messages"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|m| m["role"] != "system")
        .map(|m| m["content"].as_str().unwrap_or_default().to_owned())
        .collect();
    assert_eq!(
        msgs,
        [
            "capital of France?",
            "Paris.",
            "capital of Pakistan?",
            "Islamabad.",
            "capital of Australia?"
        ]
    );
}

/// Regression (#64): an empty reply is asked for once more, and an empty
/// answer never joins the history, so the next turn is not refused for an
/// empty assistant message (Mistral, Anthropic and Gemini refuse one).
#[tokio::test]
async fn an_empty_reply_never_poisons_the_next_turns() {
    let s = web_server(&["", "Harrison Chase.", "", "", "Islamabad."]).await;
    let mut chat = open_with_search(&s).await;
    let observer = |_: &tern_core::EventEnvelope| {};
    // The first empty reply is retried and the second reply answers.
    let out = chat
        .send(
            "who created LangChain?",
            &DenyAll,
            CancelFlag::default(),
            &observer,
        )
        .await;
    assert_eq!(out.state, TurnState::Completed, "{:?}", out.reason);
    assert_eq!(out.answer.as_deref(), Some("Harrison Chase."));
    assert_eq!(out.model_calls, 2);
    // Two empty replies: the turn ends with a gap and leaves no message.
    let out = chat
        .send(
            "and LlamaIndex?",
            &DenyAll,
            CancelFlag::default(),
            &observer,
        )
        .await;
    assert_eq!(out.state, TurnState::CompletedWithGaps);
    // The next turn carries no empty assistant message and gets its answer.
    let out = chat
        .send(
            "capital of Pakistan?",
            &DenyAll,
            CancelFlag::default(),
            &observer,
        )
        .await;
    assert_eq!(out.answer.as_deref(), Some("Islamabad."));
    let v: serde_json::Value = serde_json::from_str(&last_body(&s).await).unwrap();
    let msgs: Vec<_> = v["messages"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|m| m["role"] != "system")
        .map(|m| m["content"].as_str().unwrap_or_default().to_owned())
        .collect();
    assert_eq!(
        msgs,
        [
            "who created LangChain?",
            "Harrison Chase.",
            "capital of Pakistan?"
        ]
    );
}

#[test]
fn tidy_history_drops_empty_exchanges() {
    use tern_providers::ChatMessage;
    let mut h = vec![
        ChatMessage::user("a"),
        ChatMessage::assistant("A"),
        ChatMessage::user("b"),
        ChatMessage::assistant("  "),
        ChatMessage::user("c"),
    ];
    assert_eq!(tern_agent::chat::tidy_history(&mut h), 2);
    let c: Vec<_> = h.iter().map(|m| m.content.as_str()).collect();
    assert_eq!(c, ["a", "A", "c"]);
}

/// Approves every request and keeps what it was asked.
#[derive(Default)]
struct Approves(Mutex<Vec<tern_agent::ApprovalRequest>>);

impl tern_agent::Approver for Approves {
    fn ask<'a>(
        &'a self,
        req: &'a tern_agent::ApprovalRequest,
    ) -> tern_agent::tools::BoxFuture<'a, tern_agent::ApprovalAnswer> {
        self.0.lock().unwrap().push(req.clone());
        Box::pin(async { tern_agent::ApprovalAnswer::AllowOnce })
    }
}

/// Regression (#64): "read <full path>" with no workspace open reads the
/// file after the user approves it; quoted paths with spaces work. The path
/// is read before the first model call, so the turn makes one model call.
#[tokio::test]
async fn a_typed_full_path_is_read_after_approval() {
    let d = tempfile::tempdir().unwrap();
    let f = d.path().join("My Notes.txt");
    std::fs::write(&f, "The meeting is on Tuesday.").unwrap();
    let quoted = format!("\"{}\"", f.display());
    let call = format!(
        "```tool\n{}\n```",
        json!({"tool": "file_read", "args": {"path": quoted}})
    );
    let s = web_server(&["It says the meeting is on Tuesday."]).await;
    let mut chat = open_with_search(&s).await;
    let observer = |_: &tern_core::EventEnvelope| {};
    let approver = Approves::default();
    let out = chat
        .send(
            &format!("read {quoted}"),
            &approver,
            CancelFlag::default(),
            &observer,
        )
        .await;
    assert_eq!(out.state, TurnState::Completed, "{:?}", out.reason);
    assert_eq!(out.tool_calls, 1);
    let asked = approver.0.lock().unwrap().clone();
    assert_eq!(asked.len(), 1, "a file outside any workspace is approved");
    assert_eq!(asked[0].tool, "file_read");
    assert!(
        asked[0].summary.contains("My Notes.txt"),
        "{}",
        asked[0].summary
    );
    let reqs = s.received_requests().await.unwrap();
    let chats: Vec<String> = reqs
        .iter()
        .filter(|r| r.url.path() == "/v1/chat/completions")
        .map(|r| String::from_utf8_lossy(&r.body).into_owned())
        .collect();
    assert_eq!(chats.len(), 1, "one model call: the file was read first");
    assert_eq!(out.model_calls, 1);
    assert_eq!(
        out.answer.as_deref(),
        Some("It says the meeting is on Tuesday.")
    );
    assert!(chats[0].contains("file_read"), "offered with no workspace");
    assert!(
        chats[0].contains("The meeting is on Tuesday."),
        "{}",
        chats[0]
    );

    // Declined, the file is not read and the turn says so.
    let s = web_server(&[&call]).await;
    let mut chat = open_with_search(&s).await;
    let out = chat
        .send("read it", &DenyAll, CancelFlag::default(), &observer)
        .await;
    assert_eq!(out.state, TurnState::Blocked);
}
