//! The app's embedded assets: Tern's own (the mark, the glyph set, the
//! built-in service logos, the fonts) in front of the kit's full Lucide
//! catalog. See `docs/decisions/0002-palette-type-glyphs.md`.

use std::borrow::Cow;

use gpui_kit::{App, AssetSource, SharedString};

/// The tern mark as one-colour SVG; the view tints and animates it.
pub const MARK: &str = "tern/mark.svg";
const MARK_SVG: &[u8] = include_bytes!("../assets/tern-mark.svg");
/// The Tern Crawl logo in colour (cyan-to-blue, grained), drawn with `img`
/// rather than `svg` so its gradient survives. The one-colour version is
/// the `tern-crawl` glyph.
pub const CRAWL_LOGO: &str = "tern/brand/tern-crawl.svg";
const CRAWL_LOGO_SVG: &[u8] = include_bytes!("../assets/brand/tern-crawl.svg");

/// Tern's own glyphs, 16 px, 1.5 px stroke, drawn in `currentColor`.
const GLYPHS: &[(&str, &[u8])] = &[
    (
        "branch-reply",
        include_bytes!("../assets/glyphs/branch-reply.svg"),
    ),
    (
        "checkpoint",
        include_bytes!("../assets/glyphs/checkpoint.svg"),
    ),
    ("research", include_bytes!("../assets/glyphs/research.svg")),
    ("steer", include_bytes!("../assets/glyphs/steer.svg")),
    ("study", include_bytes!("../assets/glyphs/study.svg")),
    (
        "tern-crawl",
        include_bytes!("../assets/glyphs/tern-crawl.svg"),
    ),
    (
        "verdict-contested",
        include_bytes!("../assets/glyphs/verdict-contested.svg"),
    ),
    (
        "verdict-partial",
        include_bytes!("../assets/glyphs/verdict-partial.svg"),
    ),
    (
        "verdict-supported",
        include_bytes!("../assets/glyphs/verdict-supported.svg"),
    ),
    (
        "verdict-unsupported",
        include_bytes!("../assets/glyphs/verdict-unsupported.svg"),
    ),
    (
        "verdict-unverified",
        include_bytes!("../assets/glyphs/verdict-unverified.svg"),
    ),
    ("watcher", include_bytes!("../assets/glyphs/watcher.svg")),
];

/// Logos for services Tern knows, one colour (Simple Icons, CC0 data;
/// the marks belong to their owners).
const SERVICES: &[(&str, &[u8])] = &[
    ("arxiv", include_bytes!("../assets/services/arxiv.svg")),
    ("brave", include_bytes!("../assets/services/brave.svg")),
    // Decision-model providers, traced from their own marks in one colour.
    ("subq", include_bytes!("../assets/services/subq.svg")),
    (
        "typesafe",
        include_bytes!("../assets/services/typesafe.svg"),
    ),
    ("voyage", include_bytes!("../assets/services/voyage.svg")),
    ("doi", include_bytes!("../assets/services/doi.svg")),
    (
        "duckduckgo",
        include_bytes!("../assets/services/duckduckgo.svg"),
    ),
    ("github", include_bytes!("../assets/services/github.svg")),
    (
        "googlescholar",
        include_bytes!("../assets/services/googlescholar.svg"),
    ),
    ("orcid", include_bytes!("../assets/services/orcid.svg")),
    ("pubmed", include_bytes!("../assets/services/pubmed.svg")),
    (
        "researchgate",
        include_bytes!("../assets/services/researchgate.svg"),
    ),
    (
        "semanticscholar",
        include_bytes!("../assets/services/semanticscholar.svg"),
    ),
    (
        "wikipedia",
        include_bytes!("../assets/services/wikipedia.svg"),
    ),
    ("zotero", include_bytes!("../assets/services/zotero.svg")),
];

/// The UI, mono, and serif faces (SIL Open Font License 1.1).
pub const UI_FONT: &str = "IBM Plex Sans";
pub const MONO_FONT: &str = "IBM Plex Mono";
pub const SERIF_FONT: &str = "Instrument Serif";
const FONTS: &[&[u8]] = &[
    include_bytes!("../assets/fonts/IBMPlexSans-Regular.ttf"),
    include_bytes!("../assets/fonts/IBMPlexSans-Medium.ttf"),
    include_bytes!("../assets/fonts/IBMPlexSans-SemiBold.ttf"),
    include_bytes!("../assets/fonts/IBMPlexSans-Italic.ttf"),
    include_bytes!("../assets/fonts/IBMPlexMono-Regular.ttf"),
    include_bytes!("../assets/fonts/InstrumentSerif-Italic.ttf"),
];

/// Registers the bundled fonts. A failure leaves the system font in place.
pub fn register_fonts(cx: &mut App) {
    let fonts = FONTS.iter().map(|f| Cow::Borrowed(*f)).collect();
    if let Err(e) = cx.text_system().add_fonts(fonts) {
        eprintln!("Tern could not load its fonts: {e}");
    }
}

