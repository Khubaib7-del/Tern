//! The built-in browser pane's state: history, the page on screen, and the
//! request in flight. GPUI-free so it is unit tested; pages are read
//! through `tern_agent::browse`.

use tern_agent::browse::{self, BrowsedPage, History};

/// What the browser pane shows: the live page (a real engine where the
/// build has one), Tern's explanation of it (the page read by the
/// crawler, search results, Tern's cited answer), or both side by side.
/// Page first (#68): a link opens chat | page half and half, so the
/// pane shows just the page; Explanation and Side by side stay as tabs.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum PaneView {
    #[default]
    Page,
    Explanation,
    Split,
}

/// The browser's share of the room beside the sidebar when it opens: half
/// (#68: chat | page 50/50); the divider drags it between the bounds.
pub const SPLIT_DEFAULT: f32 = 0.5;
pub const SPLIT_MIN: f32 = 0.3;
pub const SPLIT_MAX: f32 = 0.7;

/// The browser pane's width: its share of `room` (the window right of
/// the rail and any open sidebar) less the divider.
pub fn split_width(room: f32, share: f32) -> f32 {
    ((room - crate::panels::HANDLE_W).max(0.) * share.clamp(SPLIT_MIN, SPLIT_MAX)).floor()
}

/// The share the divider dragged to `x`, in a room from `left` to the
/// window's right edge `right`.
pub fn split_share(x: f32, left: f32, right: f32) -> f32 {
    let room = (right - left).max(1.);
    ((right - x) / room).clamp(SPLIT_MIN, SPLIT_MAX)
}

impl PaneView {
    pub const ALL: [PaneView; 3] = [PaneView::Page, PaneView::Explanation, PaneView::Split];

    pub fn label(self) -> &'static str {
        match self {
            Self::Page => "Page",
            Self::Explanation => "Explanation",
            Self::Split => "Side by side",
        }
    }

    pub fn index(self) -> usize {
        Self::ALL.iter().position(|v| *v == self).unwrap_or(0)
    }

    pub fn shows_page(self) -> bool {
        self != Self::Explanation
    }

    pub fn shows_explanation(self) -> bool {
        self != Self::Page
    }
}

/// A browser tab that is not the one showing.
#[derive(Debug, Default)]
pub struct Parked {
    pub history: History,
    pub page: Option<BrowsedPage>,
    pub title: String,
}

