//! Model listing, capabilities, and categories.
//!
//! Every model a provider lists for the key is kept. What a model can do
//! comes from the provider's own metadata first (Mistral `capabilities`,
//! Anthropic `capabilities`, OpenRouter modalities, Gemini generation
//! methods, Ollama `capabilities`, LM Studio `type`, xAI modalities) and,
//! where the listing says nothing, from Tern's pattern table
//! ([`table_capabilities`]). See `docs/models.md`.

use serde::Serialize;

/// What a model is for, in one word: kept for callers that only need a
/// coarse answer. [`ModelInfo::capabilities`] holds the detail.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum ModelKind {
    Chat,
    Embedding,
    Audio,
    Image,
    Ocr,
    Moderation,
    Other,
}

/// One thing a model can do.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Capability {
    /// Answers chat messages.
    Chat,
    /// Reads pictures sent with a chat message.
    Vision,
    /// Calls tools (function calling).
    Tools,
    /// Thinks before answering.
    Reasoning,
    /// Transcribes speech (`/audio/transcriptions`).
    SpeechToText,
    /// Reads text aloud (`/audio/speech`).
    TextToSpeech,
    /// Reads text out of documents and pictures (Mistral `/ocr`).
    Ocr,
    /// Makes pictures.
    ImageGen,
    /// Turns text into vectors.
    Embedding,
    /// Screens text for safety.
    Moderation,
    /// Ranks passages against a query.
    Rerank,
}

impl Capability {
    /// A short badge for the picker.
    pub fn badge(self) -> &'static str {
        match self {
            Self::Chat => "chat",
            Self::Vision => "vision",
            Self::Tools => "tools",
            Self::Reasoning => "thinks",
            Self::SpeechToText => "speech to text",
            Self::TextToSpeech => "text to speech",
            Self::Ocr => "OCR",
            Self::ImageGen => "makes images",
            Self::Embedding => "embeddings",
            Self::Moderation => "safety",
            Self::Rerank => "rerank",
        }
    }
}

/// The picker section a model is listed in. A model is in exactly one.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Category {
    /// Chat models that read text only.
    Chat,
    /// Chat models that also read pictures.
    Vision,
    /// Speech to text and text to speech.
    Speech,
    Ocr,
    /// Image makers.
    Image,
    Embeddings,
    /// Moderation and prompt-guard models.
    Safety,
    /// Rerankers, video, realtime, time series: listed, never used by Tern.
    Other,
}

impl Category {
    /// Picker order.
    pub const ALL: [Category; 8] = [
        Self::Chat,
        Self::Vision,
        Self::Speech,
        Self::Ocr,
        Self::Image,
        Self::Embeddings,
        Self::Safety,
        Self::Other,
    ];

    pub fn label(self) -> &'static str {
        match self {
            Self::Chat => "Chat",
            Self::Vision => "Vision",
            Self::Speech => "Speech",
            Self::Ocr => "OCR",
            Self::Image => "Image",
            Self::Embeddings => "Embeddings",
            Self::Safety => "Safety",
            Self::Other => "Other",
        }
    }
}

/// Where a model is in its provider's lifecycle. Anything but `Active` is
/// listed behind "Show retired".
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Lifecycle {
    #[default]
    Active,
    /// Still answers, closed to new use or scheduled for retirement.
    Deprecated,
    /// No longer answers.
    Retired,
}

/// How much the listing said about a model's capabilities.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Source {
    /// Nothing: the pattern table decides.
    #[default]
    Table,
    /// The role only (chat, embeddings, image): the table adds vision and
    /// tools, and a clearly named voice, OCR, or image model keeps its role.
    Partial,
    /// The listing names every capability: it is taken as given.
    Listing,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct ModelInfo {
    pub id: String,
    pub display_name: Option<String>,
    pub kind: ModelKind,
    pub context_window: Option<u64>,
    /// Sorted, no repeats.
    pub capabilities: Vec<Capability>,
    pub lifecycle: Lifecycle,
    /// Other names the provider accepts for this model (`*-latest`).
    pub aliases: Vec<String>,
    pub source: Source,
}

impl ModelInfo {
    /// A model known by id only: capabilities from the pattern table.
    pub fn new(id: impl Into<String>) -> Self {
        let id = id.into();
        let capabilities = table_capabilities(&id);
        let kind = kind_of(&capabilities);
        Self {
            id,
            display_name: None,
            kind,
            context_window: None,
            capabilities,
            lifecycle: Lifecycle::Active,
            aliases: Vec::new(),
            source: Source::Table,
        }
    }

