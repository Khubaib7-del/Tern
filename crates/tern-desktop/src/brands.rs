//! Provider and search-service logos: which mark a service gets and how it
//! is drawn. GPUI-free so the lookup is unit tested; `view` turns a
//! [`Logo`] into an element.
//!
//! Marks with colour (Mistral's pixel M, Claude's spark, Gemini's star…)
//! are drawn as images in their own colours. One-colour marks (OpenAI,
//! xAI, Ollama…) are drawn in the text colour, so they read on both
//! themes, the way their owners show them on dark and light. A service
//! Tern has no mark for, or a custom one, gets its initial.
//!
//! Sources and licences: `assets/brands/NOTICE.md`.

/// How a logo is drawn.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Tint {
    /// An image in the mark's own colours.
    Original,
    /// One colour, tinted with the text colour.
    Mono,
}

/// What to draw in a service's logo spot.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Logo {
    /// A bundled mark: its asset path and how to draw it.
    Mark(&'static str, Tint),
    /// No mark: the service's initial on a neutral tile.
    Initial(String),
}

/// `(id, asset bytes, tint)` for every bundled brand mark.
const BRANDS: &[(&str, &[u8], Tint)] = &[
    (
        "anthropic",
        include_bytes!("../assets/brands/anthropic.svg"),
        Tint::Original,
    ),
    (
        "brave",
        include_bytes!("../assets/brands/brave.svg"),
        Tint::Original,
    ),
    (
        "deepseek",
        include_bytes!("../assets/brands/deepseek.svg"),
        Tint::Original,
    ),
    (
        "google",
        include_bytes!("../assets/brands/google.svg"),
        Tint::Original,
    ),
    (
        "groq",
        include_bytes!("../assets/brands/groq.svg"),
        Tint::Original,
    ),
    (
        "lmstudio",
        include_bytes!("../assets/brands/lmstudio.svg"),
        Tint::Mono,
    ),
    (
        "mistral",
        include_bytes!("../assets/brands/mistral.svg"),
        Tint::Original,
    ),
    (
        "ollama",
        include_bytes!("../assets/brands/ollama.svg"),
        Tint::Mono,
    ),
    (
        "openai",
        include_bytes!("../assets/brands/openai.svg"),
        Tint::Mono,
    ),
    (
        "openrouter",
        include_bytes!("../assets/brands/openrouter.svg"),
        Tint::Mono,
    ),
    (
        "searxng",
        include_bytes!("../assets/brands/searxng.svg"),
        Tint::Original,
    ),
    (
        "tavily",
        include_bytes!("../assets/brands/tavily.svg"),
        Tint::Original,
    ),
    (
        "xai",
        include_bytes!("../assets/brands/xai.svg"),
        Tint::Mono,
    ),
];

/// Where brand marks live in the asset source.
const PREFIX: &str = "tern/brands/";

/// The canonical id for a provider or search id, or a display name
/// ("Google AI Studio", "LM Studio", "Brave Search").
fn canonical(id_or_name: &str) -> Option<&'static str> {
    let k: String = id_or_name
        .trim()
        .to_ascii_lowercase()
        .chars()
        .filter(|c| c.is_ascii_alphanumeric())
        .collect();
    Some(match k.as_str() {
        "mistral" | "mistralai" => "mistral",
        "openai" => "openai",
        "anthropic" | "claude" => "anthropic",
        "google" | "googleaistudio" | "gemini" => "google",
        "groq" => "groq",
        "openrouter" => "openrouter",
        "xai" | "grok" => "xai",
        "deepseek" => "deepseek",
        "ollama" => "ollama",
        "lmstudio" => "lmstudio",
        "tavily" => "tavily",
        "brave" | "bravesearch" => "brave",
        "searxng" => "searxng",
        "wikipedia" => "wikipedia",
        _ => return None,
    })
}

/// The logo for a provider or search service, by id or name.
pub fn logo(id_or_name: &str) -> Logo {
    match canonical(id_or_name) {
        // Wikipedia's W is the one-colour service logo Tern already ships.
        Some("wikipedia") => Logo::Mark("tern/service/wikipedia.svg", Tint::Mono),
        Some(id) => match BRANDS.iter().find(|(n, _, _)| *n == id) {
            Some((n, _, tint)) => Logo::Mark(path_of(n), *tint),
            None => Logo::Initial(initial(id_or_name)),
        },
        None => Logo::Initial(initial(id_or_name)),
    }
}

