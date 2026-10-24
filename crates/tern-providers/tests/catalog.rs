//! The full model catalogue: every model a key can use, every page, with
//! its capabilities. Fixtures are in the providers' documented shapes, with
//! the model ids their model pages listed on 2026-10-24 (see
//! `docs/models.md`).

use std::collections::BTreeMap;

use tern_core::SecretString;
use tern_providers::{Capability, Category, Lifecycle, ModelInfo, Provider, find};
use wiremock::matchers::{body_string_contains, header, method, path, query_param};
use wiremock::{Mock, MockServer, ResponseTemplate};

fn key() -> Option<SecretString> {
    Some(SecretString::new("sk-test-0123456789abcdefghij"))
}

fn fixture(name: &str) -> serde_json::Value {
    let p = format!("{}/tests/fixtures/{name}", env!("CARGO_MANIFEST_DIR"));
    serde_json::from_str(&std::fs::read_to_string(p).unwrap()).unwrap()
}

async fn listed(provider: &str, base_path: &str, body: serde_json::Value) -> Vec<ModelInfo> {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path(format!("{base_path}/models")))
        .respond_with(ResponseTemplate::new(200).set_body_json(body))
        .mount(&server)
        .await;
    let base = format!("{}{base_path}", server.uri());
    Provider::new(find(provider).unwrap(), key(), Some(&base))
        .unwrap()
        .list_models()
        .await
        .unwrap()
}

fn by_category(models: &[ModelInfo]) -> BTreeMap<Category, Vec<&str>> {
    let mut out: BTreeMap<Category, Vec<&str>> = BTreeMap::new();
    for m in models.iter().filter(|m| !m.is_retired()) {
        out.entry(m.category()).or_default().push(m.id.as_str());
    }
    out
}

#[tokio::test]
async fn groq_lists_every_model_in_its_category() {
    let models = listed("groq", "/openai/v1", fixture("groq_models.json")).await;
    assert_eq!(models.len(), 15, "nothing filtered out");
    let cats = by_category(&models);
    assert_eq!(
        cats[&Category::Speech],
        [
            "canopylabs/orpheus-arabic-saudi",
            "canopylabs/orpheus-v1-english",
            "whisper-large-v3",
            "whisper-large-v3-turbo"
        ]
    );
    assert_eq!(
        cats[&Category::Safety],
        [
            "meta-llama/llama-prompt-guard-2-22m",
            "meta-llama/llama-prompt-guard-2-86m",
            "openai/gpt-oss-safeguard-20b"
        ]
    );
    assert_eq!(
        cats[&Category::Vision],
        ["meta-models/muse-spark-open", "qwen/qwen3.8-27b"]
    );
    assert_eq!(cats[&Category::Chat].len(), 5);
    let whisper = models.iter().find(|m| m.id == "whisper-large-v3").unwrap();
    assert_eq!(whisper.capabilities, [Capability::SpeechToText]);
    assert!(whisper.not_chat_reason().unwrap().contains("mic"));
    // `active: false` is kept, marked retired.
    let gone = models.iter().find(|m| m.id == "gemma2-9b-it").unwrap();
    assert_eq!(gone.lifecycle, Lifecycle::Retired);
    let gpt = models
        .iter()
        .find(|m| m.id == "openai/gpt-oss-120b")
        .unwrap();
    assert!(gpt.has(Capability::Reasoning) && gpt.has(Capability::Tools));
    assert_eq!(gpt.context_window, Some(131_072));
}

