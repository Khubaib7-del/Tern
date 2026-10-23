//! Model listing and kinds.

use serde::Serialize;

/// What a model is for. Inferred from the listing when the provider says,
/// otherwise from the model id; `Other` when neither tells.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum ModelKind {
    Chat,
    Embedding,
    Audio,
    Image,
    Moderation,
    Other,
}

#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct ModelInfo {
    pub id: String,
    pub display_name: Option<String>,
    pub kind: ModelKind,
    pub context_window: Option<u64>,
}

impl ModelInfo {
    pub fn new(id: impl Into<String>) -> Self {
        let id = id.into();
        let kind = classify(&id);
        Self {
            id,
            display_name: None,
            kind,
            context_window: None,
        }
    }
}

/// A context window from Tern's own table, for listings that leave it out
/// (OpenAI, Anthropic, Groq, xAI, DeepSeek, Ollama, LM Studio). Matched on
/// the id without any `provider/` or `models/` prefix or `:tag`; the first
/// rule whose prefix the id starts with wins, so longer prefixes come first.
/// Sizes are the vendors' published windows as of October 2026.
pub fn known_context_window(id: &str) -> Option<u64> {
    let s = id.to_ascii_lowercase();
    let s = s.rsplit('/').next().unwrap_or(&s);
    let s = s.split(':').next().unwrap_or(s);
    const K: u64 = 1_000;
    const TABLE: &[(&str, u64)] = &[
        // Mistral
        ("codestral", 256 * K),
        ("devstral", 128 * K),
        ("magistral", 128 * K),
        ("ministral", 128 * K),
        ("mistral-large", 128 * K),
        ("mistral-medium", 128 * K),
        ("mistral-small", 128 * K),
        ("mistral-saba", 32 * K),
        ("mistral-tiny", 32 * K),
        ("open-mistral-nemo", 128 * K),
        ("mistral-nemo", 128 * K),
        ("pixtral", 128 * K),
        ("voxtral", 32 * K),
        ("mistral", 32 * K),
        // OpenAI
        ("gpt-5", 400 * K),
        ("gpt-4.1", 1_047_576),
        ("gpt-4o", 128 * K),
        ("gpt-4-turbo", 128 * K),
        ("gpt-4", 8_192),
        ("gpt-3.5", 16_385),
        ("o1-mini", 128 * K),
        ("o1", 200 * K),
        ("o3", 200 * K),
        ("o4", 200 * K),
        ("gpt-oss", 131_072),
        // Anthropic
        ("claude", 200 * K),
        // Google
        ("gemini-1.5-pro", 2_097_152),
        ("gemini", 1_048_576),
        ("gemma-3", 128 * K),
        ("gemma3", 128 * K),
        ("gemma", 8_192),
        // xAI
        ("grok-4", 256 * K),
        ("grok-code", 256 * K),
        ("grok", 131_072),
        // DeepSeek
        ("deepseek", 128 * K),
        // Meta, Qwen, Kimi, others served by Groq, OpenRouter, Ollama
        ("llama-4", 131_072),
        ("llama-3.3", 131_072),
        ("llama-3.2", 131_072),
        ("llama-3.1", 131_072),
        ("llama3.3", 131_072),
        ("llama3.2", 131_072),
        ("llama3.1", 131_072),
        ("llama3", 8_192),
        ("qwen3", 128 * K),
        ("qwen2.5", 128 * K),
        ("qwq", 128 * K),
        ("kimi-k2", 256 * K),
        ("phi4", 16 * K),
        ("phi3", 128 * K),
    ];
    TABLE
        .iter()
        .find(|(p, _)| s.starts_with(p))
        .map(|(_, w)| *w)
}

/// Guesses a model's kind from its id. Unknown ids count as chat, because
/// every provider in the catalog is primarily a chat API.
pub fn classify(id: &str) -> ModelKind {
    let s = id.to_ascii_lowercase();
    let has = |needles: &[&str]| needles.iter().any(|n| s.contains(n));
    if has(&["embed", "bge-", "e5-", "nomic-embed"]) {
        ModelKind::Embedding
    } else if has(&["whisper", "tts", "transcribe", "audio", "speech"]) {
        ModelKind::Audio
    } else if has(&[
        "dall-e",
        "gpt-image",
        "imagen",
        "stable-diffusion",
        "sdxl",
        "flux",
    ]) {
        ModelKind::Image
    } else if has(&["moderation", "guard"]) {
        ModelKind::Moderation
    } else {
        ModelKind::Chat
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn known_windows_cover_common_models() {
        assert_eq!(known_context_window("ministral-8b-latest"), Some(128_000));
        assert_eq!(known_context_window("codestral-2508"), Some(256_000));
        assert_eq!(known_context_window("mistral-large-latest"), Some(128_000));
        assert_eq!(known_context_window("gpt-4o-mini"), Some(128_000));
        assert_eq!(known_context_window("gpt-4.1-mini"), Some(1_047_576));
        assert_eq!(known_context_window("claude-sonnet-4-5"), Some(200_000));
        assert_eq!(
            known_context_window("models/gemini-2.5-flash"),
            Some(1_048_576)
        );
        assert_eq!(
            known_context_window("meta-llama/llama-3.3-70b-instruct"),
            Some(131_072)
        );
        assert_eq!(known_context_window("llama3.2:3b"), Some(131_072));
        assert_eq!(
            known_context_window("llama-3.3-70b-versatile"),
            Some(131_072)
        );
        assert_eq!(known_context_window("my-own-finetune"), None);
    }

    #[test]
    fn classifies_common_ids() {
        assert_eq!(classify("gpt-4.1-mini"), ModelKind::Chat);
        assert_eq!(classify("claude-sonnet-4-5"), ModelKind::Chat);
        assert_eq!(classify("text-embedding-3-small"), ModelKind::Embedding);
        assert_eq!(
            classify("models/gemini-embedding-001"),
            ModelKind::Embedding
        );
        assert_eq!(classify("whisper-large-v3"), ModelKind::Audio);
        assert_eq!(classify("dall-e-3"), ModelKind::Image);
        assert_eq!(classify("omni-moderation-latest"), ModelKind::Moderation);
        assert_eq!(classify("llama-guard-4-12b"), ModelKind::Moderation);
    }
}
