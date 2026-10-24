//! End-to-end adapter tests against a local mock server.

use tern_core::SecretString;
use tern_providers::{ChatMessage, ChatRequest, Provider, ProviderError, ProviderSpec, Wire, find};
use wiremock::matchers::{body_partial_json, header, method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

const KEY: &str = "sk-test-0123456789abcdefghijklmn";

fn spec(id: &str) -> &'static ProviderSpec {
    find(id).unwrap()
}

fn key() -> Option<SecretString> {
    Some(SecretString::new(KEY))
}

#[tokio::test]
async fn openai_compatible_lists_models_with_bearer_auth() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/v1/models"))
        .and(header("authorization", format!("Bearer {KEY}").as_str()))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "data": [{"id": "gpt-4.1-mini"}, {"id": "text-embedding-3-small"}, {"id": "gpt-4.1"}]
        })))
        .mount(&server)
        .await;
    let p = Provider::new(spec("openai"), key(), Some(&format!("{}/v1", server.uri()))).unwrap();
    let models = p.list_models().await.unwrap();
    let ids: Vec<_> = models.iter().map(|m| m.id.as_str()).collect();
    assert_eq!(ids, ["gpt-4.1", "gpt-4.1-mini", "text-embedding-3-small"]);
    let report = p.probe().await.unwrap();
    assert_eq!((report.models, report.chat_models), (3, 2));
}

#[tokio::test]
async fn anthropic_chat_sends_version_and_key_headers() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/v1/messages"))
        .and(header("x-api-key", KEY))
        .and(header("anthropic-version", "2023-06-01"))
        .and(body_partial_json(serde_json::json!({"model": "claude-sonnet-4-5", "system": "cite"})))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "content": [{"type": "text", "text": "Done."}], "usage": {"input_tokens": 5, "output_tokens": 1}
        })))
        .mount(&server)
        .await;
    let p = Provider::new(
        spec("anthropic"),
        key(),
        Some(&format!("{}/v1", server.uri())),
    )
    .unwrap();
    let req =
        ChatRequest::new("claude-sonnet-4-5", vec![ChatMessage::user("hi")]).with_system("cite");
    let r = p.chat(&req).await.unwrap();
    assert_eq!(r.text, "Done.");
    assert_eq!(r.usage.total(), 6);
}

#[tokio::test]
async fn gemini_chat_uses_header_key_not_query() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/v1beta/models/gemini-2.5-flash:generateContent"))
        .and(header("x-goog-api-key", KEY))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "candidates": [{"content": {"parts": [{"text": "ok"}]}}]
        })))
        .mount(&server)
        .await;
    let p = Provider::new(
        spec("google"),
        key(),
        Some(&format!("{}/v1beta", server.uri())),
    )
    .unwrap();
    let r = p
        .chat(&ChatRequest::new(
            "models/gemini-2.5-flash",
            vec![ChatMessage::user("hi")],
        ))
        .await
        .unwrap();
    assert_eq!(r.text, "ok");
    let reqs = server.received_requests().await.unwrap();
    assert!(
        reqs.iter().all(|r| !r.url.as_str().contains(KEY)),
        "key leaked into a URL"
    );
}

#[tokio::test]
async fn refused_key_and_rate_limit_are_plain_errors() {
    let server = MockServer::start().await;
    Mock::given(path("/v1/models"))
        .respond_with(
            ResponseTemplate::new(401).set_body_json(serde_json::json!({
                "error": {"message": "Invalid API Key", "type": "invalid_request_error", "code": "invalid_api_key"}
            })),
        )
        .mount(&server)
        .await;
    Mock::given(path("/v1/chat/completions"))
        .respond_with(ResponseTemplate::new(429).insert_header("retry-after", "7"))
        .mount(&server)
        .await;
    let p = Provider::new(spec("groq"), key(), Some(&format!("{}/v1", server.uri()))).unwrap();
    let e = p.list_models().await.unwrap_err();
    assert_eq!(
        e,
        ProviderError::Auth {
            provider: "Groq",
            status: 401,
            detail: "Invalid API Key".into(),
        }
    );
    assert!(e.to_string().contains("refused the key"));
    assert!(
        e.to_string()
            .ends_with("The provider said: Invalid API Key")
    );
    let e = p
        .chat(&ChatRequest::new("m", vec![ChatMessage::user("x")]))
        .await
        .unwrap_err();
    assert_eq!(
        e,
        ProviderError::RateLimited {
            provider: "Groq",
            retry_after: Some(7),
            detail: String::new(),
        }
    );
}

