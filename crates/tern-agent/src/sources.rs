//! Pages a turn read, for source rows in front ends.

use std::path::PathBuf;

use serde::{Deserialize, Serialize};
use serde_json::Value;

/// One page the agent read, with what a source row needs to show it.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct PageRef {
    pub title: String,
    /// The final URL after redirects; citations link here.
    pub url: String,
    pub site: String,
    /// A glyph the app ships for a known service (see `tern_crawl::icon`).
    #[serde(default)]
    pub builtin: Option<String>,
    /// The page's declared icon, or `/favicon.ico` on its origin.
    #[serde(default)]
    pub favicon: Option<String>,
}

impl PageRef {
    /// The title to show, falling back to the site when a page had none.
    pub fn label(&self) -> &str {
        if self.title.trim().is_empty() {
            &self.site
        } else {
            &self.title
        }
    }
}

/// The page a `tool.executed` event carries, if any.
pub fn page_of(kind: &str, payload: &Value) -> Option<PageRef> {
    if kind != "tool.executed" {
        return None;
    }
    serde_json::from_value(payload.get("meta")?.get("page")?.clone()).ok()
}

/// Adds a page unless one with the same URL is already there.
pub fn push_unique(pages: &mut Vec<PageRef>, page: PageRef) {
    if !pages.iter().any(|p| p.url == page.url) {
        pages.push(page);
    }
}

/// A site's favicon as a file in `<data>/cache/icons/`, downloaded once
/// through the crawler's network guard. `None` when it cannot be had.
pub async fn cached_favicon(site: &str, url: &str) -> Option<PathBuf> {
    let name = icon_file_name(site)?;
    let dir = tern_platform::AppPaths::discover()
        .ok()?
        .data_dir
        .join("cache")
        .join("icons");
    let path = dir.join(name);
    if path.exists() {
        return Some(path);
    }
    let bytes = tern_crawl::Fetcher::new().fetch_icon(url).await.ok()?;
    std::fs::create_dir_all(&dir).ok()?;
    std::fs::write(&path, bytes).ok()?;
    Some(path)
}

/// A site reduced to safe filename characters.
fn icon_file_name(site: &str) -> Option<String> {
    let name: String = site
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '.' || c == '-' {
                c.to_ascii_lowercase()
            } else {
                '_'
            }
        })
        .collect();
    let name = name.trim_matches('.').to_owned();
    (!name.is_empty()).then_some(name)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn reads_pages_from_tool_events_only() {
        let payload = json!({"tool": "read_url", "meta": {"page": {
            "title": "", "url": "https://arxiv.org/abs/1", "site": "arxiv.org",
            "builtin": "arxiv", "favicon": "https://arxiv.org/favicon.ico"}}});
        let p = page_of("tool.executed", &payload).unwrap();
        assert_eq!(p.label(), "arxiv.org");
        assert_eq!(p.builtin.as_deref(), Some("arxiv"));
        assert_eq!(page_of("tool.failed", &payload), None);
        assert_eq!(
            page_of("tool.executed", &json!({"tool": "web_search"})),
            None
        );
        let mut pages = vec![p.clone()];
        push_unique(&mut pages, p);
        assert_eq!(pages.len(), 1);
    }

    #[test]
    fn icon_names_are_safe() {
        assert_eq!(icon_file_name("ArXiv.org").as_deref(), Some("arxiv.org"));
        assert_eq!(icon_file_name("../etc").as_deref(), Some("_etc"));
        assert_eq!(icon_file_name("a:b/c").as_deref(), Some("a_b_c"));
        assert_eq!(icon_file_name(".."), None);
    }
}
