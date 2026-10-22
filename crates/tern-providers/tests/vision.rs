//! Pictures reach each wire format in its own shape.

use tern_core::SecretString;
use tern_providers::{ChatMessage, ChatRequest, ImageInput, Provider, find};
use wiremock::matchers::{body_partial_json, method};
use wiremock::{Mock, MockServer, ResponseTemplate};

fn img() -> ImageInput {
    ImageInput {
        name: "chart.png".into(),
        media_type: "image/png".into(),
        data: "iVBORw0KGgo=".into(),
    }
}

fn req() -> ChatRequest {
    ChatRequest::new(
        "m",
        vec![ChatMessage::user("What does the chart show?").with_images(vec![img()])],
    )
}

#[test]
fn messages_without_pictures_serialize_as_before() {
    let v = serde_json::to_value(ChatMessage::user("hi")).unwrap();
    assert_eq!(v, serde_json::json!({"role": "user", "content": "hi"}));
    let back: ChatMessage = serde_json::from_value(v).unwrap();
    assert!(back.images.is_empty());
    assert_eq!(img().data_url(), "data:image/png;base64,iVBORw0KGgo=");
}

#[tokio::test]
async fn openai_sends_image_url_parts() {
    let s = MockServer::start().await;
    Mock::given(method("POST"))
        .and(body_partial_json(serde_json::json!({"messages": [{"role": "user", "content": [
            {"type": "text", "text": "What does the chart show?"},
            {"type": "image_url", "image_url": {"url": "data:image/png;base64,iVBORw0KGgo="}}
        ]}]})))
        .respond_with(ResponseTemplate::new(200).set_body_json(
            serde_json::json!({"choices": [{"message": {"content": "A rising line."}, "finish_reason": "stop"}]}),
        ))
        .expect(1)
        .mount(&s)
        .await;
    let p = Provider::new(
        find("openai").unwrap(),
        Some(SecretString::new("sk-test-0123456789abcdefghijklmn")),
        Some(&s.uri()),
    )
    .unwrap();
    assert_eq!(p.chat(&req()).await.unwrap().text, "A rising line.");
}

#[tokio::test]
async fn anthropic_sends_base64_image_blocks_first() {
    let s = MockServer::start().await;
    Mock::given(method("POST"))
        .and(body_partial_json(serde_json::json!({"messages": [{"role": "user", "content": [
            {"type": "image", "source": {"type": "base64", "media_type": "image/png", "data": "iVBORw0KGgo="}},
            {"type": "text", "text": "What does the chart show?"}
        ]}]})))
        .respond_with(ResponseTemplate::new(200).set_body_json(
            serde_json::json!({"content": [{"type": "text", "text": "A bar chart."}], "stop_reason": "end_turn"}),
        ))
        .expect(1)
        .mount(&s)
        .await;
    let p = Provider::new(
        find("anthropic").unwrap(),
        Some(SecretString::new("sk-test-0123456789abcdefghijklmn")),
        Some(&s.uri()),
    )
    .unwrap();
    assert_eq!(p.chat(&req()).await.unwrap().text, "A bar chart.");
}

#[tokio::test]
async fn gemini_sends_inline_data() {
    let s = MockServer::start().await;
    Mock::given(method("POST"))
        .and(body_partial_json(serde_json::json!({"contents": [{"role": "user", "parts": [
            {"text": "What does the chart show?"},
            {"inline_data": {"mime_type": "image/png", "data": "iVBORw0KGgo="}}
        ]}]})))
        .respond_with(ResponseTemplate::new(200).set_body_json(
            serde_json::json!({"candidates": [{"content": {"parts": [{"text": "A pie."}]}, "finishReason": "STOP"}]}),
        ))
        .expect(1)
        .mount(&s)
        .await;
    let p = Provider::new(
        find("google").unwrap(),
        Some(SecretString::new("sk-test-0123456789abcdefghijklmn")),
        Some(&s.uri()),
    )
    .unwrap();
    assert_eq!(p.chat(&req()).await.unwrap().text, "A pie.");
}
