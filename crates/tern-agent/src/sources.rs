//! Pages a turn read, for source rows in front ends.

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
}
