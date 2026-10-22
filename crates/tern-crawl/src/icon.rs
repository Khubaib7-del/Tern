//! Which logo a source shows: a built-in icon for services Tern knows, or
//! the site's own favicon.
//!
//! Built-in ids name glyphs the front ends ship (`github`, `arxiv`). The
//! desktop app draws them; the TUI shows the site name instead.

use serde::Serialize;
use url::Url;

/// The built-in logo of Tern Crawl, Tern's own crawler and reader (used
/// for "Read 6 pages").
pub const TERN_CRAWL: &str = "tern-crawl";
/// The same logo under its earlier name.
pub const TERN_READER: &str = TERN_CRAWL;

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct SourceIcon {
    /// A glyph the app ships, when the service is known.
    pub builtin: Option<&'static str>,
    /// The page's declared icon, or `/favicon.ico` on its origin.
    pub favicon: Option<String>,
}

impl SourceIcon {
    /// The icon for a URL before its page is read: built-in or `/favicon.ico`.
    pub fn for_url(url: &Url) -> Self {
        Self {
            builtin: url.host_str().and_then(builtin_for_host),
            favicon: default_favicon(url),
        }
    }
}

const KNOWN: &[(&str, &str)] = &[
    ("github.com", "github"),
    ("githubusercontent.com", "github"),
    ("gitlab.com", "gitlab"),
    ("arxiv.org", "arxiv"),
    ("wikipedia.org", "wikipedia"),
    ("doi.org", "doi"),
    ("semanticscholar.org", "semantic-scholar"),
    ("openalex.org", "openalex"),
    ("crossref.org", "crossref"),
    ("pubmed.ncbi.nlm.nih.gov", "pubmed"),
    ("ncbi.nlm.nih.gov", "ncbi"),
    ("openreview.net", "openreview"),
    ("huggingface.co", "hugging-face"),
    ("stackoverflow.com", "stack-exchange"),
    ("stackexchange.com", "stack-exchange"),
    ("youtube.com", "youtube"),
    ("youtu.be", "youtube"),
    ("reddit.com", "reddit"),
    ("medium.com", "medium"),
    ("docs.rs", "docs-rs"),
    ("crates.io", "crates-io"),
    ("npmjs.com", "npm"),
    ("pypi.org", "pypi"),
];

/// The built-in glyph for a host, matching the domain or any subdomain of it.
pub fn builtin_for_host(host: &str) -> Option<&'static str> {
    let host = host.trim_end_matches('.').to_ascii_lowercase();
    if host.starts_with("scholar.google.") {
        return Some("google-scholar");
    }
    KNOWN
        .iter()
        .find(|(d, _)| host == *d || host.ends_with(&format!(".{d}")))
        .map(|(_, id)| *id)
}

/// The site as people say it: host without `www.`.
pub fn site_of(url: &Url) -> String {
    let host = url.host_str().unwrap_or_default();
    host.strip_prefix("www.").unwrap_or(host).to_owned()
}

fn default_favicon(url: &Url) -> Option<String> {
    let mut u = url.clone();
    u.set_path("/favicon.ico");
    u.set_query(None);
    u.set_fragment(None);
    Some(u.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn known_services_get_builtins_and_subdomains_match() {
        assert_eq!(builtin_for_host("github.com"), Some("github"));
        assert_eq!(
            builtin_for_host("raw.githubusercontent.com"),
            Some("github")
        );
        assert_eq!(builtin_for_host("en.wikipedia.org"), Some("wikipedia"));
        assert_eq!(
            builtin_for_host("scholar.google.com.pk"),
            Some("google-scholar")
        );
        assert_eq!(builtin_for_host("notgithub.com"), None);
        assert_eq!(builtin_for_host("example.org"), None);
    }

    #[test]
    fn unknown_sites_fall_back_to_their_favicon() {
        let u = Url::parse("https://www.example.org/a/b?q=1#x").unwrap();
        let i = SourceIcon::for_url(&u);
        assert_eq!(i.builtin, None);
        assert_eq!(
            i.favicon.as_deref(),
            Some("https://www.example.org/favicon.ico")
        );
        assert_eq!(site_of(&u), "example.org");
    }
}
