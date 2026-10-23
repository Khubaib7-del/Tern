//! The built-in browser pane's state: history, the page on screen, and the
//! request in flight. GPUI-free so it is unit tested; pages are read
//! through `tern_agent::browse`.

use tern_agent::browse::{self, BrowsedPage, History};

#[derive(Debug, Default)]
pub struct BrowserState {
    /// The pane is showing (it takes the run panel's place).
    pub visible: bool,
    pub history: History,
    pub page: Option<BrowsedPage>,
    /// The address being loaded.
    pub loading: Option<String>,
    pub error: Option<String>,
    /// Numbers each load so a slow, older page never replaces a newer one.
    request: u64,
}

impl BrowserState {
    /// Starts loading what was typed or clicked. Returns the request to
    /// send, or `None` (with `error` set) when it is not an address.
    pub fn navigate(&mut self, input: &str) -> Option<(u64, String)> {
        // A web address opens; other words search with the chat's search.
        let url = match browse::target(input) {
            Some(browse::Target::Page(url)) => url,
            Some(browse::Target::Search(q)) => browse::search_address(&q),
            None => {
                self.error =
                    Some("Type a web address, like arxiv.org, or words to search for.".into());
                return None;
            }
        };
        self.visible = true;
        self.history.visit(&url);
        Some(self.load(url))
    }

    pub fn back(&mut self) -> Option<(u64, String)> {
        let url = self.history.back()?.to_owned();
        Some(self.load(url))
    }

    pub fn forward(&mut self) -> Option<(u64, String)> {
        let url = self.history.forward()?.to_owned();
        Some(self.load(url))
    }

    pub fn reload(&mut self) -> Option<(u64, String)> {
        let url = self.history.current()?.to_owned();
        Some(self.load(url))
    }

    fn load(&mut self, url: String) -> (u64, String) {
        self.request += 1;
        self.loading = Some(url.clone());
        self.error = None;
        (self.request, url)
    }

    /// A load finished. Stale answers are dropped. Returns whether it was
    /// used.
    pub fn loaded(&mut self, request: u64, result: Result<BrowsedPage, String>) -> bool {
        if request != self.request {
            return false;
        }
        self.loading = None;
        match result {
            Ok(page) => {
                self.history.replace(&page.page.url);
                self.page = Some(page);
                self.error = None;
            }
            Err(e) => self.error = Some(e),
        }
        true
    }

    /// The address to show in the bar.
    pub fn address(&self) -> String {
        self.loading
            .clone()
            .or_else(|| self.page.as_ref().map(|p| p.page.url.clone()))
            .map(|a| browse::display_address(&a))
            .unwrap_or_default()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tern_agent::sources::PageRef;

    fn page(url: &str) -> BrowsedPage {
        BrowsedPage {
            page: PageRef {
                title: "T".into(),
                url: url.into(),
                site: "x.org".into(),
                builtin: None,
                favicon: None,
            },
            markdown: "# T".into(),
            links: Vec::new(),
            truncated: false,
        }
    }

    #[test]
    fn loads_redirects_and_drops_stale_pages() {
        let mut b = BrowserState::default();
        assert!(b.navigate("   ").is_none());
        assert!(b.error.is_some() && !b.visible);
        // Words are a search, kept in history under a search address.
        let (_, s) = b.navigate("mistral le chat").unwrap();
        assert_eq!(s, "tern-search:mistral le chat");
        assert_eq!(b.address(), "mistral le chat");
        let (r1, u1) = b.navigate("x.org/a").unwrap();
        assert_eq!(u1, "https://x.org/a");
        assert!(b.visible && b.error.is_none());
        assert_eq!(b.address(), "https://x.org/a");
        let (r2, _) = b.navigate("x.org/b").unwrap();
        assert!(!b.loaded(r1, Ok(page("https://x.org/a"))));
        assert!(b.loaded(r2, Ok(page("https://x.org/b2"))));
        assert_eq!(b.address(), "https://x.org/b2");
        let (r3, back) = b.back().unwrap();
        assert_eq!(back, "https://x.org/a");
        assert!(b.loaded(r3, Err("Site down".into())));
        assert_eq!(b.error.as_deref(), Some("Site down"));
        let (_, fwd) = b.forward().unwrap();
        assert_eq!(fwd, "https://x.org/b2");
        assert!(b.reload().is_some());
    }
}