#[tokio::test]
async fn mistral_uses_its_capability_flags_and_folds_aliases() {
    let models = listed("mistral", "/v1", fixture("mistral_models.json")).await;
    // 27 models; the 15 alias cards fold into their dated models.
    assert_eq!(models.len(), 27);
    let get = |id: &str| models.iter().find(|m| m.id == id).unwrap();
    let small = get("mistral-small-2603");
    assert_eq!(small.label(), "Mistral Small 4");
    assert_eq!(small.aliases, ["mistral-small-latest"]);
    assert!(small.sees_images() && small.has(Capability::Reasoning));
    assert!(small.matches("small 4"));
    let large4 = get("mistral-large-4");
    assert_eq!(large4.aliases, ["mistral-large-latest"]);
    assert_eq!(large4.context_window, Some(1_000_000));
    assert_eq!(get("ministral-8b-2512").label(), "Ministral 3 8B");
    let cats = by_category(&models);
    assert_eq!(
        cats[&Category::Ocr],
        ["mistral-ocr-2512", "mistral-ocr-4-1"]
    );
    assert_eq!(cats[&Category::Speech].len(), 7);
    assert_eq!(
        cats[&Category::Embeddings],
        ["codestral-embed-2505", "mistral-embed-2312"]
    );
    assert_eq!(cats[&Category::Safety], ["mistral-moderation-2603"]);
    assert_eq!(cats[&Category::Vision].len(), 8);
    assert_eq!(
        cats[&Category::Chat],
        ["codestral-2508", "voxtral-small-2507", "zai-glm-5-3"]
    );
    assert_eq!(
        get("voxtral-mini-2602").capabilities,
        [Capability::SpeechToText]
    );
    assert_eq!(
        get("voxtral-mini-tts-2603").capabilities,
        [Capability::TextToSpeech]
    );
    assert_eq!(get("mistral-ocr-4-1").aliases, ["mistral-ocr-latest"]);
    // Deprecated models stay listed, marked.
    let retired: Vec<_> = models
        .iter()
        .filter(|m| m.is_retired())
        .map(|m| m.id.as_str())
        .collect();
    assert_eq!(
        retired,
        [
            "mistral-medium-2508",
            "mistral-ocr-2505",
            "mistral-small-2506",
            "pixtral-large-2411"
        ]
    );
}

#[tokio::test]
async fn anthropic_reads_every_page() {
    let server = MockServer::start().await;
    let card = |id: &str, img: bool| {
        serde_json::json!({"id": id, "type": "model", "display_name": id, "lifecycle": "active",
            "max_input_tokens": 200000, "max_tokens": 64000,
            "capabilities": {"image_input": {"supported": img}, "thinking": {"supported": true}}})
    };
    Mock::given(method("GET"))
        .and(path("/v1/models"))
        .and(query_param("after_id", "claude-b"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "data": [card("claude-c", false)], "has_more": false, "first_id": "claude-c", "last_id": "claude-c"
        })))
        .with_priority(1)
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path("/v1/models"))
        .and(query_param("limit", "1000"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "data": [card("claude-a", true), card("claude-b", true)], "has_more": true, "first_id": "claude-a", "last_id": "claude-b"
        })))
        .with_priority(2)
        .mount(&server)
        .await;
    let models = Provider::new(
        find("anthropic").unwrap(),
        key(),
        Some(&format!("{}/v1", server.uri())),
    )
    .unwrap()
    .list_models()
    .await
    .unwrap();
    let ids: Vec<_> = models.iter().map(|m| m.id.as_str()).collect();
    assert_eq!(ids, ["claude-a", "claude-b", "claude-c"]);
    assert!(models[0].sees_images() && !models[2].sees_images());
    assert_eq!(models[2].context_window, Some(200_000));
}

#[tokio::test]
async fn gemini_follows_the_page_token() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/v1beta/models"))
        .and(query_param("pageToken", "p2"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({"models": [
            {"name": "models/gemini-embedding-001", "supportedGenerationMethods": ["embedContent"]},
            {"name": "models/imagen-4.0-generate-001", "supportedGenerationMethods": ["predict"]}
        ]})))
        .with_priority(1)
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path("/v1beta/models"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({"models": [
            {"name": "models/gemini-3.8-flash", "inputTokenLimit": 1048576, "thinking": true, "supportedGenerationMethods": ["generateContent", "countTokens"]},
            {"name": "models/gemini-2.5-flash-preview-tts", "supportedGenerationMethods": ["generateContent"]}
        ], "nextPageToken": "p2"})))
        .with_priority(2)
        .mount(&server)
        .await;
    let models = Provider::new(
        find("google").unwrap(),
        key(),
        Some(&format!("{}/v1beta", server.uri())),
    )
    .unwrap()
    .list_models()
    .await
    .unwrap();
    let cats: Vec<_> = models
        .iter()
        .map(|m| (m.id.as_str(), m.category()))
        .collect();
    assert_eq!(
        cats,
        [
            ("gemini-2.5-flash-preview-tts", Category::Speech),
            ("gemini-3.8-flash", Category::Vision),
            ("gemini-embedding-001", Category::Embeddings),
            ("imagen-4.0-generate-001", Category::Image),
        ]
    );
}