#[tokio::test]
async fn error_bodies_are_redacted() {
    let server = MockServer::start().await;
    Mock::given(path("/v1/models"))
        .respond_with(
            ResponseTemplate::new(500).set_body_string(format!("internal error for key {KEY}")),
        )
        .mount(&server)
        .await;
    let p = Provider::new(spec("openai"), key(), Some(&format!("{}/v1", server.uri()))).unwrap();
    let e = p.list_models().await.unwrap_err().to_string();
    assert!(!e.contains(KEY), "{e}");
    assert!(e.contains("HTTP 500"));
}

#[tokio::test]
async fn local_server_needs_no_key_and_unreachable_is_explained() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/v1/models"))
        .respond_with(
            ResponseTemplate::new(200)
                .set_body_json(serde_json::json!({"data": [{"id": "llama3.2"}]})),
        )
        .mount(&server)
        .await;
    let p = Provider::new(spec("ollama"), None, Some(&format!("{}/v1", server.uri()))).unwrap();
    assert_eq!(p.list_models().await.unwrap()[0].id, "llama3.2");
    let reqs = server.received_requests().await.unwrap();
    assert!(reqs[0].headers.get("authorization").is_none());

    let dead = Provider::new(spec("lmstudio"), None, Some("http://127.0.0.1:9")).unwrap();
    let e = dead.list_models().await.unwrap_err();
    assert!(matches!(e, ProviderError::Network { .. }), "{e:?}");
}

#[test]
fn missing_key_and_bad_urls_are_refused_up_front() {
    let e = Provider::new(spec("openai"), None, None).unwrap_err();
    assert!(e.to_string().contains("tern provider add openai"));
    assert!(Provider::new(spec("openai"), Some(SecretString::new("")), None).is_err());
    for bad in ["ftp://x", "not a url", "file:///etc/passwd"] {
        assert!(
            matches!(
                Provider::new(spec("ollama"), None, Some(bad)),
                Err(ProviderError::BadUrl(_))
            ),
            "{bad}"
        );
    }
    assert_eq!(spec("anthropic").wire, Wire::Anthropic);
}

#[tokio::test]
async fn openai_stream_delivers_text_as_it_arrives() {
    let server = MockServer::start().await;
    let sse = concat!(
        "data: {\"choices\":[{\"delta\":{\"content\":\"Hel\"}}]}\n\n",
        "data: {\"choices\":[{\"delta\":{\"content\":\"lo.\"},\"finish_reason\":\"stop\"}]}\n\n",
        "data: {\"choices\":[],\"usage\":{\"prompt_tokens\":7,\"completion_tokens\":2}}\n\n",
        "data: [DONE]\n\n"
    );
    Mock::given(method("POST"))
        .and(path("/v1/chat/completions"))
        .and(body_partial_json(
            serde_json::json!({"stream": true, "stream_options": {"include_usage": true}}),
        ))
        .respond_with(ResponseTemplate::new(200).set_body_raw(sse, "text/event-stream"))
        .mount(&server)
        .await;
    let p = Provider::new(spec("openai"), key(), Some(&format!("{}/v1", server.uri()))).unwrap();
    let req = ChatRequest::new("gpt-4.1-mini", vec![ChatMessage::user("hi")]);
    let got = std::sync::Mutex::new(Vec::<String>::new());
    let r = p
        .chat_stream(&req, &|t| {
            got.lock().unwrap().push(t.to_owned());
            true
        })
        .await
        .unwrap();
    assert_eq!(got.into_inner().unwrap(), ["Hel", "lo."]);
    assert_eq!(r.text, "Hello.");
    assert_eq!(r.usage, tern_providers::Usage::measured(7, 2));

    // Returning false stops the read and marks the reply cancelled.
    let r = p.chat_stream(&req, &|_| false).await.unwrap();
    assert_eq!(r.text, "Hel");
    assert_eq!(r.finish_reason.as_deref(), Some("cancelled"));
}

#[tokio::test]
async fn stream_errors_use_the_normal_status_mapping() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/v1/chat/completions"))
        .respond_with(ResponseTemplate::new(401))
        .mount(&server)
        .await;
    let p = Provider::new(spec("groq"), key(), Some(&format!("{}/v1", server.uri()))).unwrap();
    let req = ChatRequest::new("m", vec![ChatMessage::user("hi")]);
    let e = p.chat_stream(&req, &|_| true).await.unwrap_err();
    assert!(
        matches!(e, ProviderError::Auth { status: 401, .. }),
        "{e:?}"
    );
}
