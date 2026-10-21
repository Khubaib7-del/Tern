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