    /// A model whose listing said what it can do (`source` says how much).
    pub fn with_listing(id: impl Into<String>, caps: Vec<Capability>, source: Source) -> Self {
        let mut m = Self::new(id);
        m.source = source;
        m.capabilities = caps;
        m
    }

    pub fn has(&self, c: Capability) -> bool {
        self.capabilities.contains(&c)
    }

    pub fn can_chat(&self) -> bool {
        self.has(Capability::Chat)
    }

    /// Chat with pictures: the model can describe an attached image.
    pub fn sees_images(&self) -> bool {
        self.can_chat() && self.has(Capability::Vision)
    }

    pub fn is_retired(&self) -> bool {
        self.lifecycle != Lifecycle::Active
    }

    pub fn category(&self) -> Category {
        let has = |c| self.has(c);
        if has(Capability::Chat) {
            return if has(Capability::Vision) {
                Category::Vision
            } else {
                Category::Chat
            };
        }
        if has(Capability::SpeechToText) || has(Capability::TextToSpeech) {
            Category::Speech
        } else if has(Capability::Ocr) {
            Category::Ocr
        } else if has(Capability::ImageGen) {
            Category::Image
        } else if has(Capability::Embedding) {
            Category::Embeddings
        } else if has(Capability::Moderation) {
            Category::Safety
        } else {
            Category::Other
        }
    }

    /// The name to show: the provider's display name, else Tern's table's,
    /// else the id.
    pub fn label(&self) -> &str {
        self.display_name
            .as_deref()
            .or_else(|| known_display_name(&self.id))
            .unwrap_or(&self.id)
    }

    /// Whether a search matches the id, the name, or an alias.
    pub fn matches(&self, query: &str) -> bool {
        let q = query.trim().to_ascii_lowercase();
        if q.is_empty() {
            return true;
        }
        let hit = |s: &str| s.to_ascii_lowercase().contains(&q);
        hit(&self.id) || hit(self.label()) || self.aliases.iter().any(|a| hit(a))
    }

    /// Why this model cannot be the chat model, in one sentence the user
    /// reads; `None` for chat models.
    pub fn not_chat_reason(&self) -> Option<String> {
        if self.can_chat() {
            return None;
        }
        let id = &self.id;
        let has = |c| self.has(c);
        Some(if has(Capability::SpeechToText) {
            format!(
                "{id} only transcribes speech, so it cannot chat. Pick a chat model; Tern uses {id} for the mic."
            )
        } else if has(Capability::TextToSpeech) {
            format!("{id} only reads text aloud, so it cannot chat. Pick a chat model.")
        } else if has(Capability::Ocr) {
            format!(
                "{id} only reads text out of documents and pictures, so it cannot chat. Pick a chat model; Tern uses {id} for attachments the chat model cannot see."
            )
        } else if has(Capability::ImageGen) {
            format!("{id} only makes pictures, so it cannot chat. Pick a chat model.")
        } else if has(Capability::Embedding) {
            format!("{id} only turns text into embeddings, so it cannot chat. Pick a chat model.")
        } else if has(Capability::Moderation) {
            format!("{id} only screens text for safety, so it cannot chat. Pick a chat model.")
        } else if has(Capability::Rerank) {
            format!("{id} only ranks passages, so it cannot chat. Pick a chat model.")
        } else {
            format!("{id} is not a chat model. Pick a chat model.")
        })
    }

    /// Settles the capabilities once the listing is read: the table fills
    /// what the listing left out, reasoning comes from Tern's thinking
    /// table for `provider`, and `kind` follows.
    pub fn finish(&mut self, provider: &str) {
        let table = table_capabilities(&self.id);
        let mut caps = std::mem::take(&mut self.capabilities);
        match self.source {
            Source::Table => caps = table,
            Source::Listing if caps.is_empty() => caps = table,
            Source::Listing => {}
            Source::Partial => {
                let strong = strong_role(&self.id);
                let listed_role = caps.iter().any(|c| !matches!(c, Capability::Chat));
                if let (Some(role), false) = (strong, listed_role) {
                    caps = vec![role];
                } else if caps.is_empty() {
                    caps = table;
                } else if caps.contains(&Capability::Chat) {
                    for c in [Capability::Vision, Capability::Tools] {
                        if table.contains(&c) {
                            caps.push(c);
                        }
                    }
                }
            }
        }
        if caps.contains(&Capability::Chat)
            && crate::effort::thinking_support(provider, &self.id).visible()
        {
            caps.push(Capability::Reasoning);
        }
        caps.sort();
        caps.dedup();
        self.kind = kind_of(&caps);
        self.capabilities = caps;
        if self.context_window.is_none() && self.can_chat() {
            self.context_window = known_context_window(&self.id);
        }
    }
}