/// The asset path of one of Tern's glyphs, if `name` is one.
pub fn glyph(name: &str) -> Option<SharedString> {
    GLYPHS
        .iter()
        .any(|(n, _)| *n == name)
        .then(|| format!("tern/glyph/{name}.svg").into())
}

/// Every glyph name, for the Appearance screen.
pub fn glyph_names() -> impl Iterator<Item = &'static str> {
    GLYPHS.iter().map(|(n, _)| *n)
}

/// Every built-in service logo name.
pub fn service_names() -> impl Iterator<Item = &'static str> {
    SERVICES.iter().map(|(n, _)| *n)
}

/// The built-in logo for a site, by its host, when Tern knows the service.
pub fn service_for(site: &str) -> Option<&'static str> {
    let host = site.trim().trim_start_matches("www.").to_ascii_lowercase();
    let is = |d: &str| host == d || host.ends_with(&format!(".{d}"));
    let name = if is("github.com") || is("githubusercontent.com") {
        "github"
    } else if is("arxiv.org") {
        "arxiv"
    } else if is("wikipedia.org") {
        "wikipedia"
    } else if is("doi.org") {
        "doi"
    } else if is("semanticscholar.org") {
        "semanticscholar"
    } else if host == "pubmed.ncbi.nlm.nih.gov" || is("ncbi.nlm.nih.gov") {
        "pubmed"
    } else if is("search.brave.com") || is("brave.com") {
        "brave"
    } else if is("duckduckgo.com") {
        "duckduckgo"
    } else if host == "scholar.google.com" {
        "googlescholar"
    } else if is("orcid.org") {
        "orcid"
    } else if is("zotero.org") {
        "zotero"
    } else if is("researchgate.net") {
        "researchgate"
    } else {
        return None;
    };
    Some(name)
}

/// The asset path of a service logo.
pub fn service_path(name: &str) -> SharedString {
    format!("tern/service/{name}.svg").into()
}

fn lookup(path: &str) -> Option<&'static [u8]> {
    if path == MARK {
        return Some(MARK_SVG);
    }
    if path == CRAWL_LOGO {
        return Some(CRAWL_LOGO_SVG);
    }
    let find = |set: &'static [(&'static str, &'static [u8])], prefix: &str| {
        let name = path.strip_prefix(prefix)?.strip_suffix(".svg")?;
        set.iter().find(|(n, _)| *n == name).map(|(_, b)| *b)
    };
    find(GLYPHS, "tern/glyph/")
        .or_else(|| find(SERVICES, "tern/service/"))
        .or_else(|| crate::brands::lookup(path))
}

#[derive(Debug)]
pub struct TernAssets;

impl AssetSource for TernAssets {
    fn load(&self, path: &str) -> gpui_kit::Result<Option<Cow<'static, [u8]>>> {
        if let Some(bytes) = lookup(path) {
            return Ok(Some(Cow::Borrowed(bytes)));
        }
        gpui_kit::assets::AllAssets.load(path)
    }

    fn list(&self, path: &str) -> gpui_kit::Result<Vec<SharedString>> {
        let mut out = gpui_kit::assets::AllAssets.list(path)?;
        let ours = [SharedString::from(MARK), SharedString::from(CRAWL_LOGO)]
            .into_iter()
            .chain(
                GLYPHS
                    .iter()
                    .map(|(n, _)| format!("tern/glyph/{n}.svg").into()),
            )
            .chain(SERVICES.iter().map(|(n, _)| service_path(n)))
            .chain(crate::brands::paths().map(SharedString::from));
        out.extend(ours.filter(|p| p.starts_with(path)));
        Ok(out)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_asset_resolves() {
        for n in glyph_names() {
            let path = glyph(n).unwrap();
            assert!(lookup(&path).is_some(), "{path}");
        }
        for n in service_names() {
            assert!(lookup(&service_path(n)).is_some(), "{n}");
        }
        assert!(glyph("nope").is_none());
        assert!(lookup(MARK).is_some());
        assert!(lookup(CRAWL_LOGO).is_some());
    }

    #[test]
    fn known_sites_get_their_logo() {
        assert_eq!(service_for("github.com"), Some("github"));
        assert_eq!(service_for("en.wikipedia.org"), Some("wikipedia"));
        assert_eq!(service_for("www.arxiv.org"), Some("arxiv"));
        assert_eq!(service_for("pubmed.ncbi.nlm.nih.gov"), Some("pubmed"));
        assert_eq!(service_for("notgithub.com"), None);
        assert_eq!(service_for("example.org"), None);
        for n in [
            "github",
            "arxiv",
            "wikipedia",
            "doi",
            "pubmed",
            "brave",
            "duckduckgo",
        ] {
            assert!(service_names().any(|s| s == n), "{n}");
        }
    }
}
