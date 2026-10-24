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

/// One bundled mark: its id, asset path, bytes, and how it is drawn.
macro_rules! brand {
    ($id:literal, $tint:ident) => {
        (
            $id,
            concat!("tern/brands/", $id, ".svg"),
            include_bytes!(concat!("../assets/brands/", $id, ".svg")) as &[u8],
            Tint::$tint,
        )
    };
}

/// `(id, asset path, asset bytes, tint)` for every bundled brand mark.
/// Colour marks keep their brand colours; `Mono` is only for brands whose
/// mark is one colour (OpenAI, xAI, Ollama…), drawn at full contrast in
/// the theme's ink so it never fades into the tile.
const BRANDS: &[(&str, &str, &[u8], Tint)] = &[
    brand!("anthropic", Original),
    brand!("brave", Original),
    brand!("cerebras", Original),
    brand!("context7", Original),
    brand!("deepseek", Original),
    brand!("figma", Original),
    brand!("firecrawl", Original),
    brand!("fireworks", Original),
    brand!("github", Mono),
    brand!("google", Original),
    brand!("groq", Original),
    brand!("jev", Original),
    brand!("jina", Original),
    brand!("lmstudio", Original),
    brand!("mcp", Mono),
    brand!("mistral", Original),
    brand!("notion", Mono),
    brand!("ollama", Mono),
    brand!("openai", Mono),
    brand!("openrouter", Mono),
    brand!("searxng", Original),
    brand!("subq", Original),
    brand!("tavily", Original),
    brand!("together", Original),
    brand!("voyage", Mono),
    brand!("xai", Mono),
];

/// Where brand marks live in the asset source.
const PREFIX: &str = "tern/brands/";

/// The canonical id for a provider, service, or MCP template id, or a
/// display name ("Google AI Studio", "LM Studio", "Brave Search").
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
        "together" | "togetherai" => "together",
        "fireworks" | "fireworksai" => "fireworks",
        "cerebras" => "cerebras",
        "tavily" => "tavily",
        "brave" | "bravesearch" => "brave",
        "searxng" => "searxng",
        "wikipedia" => "wikipedia",
        "firecrawl" => "firecrawl",
        "jina" | "jinareader" | "jinaai" => "jina",
        "jev" | "typesafe" | "typesafejev" | "typesafeai" => "jev",
        "subq" | "subqsnap" | "subquadratic" | "subquadraticsubqsnap" => "subq",
        "voyage" | "voyageai" | "voyagererank" | "voyageairerank" => "voyage",
        "context7" => "context7",
        "github" => "github",
        "notion" => "notion",
        "figma" => "figma",
        // The reference servers from the MCP project carry its mark.
        "mcp" | "filesystem" | "fetch" | "memory" | "sequentialthinking" | "time" | "git" => "mcp",
        _ => return None,
    })
}

/// The logo for a provider or search service, by id or name.
pub fn logo(id_or_name: &str) -> Logo {
    match canonical(id_or_name) {
        // Wikipedia's W is the one-colour service logo Tern already ships.
        Some("wikipedia") => Logo::Mark("tern/service/wikipedia.svg", Tint::Mono),
        Some(id) => match BRANDS.iter().find(|(n, _, _, _)| *n == id) {
            Some((_, path, _, tint)) => Logo::Mark(path, *tint),
            None => Logo::Initial(initial(id_or_name)),
        },
        None => Logo::Initial(initial(id_or_name)),
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
    BRANDS
        .iter()
        .find(|(n, _, _, _)| *n == id)
        .map(|(_, _, b, _)| *b)
}

/// Every brand asset path.
pub fn paths() -> impl Iterator<Item = &'static str> {
    BRANDS.iter().map(|(_, p, _, _)| *p)
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
            "together",
            "Fireworks AI",
            "cerebras",
            "firecrawl",
            "Jina Reader",
            "TypeSafe Jev",
            "Subquadratic SubQ Snap",
            "Voyage AI Rerank",
            "context7",
            "github",
            "filesystem",
            "sequential-thinking",
        ] {
            assert!(matches!(logo(id), Logo::Mark(..)), "{id}");
        }
    }

    #[test]
    fn unknown_and_custom_services_fall_back_to_their_initial() {
        assert_eq!(
            logo("Together AI"),
            Logo::Mark("tern/brands/together.svg", Tint::Original)
        );
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
            let tint = BRANDS.iter().find(|(n, _, _, _)| *n == id).unwrap().3;
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