fn kind_of(caps: &[Capability]) -> ModelKind {
    let has = |c| caps.contains(&c);
    if has(Capability::Chat) {
        ModelKind::Chat
    } else if has(Capability::SpeechToText) || has(Capability::TextToSpeech) {
        ModelKind::Audio
    } else if has(Capability::Ocr) {
        ModelKind::Ocr
    } else if has(Capability::ImageGen) {
        ModelKind::Image
    } else if has(Capability::Embedding) {
        ModelKind::Embedding
    } else if has(Capability::Moderation) {
        ModelKind::Moderation
    } else {
        ModelKind::Other
    }
}

/// The id without a `provider/` or `models/` prefix or `:tag`, lower case.
fn bare(id: &str) -> String {
    let s = id.to_ascii_lowercase();
    let s = s.rsplit('/').next().unwrap_or(&s).to_owned();
    s.split(':').next().unwrap_or(&s).to_owned()
}

/// A role so clearly in the name that a coarse listing ("this model takes
/// generateContent") does not override it: Gemini's TTS models answer
/// generateContent but only speak.
fn strong_role(id: &str) -> Option<Capability> {
    let s = bare(id);
    let has = |needles: &[&str]| needles.iter().any(|n| s.contains(n));
    if has(&["whisper", "transcribe", "-stt"]) {
        Some(Capability::SpeechToText)
    } else if has(&["-tts", "tts-", "orpheus", "playai"]) {
        Some(Capability::TextToSpeech)
    } else if has(&["-ocr", "ocr-"]) {
        Some(Capability::Ocr)
    } else if has(&["embed"]) {
        Some(Capability::Embedding)
    } else if has(&[
        "imagen",
        "dall-e",
        "gpt-image",
        "-image-generation",
        "flash-image",
    ]) {
        Some(Capability::ImageGen)
    } else if has(&["moderation", "guard", "shieldstral"]) {
        Some(Capability::Moderation)
    } else if has(&["rerank"]) {
        Some(Capability::Rerank)
    } else {
        None
    }
}

/// Tern's pattern table: what a model can do, read from its id, for
/// listings that say nothing (OpenAI, Groq, DeepSeek, Cerebras,
/// Fireworks). Ordered: the first role that matches wins; chat models then
/// get vision and tools from the family lists below. Ids that match no role
/// count as chat, because every provider in the catalog is first a chat API.
pub fn table_capabilities(id: &str) -> Vec<Capability> {
    use Capability::*;
    let s = bare(id);
    let has = |needles: &[&str]| needles.iter().any(|n| s.contains(n));
    let starts = |prefixes: &[&str]| prefixes.iter().any(|p| s.starts_with(p));
    if let Some(role) = strong_role(id) {
        return vec![role];
    }
    // Listed but not usable by Tern: realtime voice sessions, video,
    // time series.
    if has(&["realtime", "sora", "veo-", "video", "timestral"]) {
        return Vec::new();
    }
    if has(&[
        "stable-diffusion",
        "sdxl",
        "flux",
        "grok-2-image",
        "grok-imagine",
    ]) {
        return vec![ImageGen];
    }
    if has(&["nomic-embed", "bge-", "e5-", "gte-"]) {
        return vec![Embedding];
    }
    let mut caps = vec![Chat];
    let vision = has(&[
        "vision",
        "pixtral",
        "-vl",
        "vl-",
        "llava",
        "gpt-4o",
        "gpt-4.1",
        "gpt-4.5",
        "gpt-5",
        "gpt-6",
        "chatgpt-4o",
        "claude",
        "gemini",
        "gemma-3",
        "gemma3",
        "llama-4",
        "llama4",
        "muse-spark",
        "qwen3.8",
        "qwen3-vl",
        "qwen2.5-vl",
        "grok-4",
        "mistral-large-3",
        "mistral-large-2512",
        "mistral-large-4",
        "mistral-medium-3",
        "mistral-small-3.1",
        "mistral-small-3.2",
        "mistral-small-2503",
        "mistral-small-2506",
        "mistral-small-2603",
        "ministral-3",
        "ministral-14b",
        "ministral-8b-2512",
        "ministral-3b-2512",
        "glm-5-3-flash",
        "kimi-vl",
        "minicpm-v",
        "moondream",
    ]) || starts(&["o1", "o3", "o4"]);
    if vision {
        caps.push(Vision);
    }
    let tools = has(&[
        "gpt-4",
        "gpt-5",
        "gpt-6",
        "gpt-oss",
        "claude",
        "gemini",
        "mistral",
        "ministral",
        "magistral",
        "codestral",
        "devstral",
        "pixtral",
        "llama-3.1",
        "llama-3.3",
        "llama-4",
        "llama3.1",
        "llama3.3",
        "qwen",
        "grok",
        "deepseek",
        "kimi",
        "glm",
        "minimax",
        "command",
    ]) || starts(&["o1", "o3", "o4"]);
    if tools {
        caps.push(Tools);
    }
    caps
}