#[tokio::test]
async fn openrouter_asks_for_every_modality() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/api/v1/models"))
        .and(query_param("output_modalities", "all"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({"data": [
            {"id": "openai/gpt-5", "name": "OpenAI: GPT-5", "context_length": 400000,
             "architecture": {"input_modalities": ["text", "image", "file"], "output_modalities": ["text"]},
             "supported_parameters": ["tools", "reasoning", "max_tokens"]},
            {"id": "openai/gpt-4o-transcribe", "architecture": {"input_modalities": ["audio"], "output_modalities": ["transcription"]}},
            {"id": "google/gemini-2.5-flash-image", "architecture": {"input_modalities": ["text", "image"], "output_modalities": ["text", "image"]}},
            {"id": "openai/text-embedding-3-large", "architecture": {"input_modalities": ["text"], "output_modalities": ["embeddings"]}},
            {"id": "cohere/rerank-v3.5", "architecture": {"input_modalities": ["text"], "output_modalities": ["rerank"]}}
        ], "total_count": 5})))
        .mount(&server)
        .await;
    let models = Provider::new(
        find("openrouter").unwrap(),
        key(),
        Some(&format!("{}/api/v1", server.uri())),
    )
    .unwrap()
    .list_models()
    .await
    .unwrap();
    assert_eq!(models.len(), 5);
    let get = |id: &str| models.iter().find(|m| m.id == id).unwrap();
    let gpt = get("openai/gpt-5");
    assert_eq!(
        gpt.capabilities,
        [
            Capability::Chat,
            Capability::Vision,
            Capability::Tools,
            Capability::Reasoning
        ]
    );
    assert_eq!(gpt.label(), "OpenAI: GPT-5");
    assert_eq!(get("openai/gpt-4o-transcribe").category(), Category::Speech);
    assert_eq!(
        get("google/gemini-2.5-flash-image").category(),
        Category::Image
    );
    assert_eq!(
        get("openai/text-embedding-3-large").category(),
        Category::Embeddings
    );
    assert_eq!(get("cohere/rerank-v3.5").category(), Category::Other);
}

#[tokio::test]
async fn together_bare_array_with_types() {
    let models = listed(
        "together",
        "/v1",
        serde_json::json!([
            {"id": "meta-llama/Llama-4-Maverick-17B-128E-Instruct-FP8", "object": "model", "type": "chat", "context_length": 1048576},
            {"id": "black-forest-labs/FLUX.1-schnell", "object": "model", "type": "image"},
            {"id": "BAAI/bge-large-en-v1.5", "object": "model", "type": "embedding"},
            {"id": "openai/whisper-large-v3", "object": "model", "type": "transcribe"}
        ]),
    )
    .await;
    let cats: Vec<_> = models.iter().map(|m| m.category()).collect();
    assert_eq!(
        cats,
        [
            Category::Embeddings,
            Category::Image,
            Category::Vision,
            Category::Speech
        ]
    );
}

#[tokio::test]
async fn openai_style_pages_are_followed() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/v1/models"))
        .and(query_param("after", "m2"))
        .respond_with(
            ResponseTemplate::new(200)
                .set_body_json(serde_json::json!({"data": [{"id": "m3"}], "has_more": false})),
        )
        .with_priority(1)
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path("/v1/models"))
        .respond_with(ResponseTemplate::new(200).set_body_json(
            serde_json::json!({"data": [{"id": "m1"}, {"id": "m2"}], "has_more": true, "last_id": "m2"}),
        ))
        .with_priority(2)
        .mount(&server)
        .await;
    let models = Provider::new(
        find("fireworks").unwrap(),
        key(),
        Some(&format!("{}/v1", server.uri())),
    )
    .unwrap()
    .list_models()
    .await
    .unwrap();
    assert_eq!(models.len(), 3);
}