/// The address the live page loads for a history address: the page
/// itself, or the words on a web search (the explanation side lists the
/// search service's results for the same words).
pub fn engine_address(address: &str) -> String {
    match browse::search_query(address) {
        Some(q) => {
            let enc: String = q
                .bytes()
                .map(|b| match b {
                    b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                        (b as char).to_string()
                    }
                    b' ' => "+".to_owned(),
                    _ => format!("%{b:02X}"),
                })
                .collect();
            format!("https://duckduckgo.com/?q={enc}")
        }
        None => address.to_owned(),
    }
}

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
    /// Page, Explanation, or side by side.
    pub view: PaneView,
    /// Every tab; the showing one's slot is empty while it shows (its
    /// state is in the fields above).
    pub tabs: Vec<Parked>,
    pub tab: usize,
    /// The showing tab's title as the live page reports it.
    pub title: String,
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

    /// The live page moved on its own (a link clicked inside it): the
    /// history and the explanation follow. `None` when it is where Tern
    /// already is (or a page Tern cannot read, like `about:blank`).
    pub fn engine_moved(&mut self, url: &str) -> Option<(u64, String)> {
        if !(url.starts_with("http://") || url.starts_with("https://")) {
            return None;
        }
        let here = self.history.current().map(engine_address);
        if here.as_deref() == Some(url)
            || self.loading.as_deref() == Some(url)
            || self.page.as_ref().is_some_and(|p| p.page.url == url)
        {
            return None;
        }
        self.history.visit(url);
        Some(self.load(url.to_owned()))
    }

    /// Whether the showing tab is a search rather than a page.
    pub fn is_search(&self) -> bool {
        self.history
            .current()
            .is_some_and(|a| browse::search_query(a).is_some())
    }

    /// The tab strip: one title per tab, the showing one's live.
    pub fn tab_titles(&self) -> Vec<String> {
        let live = if !self.title.is_empty() {
            self.title.clone()
        } else if let Some(p) = &self.page {
            p.page.label().to_owned()
        } else {
            let a = self.address();
            if a.is_empty() { "New tab".into() } else { a }
        };
        if self.tabs.is_empty() {
            return vec![live];
        }
        self.tabs
            .iter()
            .enumerate()
            .map(|(i, t)| {
                if i == self.tab {
                    live.clone()
                } else if t.title.is_empty() {
                    "New tab".into()
                } else {
                    t.title.clone()
                }
            })
            .collect()
    }

    fn park(&mut self) -> Parked {
        let title = self.tab_titles().get(self.tab).cloned().unwrap_or_default();
        Parked {
            history: std::mem::take(&mut self.history),
            page: self.page.take(),
            title,
        }
    }

    fn show(&mut self, p: Parked) {
        self.history = p.history;
        self.page = p.page;
        self.title = String::new();
        self.loading = None;
        self.error = None;
        // A slow load for the tab left behind is dropped.
        self.request += 1;
    }

    /// Opens a new, empty tab and shows it.
    pub fn new_tab(&mut self) {
        if self.tabs.is_empty() {
            self.tabs.push(Parked::default());
        }
        let here = self.park();
        self.tabs[self.tab] = here;
        self.tabs.push(Parked::default());
        self.tab = self.tabs.len() - 1;
        self.show(Parked::default());
        self.visible = true;
    }

    /// Shows tab `i`. Returns the address its page should load.
    pub fn switch_tab(&mut self, i: usize) -> Option<String> {
        if i == self.tab || i >= self.tabs.len() {
            return None;
        }
        let here = self.park();
        self.tabs[self.tab] = here;
        let next = std::mem::take(&mut self.tabs[i]);
        self.tab = i;
        self.show(next);
        self.history.current().map(engine_address)
    }

    /// Closes tab `i`; the last tab cannot close (the pane closes instead).
    /// Returns the address the page should load when the showing tab
    /// changed.
    pub fn close_tab(&mut self, i: usize) -> Option<String> {
        if self.tabs.len() <= 1 || i >= self.tabs.len() {
            return None;
        }
        if i == self.tab {
            self.tabs.remove(i);
            let to = i.min(self.tabs.len() - 1);
            let next = std::mem::take(&mut self.tabs[to]);
            self.tab = to;
            self.show(next);
            self.history.current().map(engine_address)
        } else {
            self.tabs.remove(i);
            if i < self.tab {
                self.tab -= 1;
            }
            None
        }
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

    #[test]
    fn tabs_keep_their_own_pages_and_history() {
        let mut b = BrowserState::default();
        let (r, _) = b.navigate("x.org/a").unwrap();
        b.loaded(r, Ok(page("https://x.org/a")));
        assert_eq!(b.tab_titles(), ["T"]);
        b.new_tab();
        assert_eq!(b.tabs.len(), 2);
        assert_eq!(b.tab, 1);
        assert!(b.page.is_none() && b.history.current().is_none());
        let (r, _) = b.navigate("rust lang").unwrap();
        assert!(b.is_search());
        b.loaded(r, Ok(page("tern-search:rust lang")));
        assert_eq!(b.switch_tab(0).as_deref(), Some("https://x.org/a"));
        assert_eq!(b.page.as_ref().unwrap().page.url, "https://x.org/a");
        assert!(!b.is_search());
        assert_eq!(b.tab_titles().len(), 2);
        assert_eq!(
            b.switch_tab(1).as_deref(),
            Some("https://duckduckgo.com/?q=rust+lang")
        );
        assert_eq!(b.close_tab(1).as_deref(), Some("https://x.org/a"));
        assert_eq!((b.tabs.len(), b.tab), (1, 0));
        assert_eq!(b.close_tab(0), None, "the last tab stays");
    }

    #[test]
    fn the_live_page_leads_and_the_explanation_follows() {
        let mut b = BrowserState::default();
        let (r, _) = b.navigate("x.org/a").unwrap();
        b.loaded(r, Ok(page("https://x.org/a")));
        assert_eq!(b.engine_moved("https://x.org/a"), None);
        assert_eq!(b.engine_moved("about:blank"), None);
        let (_, u) = b.engine_moved("https://x.org/next").unwrap();
        assert_eq!(u, "https://x.org/next");
        assert_eq!(b.history.current(), Some("https://x.org/next"));
        assert!(b.history.can_back());
        assert_eq!(engine_address("https://x.org/"), "https://x.org/");
        assert_eq!(
            engine_address("tern-search:c++ & rust"),
            "https://duckduckgo.com/?q=c%2B%2B+%26+rust"
        );
        assert_eq!(PaneView::default(), PaneView::Page);
        // Half and half: the chat and the page get the same width.
        let room = 1228.;
        let w = split_width(room, SPLIT_DEFAULT);
        assert_eq!(w, ((room - crate::panels::HANDLE_W) / 2.).floor());
        assert!((room - crate::panels::HANDLE_W - w - w).abs() <= 1.);
        assert_eq!(split_share(52. + 614., 52., 1280.), 0.5);
        assert_eq!(split_share(100., 52., 1280.), SPLIT_MAX);
        assert_eq!(split_share(1270., 52., 1280.), SPLIT_MIN);
        assert!(PaneView::Split.shows_page() && PaneView::Split.shows_explanation());
        assert!(!PaneView::Page.shows_explanation());
    }
}