/// Display names for ids whose listing leaves the name out, from the
/// providers' model pages (October 2026). Mistral's ids carry a date, not
/// the version people know them by ("Mistral Small 4" is
/// `mistral-small-2603`).
pub fn known_display_name(id: &str) -> Option<&'static str> {
    const TABLE: &[(&str, &str)] = &[
        ("mistral-large-4", "Mistral Large 4"),
        ("mistral-large-2512", "Mistral Large 3"),
        ("mistral-medium-3-5", "Mistral Medium 3.5"),
        ("mistral-medium-2508", "Mistral Medium 3.1"),
        ("mistral-medium-2505", "Mistral Medium 3"),
        ("mistral-small-2603", "Mistral Small 4"),
        ("mistral-small-2506", "Mistral Small 3.2"),
        ("ministral-14b-2512", "Ministral 3 14B"),
        ("ministral-8b-2512", "Ministral 3 8B"),
        ("ministral-3b-2512", "Ministral 3 3B"),
        ("codestral-2508", "Codestral 25.08"),
        ("codestral-embed-2505", "Codestral Embed"),
        ("mistral-embed-2312", "Mistral Embed"),
        ("mistral-moderation-2603", "Mistral Moderation 2"),
        ("mistral-ocr-4-1", "OCR 4.1"),
        ("mistral-ocr-4-0", "OCR 4.0"),
        ("mistral-ocr-2512", "OCR 3"),
        ("voxtral-small-2507", "Voxtral Small"),
        ("voxtral-mini-2602", "Voxtral Mini Transcribe 2"),
        ("voxtral-mini-transcribe-3-0", "Voxtral Mini Transcribe 3"),
        ("voxtral-small-transcribe-3-0", "Voxtral Small Transcribe 3"),
        (
            "voxtral-mini-transcribe-realtime-3-0",
            "Voxtral Mini Transcribe Realtime 3",
        ),
        (
            "voxtral-mini-transcribe-realtime-2602",
            "Voxtral Mini Transcribe Realtime",
        ),
        ("voxtral-mini-tts-2603", "Voxtral TTS"),
        ("voxtral-mini-tts-3-0", "Voxtral TTS 3"),
        ("zai-glm-5-3-flash", "Z.ai GLM 5.3 Flash"),
        ("zai-glm-5-3", "Z.ai GLM 5.3"),
        ("magistral-medium-2509", "Magistral Medium 1.2"),
        ("magistral-small-2509", "Magistral Small 1.2"),
        ("devstral-2512", "Devstral 2"),
        ("pixtral-large-2411", "Pixtral Large"),
        ("pixtral-12b-2409", "Pixtral 12B"),
    ];
    let s = bare(id);
    TABLE.iter().find(|(k, _)| *k == s).map(|(_, v)| *v)
}

