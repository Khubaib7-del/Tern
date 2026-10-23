//! The providers Tern knows how to reach.

/// The request format a provider speaks.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Wire {
    OpenAiCompatible,
    Anthropic,
    Gemini,
}

/// One provider in the catalog.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ProviderSpec {
    /// Stable id used in commands and keychain names (`openai`).
    pub id: &'static str,
    pub name: &'static str,
    pub wire: Wire,
    pub base_url: &'static str,
    /// Local servers need no key.
    pub needs_key: bool,
    pub local: bool,
    /// Where the user creates a key.
    pub key_url: &'static str,
}

macro_rules! spec {
    ($id:literal, $name:literal, $wire:ident, $url:literal, $key_url:literal) => {
        ProviderSpec {
            id: $id,
            name: $name,
            wire: Wire::$wire,
            base_url: $url,
            needs_key: true,
            local: false,
            key_url: $key_url,
        }
    };
    (local $id:literal, $name:literal, $url:literal) => {
        ProviderSpec {
            id: $id,
            name: $name,
            wire: Wire::OpenAiCompatible,
            base_url: $url,
            needs_key: false,
            local: true,
            key_url: "",
        }
    };
}

/// The answer length Tern asks for when the user has not set one.
pub const DEFAULT_MAX_OUTPUT: u32 = 8192;

/// The output-token limit to ask `model` on `provider` for by default:
/// [`DEFAULT_MAX_OUTPUT`], or less for models known to cap lower (asking
/// for more than a model allows is an error on some services).
pub fn default_max_output(provider: &str, model: &str) -> u32 {
    let m = model.to_ascii_lowercase();
    let known_lower = [
        // OpenAI's older chat models.
        "gpt-3.5",
        "gpt-4-0613",
        "gpt-4-0314",
        // Anthropic's Claude 3 (not 3.5 or later).
        "claude-3-opus",
        "claude-3-sonnet",
        "claude-3-haiku",
    ];
    if known_lower.iter().any(|k| m.contains(k)) {
        return 4096;
    }
    match provider {
        // Small hosted catalogs whose defaults are often 4k.
        "together" | "fireworks" => 4096,
        _ => DEFAULT_MAX_OUTPUT,
    }
}

pub const CATALOG: &[ProviderSpec] = &[
    spec!(
        "openai",
        "OpenAI",
        OpenAiCompatible,
        "https://api.openai.com/v1",
        "https://platform.openai.com/api-keys"
    ),
    spec!(
        "anthropic",
        "Anthropic",
        Anthropic,
        "https://api.anthropic.com/v1",
        "https://console.anthropic.com/settings/keys"
    ),
    spec!(
        "google",
        "Google AI Studio",
        Gemini,
        "https://generativelanguage.googleapis.com/v1beta",
        "https://aistudio.google.com/apikey"
    ),
    spec!(
        "openrouter",
        "OpenRouter",
        OpenAiCompatible,
        "https://openrouter.ai/api/v1",
        "https://openrouter.ai/settings/keys"
    ),
    spec!(
        "groq",
        "Groq",
        OpenAiCompatible,
        "https://api.groq.com/openai/v1",
        "https://console.groq.com/keys"
    ),
    spec!(
        "mistral",
        "Mistral",
        OpenAiCompatible,
        "https://api.mistral.ai/v1",
        "https://console.mistral.ai/api-keys"
    ),
    spec!(
        "deepseek",
        "DeepSeek",
        OpenAiCompatible,
        "https://api.deepseek.com/v1",
        "https://platform.deepseek.com/api_keys"
    ),
    spec!(
        "xai",
        "xAI",
        OpenAiCompatible,
        "https://api.x.ai/v1",
        "https://console.x.ai"
    ),
    spec!(
        "together",
        "Together AI",
        OpenAiCompatible,
        "https://api.together.xyz/v1",
        "https://api.together.ai/settings/api-keys"
    ),
    spec!(
        "fireworks",
        "Fireworks AI",
        OpenAiCompatible,
        "https://api.fireworks.ai/inference/v1",
        "https://fireworks.ai/account/api-keys"
    ),
    spec!(
        "cerebras",
        "Cerebras",
        OpenAiCompatible,
        "https://api.cerebras.ai/v1",
        "https://cloud.cerebras.ai"
    ),
    spec!(local "ollama", "Ollama", "http://localhost:11434/v1"),
    spec!(local "lmstudio", "LM Studio", "http://localhost:1234/v1"),
];

/// Looks a provider up by id, case-insensitively.
pub fn find(id: &str) -> Option<&'static ProviderSpec> {
    CATALOG.iter().find(|p| p.id.eq_ignore_ascii_case(id))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn answers_get_room_by_default() {
        assert_eq!(default_max_output("mistral", "ministral-8b-latest"), 8192);
        assert_eq!(default_max_output("openai", "gpt-3.5-turbo"), 4096);
        assert_eq!(
            default_max_output("anthropic", "claude-3-haiku-20240307"),
            4096
        );
        assert_eq!(
            default_max_output("anthropic", "claude-3-5-sonnet-latest"),
            8192
        );
        assert!(default_max_output("groq", "llama-3.3-70b-versatile") > 2048);
    }

    #[test]
    fn ids_are_unique_lowercase_and_keychain_safe() {
        let mut seen = std::collections::HashSet::new();
        for p in CATALOG {
            assert!(seen.insert(p.id), "duplicate {}", p.id);
            assert!(
                p.id.bytes()
                    .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit()),
                "{}",
                p.id
            );
        }
    }

    #[test]
    fn remote_providers_use_https_and_have_a_key_page() {
        for p in CATALOG.iter().filter(|p| !p.local) {
            assert!(p.base_url.starts_with("https://"), "{}", p.id);
            assert!(p.needs_key && p.key_url.starts_with("https://"), "{}", p.id);
            assert!(!p.base_url.ends_with('/'), "{}", p.id);
        }
    }

    #[test]
    fn local_providers_stay_on_localhost() {
        for p in CATALOG.iter().filter(|p| p.local) {
            assert!(p.base_url.starts_with("http://localhost:"), "{}", p.id);
            assert!(!p.needs_key);
        }
    }

    #[test]
    fn find_is_case_insensitive() {
        assert_eq!(find("OpenAI").unwrap().id, "openai");
        assert!(find("nope").is_none());
    }
}