fn path_of(id: &str) -> &'static str {
    // The ids are a fixed set, so a match keeps the paths 'static.
    match id {
        "anthropic" => "tern/brands/anthropic.svg",
        "brave" => "tern/brands/brave.svg",
        "deepseek" => "tern/brands/deepseek.svg",
        "google" => "tern/brands/google.svg",
        "groq" => "tern/brands/groq.svg",
        "lmstudio" => "tern/brands/lmstudio.svg",
        "mistral" => "tern/brands/mistral.svg",
        "ollama" => "tern/brands/ollama.svg",
        "openai" => "tern/brands/openai.svg",
        "openrouter" => "tern/brands/openrouter.svg",
        "searxng" => "tern/brands/searxng.svg",
        "tavily" => "tern/brands/tavily.svg",
        _ => "tern/brands/xai.svg",
    }
}

/// The first letter or digit of a name, upper-cased.
pub fn initial(name: &str) -> String {
    name.chars()
        .find(|c| c.is_alphanumeric())
        .map(|c| c.to_uppercase().collect())
        .unwrap_or_default()
}

/// The bytes of a brand asset path, for the asset source.
pub fn lookup(path: &str) -> Option<&'static [u8]> {
    let id = path.strip_prefix(PREFIX)?.strip_suffix(".svg")?;
    BRANDS.iter().find(|(n, _, _)| *n == id).map(|(_, b, _)| *b)
}

/// Every brand asset path.
pub fn paths() -> impl Iterator<Item = &'static str> {
    BRANDS.iter().map(|(n, _, _)| path_of(n))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn known_services_get_their_mark_by_id_or_name() {
        assert_eq!(
            logo("mistral"),
            Logo::Mark("tern/brands/mistral.svg", Tint::Original)
        );
        assert_eq!(
            logo("Mistral"),
            Logo::Mark("tern/brands/mistral.svg", Tint::Original)
        );
        assert_eq!(
            logo("Google AI Studio"),
            Logo::Mark("tern/brands/google.svg", Tint::Original)
        );
        assert_eq!(
            logo("LM Studio"),
            Logo::Mark("tern/brands/lmstudio.svg", Tint::Mono)
        );
        assert_eq!(
            logo("openai"),
            Logo::Mark("tern/brands/openai.svg", Tint::Mono)
        );
        assert_eq!(
            logo("Brave Search"),
            Logo::Mark("tern/brands/brave.svg", Tint::Original)
        );
        assert_eq!(
            logo("wikipedia"),
            Logo::Mark("tern/service/wikipedia.svg", Tint::Mono)
        );
        for id in [
            "anthropic",
            "groq",
            "openrouter",
            "xai",
            "deepseek",
            "ollama",
            "tavily",
            "searxng",
        ] {
            assert!(matches!(logo(id), Logo::Mark(..)), "{id}");
        }
    }

    #[test]
    fn unknown_and_custom_services_fall_back_to_their_initial() {
        assert_eq!(logo("Together AI"), Logo::Initial("T".into()));
        assert_eq!(logo("my-vllm"), Logo::Initial("M".into()));
        assert_eq!(logo(""), Logo::Initial(String::new()));
    }

    #[test]
    fn every_mark_resolves_and_is_an_svg() {
        for p in paths() {
            let bytes = lookup(p).unwrap_or_else(|| panic!("{p}"));
            let text = std::str::from_utf8(bytes).unwrap();
            assert!(text.starts_with("<svg"), "{p}");
            assert!(!text.contains("1em"), "{p} keeps a relative size");
        }
        assert_eq!(paths().count(), BRANDS.len());
        assert!(lookup("tern/brands/nope.svg").is_none());
    }

    #[test]
    fn colour_marks_carry_their_colours() {
        for p in paths() {
            let text = std::str::from_utf8(lookup(p).unwrap()).unwrap();
            let id = p.trim_start_matches(PREFIX).trim_end_matches(".svg");
            let tint = BRANDS.iter().find(|(n, _, _)| *n == id).unwrap().2;
            match tint {
                Tint::Original => assert!(
                    text.contains("fill=\"#")
                        || text.contains("fill=\"gold\"")
                        || text.contains("stop-color"),
                    "{p}"
                ),
                Tint::Mono => assert!(text.contains("currentColor"), "{p}"),
            }
        }
    }
}