/// Folds alias cards into one row each. Mistral lists `mistral-large-latest`
/// and `mistral-large-2512` as two cards naming each other in `aliases`;
/// the dated id stays (it names the exact model) and the others become its
/// aliases. Models without aliases pass through.
pub fn tidy_aliases(models: Vec<ModelInfo>) -> Vec<ModelInfo> {
    use std::collections::HashMap;
    let n = models.len();
    let index: HashMap<String, usize> = models
        .iter()
        .enumerate()
        .map(|(i, m)| (m.id.clone(), i))
        .collect();
    let mut parent: Vec<usize> = (0..n).collect();
    fn root(p: &mut [usize], mut i: usize) -> usize {
        while p[i] != i {
            p[i] = p[p[i]];
            i = p[i];
        }
        i
    }
    for (i, m) in models.iter().enumerate() {
        for a in &m.aliases {
            if let Some(&j) = index.get(a) {
                let (ri, rj) = (root(&mut parent, i), root(&mut parent, j));
                if ri != rj {
                    parent[ri] = rj;
                }
            }
        }
    }
    let mut groups: HashMap<usize, Vec<usize>> = HashMap::new();
    for i in 0..n {
        let r = root(&mut parent, i);
        groups.entry(r).or_default().push(i);
    }
    let mut keep: Vec<usize> = Vec::new();
    let mut names: HashMap<usize, Vec<String>> = HashMap::new();
    for members in groups.values() {
        let best = *members
            .iter()
            .min_by_key(|&&i| canonical_rank(&models[i].id))
            .expect("non-empty group");
        let mut all: Vec<String> = members
            .iter()
            .flat_map(|&i| std::iter::once(models[i].id.clone()).chain(models[i].aliases.clone()))
            .filter(|a| *a != models[best].id)
            .collect();
        all.sort();
        all.dedup();
        keep.push(best);
        names.insert(best, all);
    }
    keep.sort_unstable();
    let mut models = models;
    let mut out = Vec::with_capacity(keep.len());
    for i in keep.into_iter().rev() {
        let mut m = models.swap_remove(i);
        m.aliases = names.remove(&i).unwrap_or_default();
        out.push(m);
    }
    out.reverse();
    out
}

/// Lower ranks are better names for a group's row: an id with a version or
/// date over a `-latest` alias, then the shorter id.
fn canonical_rank(id: &str) -> (bool, bool, usize, String) {
    let lower = id.to_ascii_lowercase();
    let floating = lower.ends_with("latest") || lower.ends_with("-beta") || !lower.contains('-');
    let dated = lower
        .rsplit('-')
        .next()
        .is_some_and(|t| t.len() >= 3 && t.chars().all(|c| c.is_ascii_digit()));
    (floating, !dated, id.len(), id.to_owned())
}