#[tokio::test]
async fn ollama_capabilities_come_from_its_own_api() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/v1/models"))
        .respond_with(
            ResponseTemplate::new(200).set_body_json(serde_json::json!({"data": [
                {"id": "gemma3:4b"}, {"id": "nomic-embed-text:latest"}, {"id": "qwen3:8b"}
            ]})),
        )
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path("/api/tags"))
        .respond_with(
            ResponseTemplate::new(200).set_body_json(serde_json::json!({"models": [
                {"name": "gemma3:4b", "capabilities": ["completion", "vision"]},
                {"name": "nomic-embed-text:latest", "capabilities": ["embedding"]},
                {"name": "qwen3:8b", "capabilities": ["completion", "tools", "thinking"]}
            ]})),
        )
        .mount(&server)
        .await;
    let models = Provider::new(
        find("ollama").unwrap(),
        None,
        Some(&format!("{}/v1", server.uri())),
    )
    .unwrap()
    .list_models()
    .await
    .unwrap();
    let get = |id: &str| models.iter().find(|m| m.id == id).unwrap();
    assert_eq!(
        get("gemma3:4b").capabilities,
        [Capability::Chat, Capability::Vision]
    );
    assert_eq!(
        get("nomic-embed-text:latest").category(),
        Category::Embeddings
    );
    assert!(get("qwen3:8b").has(Capability::Tools) && !get("qwen3:8b").sees_images());
}

#[tokio::test]
async fn transcription_posts_multipart_and_reads_text() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/openai/v1/audio/transcriptions"))
        .and(header(
            "authorization",
            "Bearer sk-test-0123456789abcdefghij",
        ))
        .and(body_string_contains(
            "name=\"model\"\r\n\r\nwhisper-large-v3-turbo",
        ))
        .and(body_string_contains(
            "filename=\"speech.wav\"\r\nContent-Type: audio/wav",
        ))
        .and(body_string_contains("RIFF"))
        .respond_with(ResponseTemplate::new(200).set_body_json(
            serde_json::json!({"text": " What is a transformer? ", "x_groq": {"id": "req_1"}}),
        ))
        .mount(&server)
        .await;
    let p = Provider::new(
        find("groq").unwrap(),
        key(),
        Some(&format!("{}/openai/v1", server.uri())),
    )
    .unwrap();
    let text = p
        .transcribe(
            "whisper-large-v3-turbo",
            b"RIFF....WAVEfmt ",
            "speech.wav",
            None,
        )
        .await
        .unwrap();
    assert_eq!(text, "What is a transformer?");
}

#[tokio::test]
async fn transcription_errors_carry_the_providers_words() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/v1/audio/transcriptions"))
        .respond_with(ResponseTemplate::new(400).set_body_json(
            serde_json::json!({"error": {"message": "file must be one of flac, mp3, wav", "type": "invalid_request_error"}}),
        ))
        .mount(&server)
        .await;
    let p = Provider::new(
        find("openai").unwrap(),
        key(),
        Some(&format!("{}/v1", server.uri())),
    )
    .unwrap();
    let e = p
        .transcribe("gpt-transcribe", b"x", "a.bin", None)
        .await
        .unwrap_err();
    assert!(
        e.to_string().contains("file must be one of flac, mp3, wav"),
        "{e}"
    );
}

#[tokio::test]
async fn mistral_ocr_returns_page_markdown() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/v1/ocr"))
        .and(body_string_contains("\"type\":\"image_url\""))
        .and(body_string_contains("data:image/png;base64,iVBOR"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "pages": [{"index": 0, "markdown": "# Table 1\n\n| a | b |", "images": []}, {"index": 1, "markdown": "Page two"}],
            "model": "mistral-ocr-4-1", "usage_info": {"pages_processed": 2}
        })))
        .mount(&server)
        .await;
    let p = Provider::new(
        find("mistral").unwrap(),
        key(),
        Some(&format!("{}/v1", server.uri())),
    )
    .unwrap();
    let text = p
        .ocr("mistral-ocr-latest", "image/png", "iVBORw0KGgo=")
        .await
        .unwrap();
    assert_eq!(text, "# Table 1\n\n| a | b |\n\nPage two");
}