/// A context window from Tern's own table, for listings that leave it out
/// (OpenAI, Anthropic, Groq, xAI, DeepSeek, Ollama, LM Studio). Matched on
/// the id without any `provider/` or `models/` prefix or `:tag`; the first
/// rule whose prefix the id starts with wins, so longer prefixes come first.
/// Sizes are the vendors' published windows as of October 2026.
pub fn known_context_window(id: &str) -> Option<u64> {
    let s = bare(id);
    const K: u64 = 1_000;
    const TABLE: &[(&str, u64)] = &[
        // Mistral
        ("codestral", 256 * K),
        ("devstral", 128 * K),
        ("magistral", 128 * K),
        ("ministral", 128 * K),
        ("mistral-large-4", 1_000 * K),
        ("mistral-large-2512", 256 * K),
        ("mistral-large", 128 * K),
        ("mistral-medium-3-5", 256 * K),
        ("mistral-medium", 128 * K),
        ("mistral-small-2603", 256 * K),
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

/// Guesses a model's kind from its id (the pattern table's role).
pub fn classify(id: &str) -> ModelKind {
    kind_of(&table_capabilities(id))
}

#[cfg(test)]
mod tests {
    use super::*;
    use Capability::*;

    #[test]
    fn known_windows_cover_common_models() {
        assert_eq!(known_context_window("ministral-8b-latest"), Some(128_000));
        assert_eq!(known_context_window("codestral-2508"), Some(256_000));
        assert_eq!(known_context_window("mistral-large-latest"), Some(128_000));
        assert_eq!(known_context_window("mistral-large-4"), Some(1_000_000));
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
        assert_eq!(classify("mistral-ocr-latest"), ModelKind::Ocr);
    }

    #[test]
    fn pattern_table_knows_each_job() {
        let caps = |id| table_capabilities(id);
        for id in [
            "whisper-large-v3",
            "whisper-large-v3-turbo",
            "distil-whisper-large-v3-en",
            "gpt-transcribe",
            "gpt-4o-mini-transcribe",
            "voxtral-mini-transcribe-3-0",
        ] {
            assert_eq!(caps(id), [SpeechToText], "{id}");
        }
        for id in [
            "playai-tts",
            "canopylabs/orpheus-v1-english",
            "tts-1-hd",
            "gpt-4o-mini-tts",
            "voxtral-mini-tts-3-0",
            "gemini-2.5-flash-preview-tts",
        ] {
            assert_eq!(caps(id), [TextToSpeech], "{id}");
        }
        for id in ["mistral-ocr-latest", "mistral-ocr-2512", "mistral-ocr-4-1"] {
            assert_eq!(caps(id), [Ocr], "{id}");
        }
        for id in ["dall-e-3", "gpt-image-1", "imagen-4.0-generate-001"] {
            assert_eq!(caps(id), [ImageGen], "{id}");
        }
        for id in [
            "meta-llama/llama-prompt-guard-2-22m",
            "openai/gpt-oss-safeguard-20b",
            "mistral-moderation-2603",
        ] {
            assert_eq!(caps(id), [Moderation], "{id}");
        }
        assert_eq!(caps("mistral-embed"), [Embedding]);
        assert!(caps("gpt-realtime").is_empty());
        assert_eq!(caps("llama-3.1-8b-instant"), [Chat, Tools]);
        assert_eq!(caps("pixtral-large-latest"), [Chat, Vision, Tools]);
        assert_eq!(caps("qwen/qwen3.8-27b"), [Chat, Vision, Tools]);
        assert_eq!(caps("my-own-finetune"), [Chat]);
    }

    #[test]
    fn categories_and_reasons() {
        let mut w = ModelInfo::new("whisper-large-v3");
        w.finish("groq");
        assert_eq!(w.category(), Category::Speech);
        let r = w.not_chat_reason().unwrap();
        assert!(
            r.starts_with("whisper-large-v3 only transcribes speech") && r.contains("mic"),
            "{r}"
        );
        let mut v = ModelInfo::new("pixtral-large-latest");
        v.finish("mistral");
        assert_eq!(v.category(), Category::Vision);
        assert!(v.not_chat_reason().is_none() && v.sees_images());
        let mut o = ModelInfo::new("mistral-ocr-latest");
        o.finish("mistral");
        assert_eq!(o.category(), Category::Ocr);
        assert!(o.not_chat_reason().unwrap().contains("attachments"));
    }

    #[test]
    fn partial_listings_keep_named_roles() {
        // Gemini says generateContent for its TTS model; the name wins.
        let mut tts =
            ModelInfo::with_listing("gemini-2.5-flash-preview-tts", vec![Chat], Source::Partial);
        tts.finish("google");
        assert_eq!(tts.capabilities, [TextToSpeech]);
        let mut chat = ModelInfo::with_listing("gemini-2.5-pro", vec![Chat], Source::Partial);
        chat.finish("google");
        assert!(chat.sees_images() && chat.has(Tools) && chat.has(Reasoning));
        // A full listing is taken as given.
        let mut m = ModelInfo::with_listing("ministral-8b-2512", vec![Chat], Source::Listing);
        m.finish("mistral");
        assert_eq!(m.capabilities, [Chat]);
    }

    #[test]
    fn display_names_and_search() {
        let mut m = ModelInfo::new("mistral-small-2603");
        m.aliases = vec!["mistral-small-latest".into()];
        assert_eq!(m.label(), "Mistral Small 4");
        assert!(m.matches("small 4"));
        assert!(m.matches("SMALL-LATEST"));
        assert!(!m.matches("large"));
    }

    #[test]
    fn alias_cards_fold_into_the_dated_id() {
        let card = |id: &str, aliases: &[&str]| {
            let mut m = ModelInfo::new(id);
            m.aliases = aliases.iter().map(|s| s.to_string()).collect();
            m
        };
        let out = tidy_aliases(vec![
            card("mistral-large-latest", &["mistral-large-2512"]),
            card("mistral-large-2512", &["mistral-large-latest"]),
            card("codestral-2508", &["codestral-latest"]),
            card("ministral-8b-2512", &[]),
        ]);
        let ids: Vec<_> = out.iter().map(|m| m.id.as_str()).collect();
        assert_eq!(
            ids,
            ["mistral-large-2512", "codestral-2508", "ministral-8b-2512"]
        );
        assert_eq!(out[0].aliases, ["mistral-large-latest"]);
        assert_eq!(out[1].aliases, ["codestral-latest"]);
    }
}
