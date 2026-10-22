//! Session tabs and the responsive layout (`docs/02-ui-definition.md`,
//! shell). GPUI-free so it is unit tested.
//!
//! | Window width | Sidebar | Run panel |
//! | --- | --- | --- |
//! | 1100 px and wider | Beside the chat | Beside the chat |
//! | 900 to 1099 px | Beside the chat | Over the chat |
//! | Under 900 px | Over the chat, closed when the window narrows | Over the chat |

use tern_core::SessionId;

/// Narrower than this, the sidebar floats over the chat.
pub const SIDEBAR_MIN_W: f32 = 900.;
/// Narrower than this, the run panel floats over the chat.
pub const PANEL_MIN_W: f32 = 1100.;
/// Narrower than this, the title bar drops its labels.
pub const COMPACT_W: f32 = 640.;

/// Where the side panels go at one window width.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Layout {
    pub sidebar_beside: bool,
    pub panel_beside: bool,
    pub compact: bool,
}

impl Layout {
    pub fn for_width(width: f32) -> Self {
        Self {
            sidebar_beside: width >= SIDEBAR_MIN_W,
            panel_beside: width >= PANEL_MIN_W,
            compact: width < COMPACT_W,
        }
    }

    /// Whether the window just became too narrow for the sidebar, so it
    /// should close instead of covering the chat.
    pub fn sidebar_squeezed(before: Option<Self>, now: Self) -> bool {
        before.is_some_and(|b| b.sidebar_beside) && !now.sidebar_beside
    }
}

/// One open chat, or an Explain in 3D view. `id` is `None` until a fresh
/// chat has its session; `explain` names an explainer's state.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Tab {
    pub id: Option<SessionId>,
    pub title: String,
    pub explain: Option<u32>,
}

impl Tab {
    fn fresh() -> Self {
        Self {
            id: None,
            title: "New chat".into(),
            explain: None,
        }
    }

    pub fn is_explain(&self) -> bool {
        self.explain.is_some()
    }
}

/// The open chats, one of them active.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Tabs {
    pub list: Vec<Tab>,
    pub active: usize,
}

impl Default for Tabs {
    fn default() -> Self {
        Self {
            list: vec![Tab::fresh()],
            active: 0,
        }
    }
}

impl Tabs {
    pub fn active_tab(&self) -> &Tab {
        &self.list[self.active]
    }

    /// Opens a chat from the list: focuses its tab when one is open,
    /// otherwise shows it in the active tab. Returns true when the chat
    /// must be loaded.
    pub fn open(&mut self, id: SessionId, title: &str) -> bool {
        if let Some(i) = self.list.iter().position(|t| t.id == Some(id)) {
            let load = i != self.active;
            self.active = i;
            return load;
        }
        // An explainer keeps its tab; the chat opens beside it.
        if self.active_tab().is_explain() {
            self.add();
        }
        self.list[self.active] = Tab {
            id: Some(id),
            title: title.to_owned(),
            explain: None,
        };
        true
    }

    /// Opens an explainer in a new tab after the active one and focuses it.
    pub fn open_explain(&mut self, key: u32, title: &str) {
        self.active += 1;
        self.list.insert(
            self.active,
            Tab {
                id: None,
                title: title.to_owned(),
                explain: Some(key),
            },
        );
    }

    /// An explainer's title changed (its scene arrived).
    pub fn retitle_explain(&mut self, key: u32, title: &str) {
        for t in self.list.iter_mut().filter(|t| t.explain == Some(key)) {
            t.title = title.to_owned();
        }
    }

    /// The chat tab to show when leaving an explainer: the nearest one
    /// before it, else after it.
    pub fn nearest_chat(&self) -> Option<usize> {
        (0..self.active)
            .rev()
            .chain(self.active + 1..self.list.len())
            .find(|&i| !self.list[i].is_explain())
    }

    /// Adds a fresh tab after the active one and focuses it.
    pub fn add(&mut self) {
        self.active += 1;
        self.list.insert(self.active, Tab::fresh());
    }

    /// Focuses tab `i`. Returns it when the chat shown must change.
    pub fn focus(&mut self, i: usize) -> Option<&Tab> {
        if i >= self.list.len() || i == self.active {
            return None;
        }
        self.active = i;
        Some(&self.list[i])
    }

    /// Closes tab `i`. Returns the tab to show when the active one closed.
    /// The last tab never closes; it turns into a fresh chat instead.
    pub fn close(&mut self, i: usize) -> Option<&Tab> {
        if i >= self.list.len() {
            return None;
        }
        if self.list.len() == 1 {
            self.list[0] = Tab::fresh();
            return Some(&self.list[0]);
        }
        self.list.remove(i);
        if i < self.active {
            self.active -= 1;
            None
        } else if i == self.active {
            self.active = self.active.min(self.list.len() - 1);
            Some(&self.list[self.active])
        } else {
            None
        }
    }

    /// The active tab's chat now has a session (a fresh chat connected, or
    /// a branch opened).
    pub fn set_active(&mut self, id: SessionId, title: &str) {
        // A chat that connects while an explainer is up takes the nearest
        // chat tab instead.
        let i = if self.active_tab().is_explain() {
            match self.nearest_chat() {
                Some(i) => i,
                None => return,
            }
        } else {
            self.active
        };
        let tab = &mut self.list[i];
        tab.id = Some(id);
        tab.title = title.to_owned();
    }

    /// The active tab turns into a fresh chat.
    pub fn reset_active(&mut self) {
        self.list[self.active] = Tab::fresh();
    }

    /// A chat was renamed or titled.
    pub fn retitle(&mut self, id: SessionId, title: &str) {
        for t in self.list.iter_mut().filter(|t| t.id == Some(id)) {
            t.title = title.to_owned();
        }
    }

    /// A chat was archived: its tabs close.
    pub fn forget(&mut self, id: SessionId) -> Option<&Tab> {
        let i = self.list.iter().position(|t| t.id == Some(id))?;
        self.close(i)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn explainers_get_their_own_tabs() {
        let mut t = Tabs::default();
        t.open_explain(7, "3D · TLB");
        assert_eq!(t.active, 1);
        assert!(t.active_tab().is_explain());
        assert_eq!(t.nearest_chat(), Some(0));
        t.retitle_explain(7, "3D · Paging");
        assert_eq!(t.list[1].title, "3D · Paging");
        // Opening a chat from the list leaves the explainer alone.
        let id = SessionId::new();
        assert!(t.open(id, "Chat"));
        assert_eq!(t.list.len(), 3);
        assert_eq!(t.list[1].explain, Some(7));
        assert_eq!(t.active_tab().id, Some(id));
        // A chat connecting under an explainer lands on a chat tab.
        t.focus(1);
        let other = SessionId::new();
        t.set_active(other, "Fresh");
        assert_eq!(t.list[0].id, Some(other));
        assert!(t.list[1].is_explain());
    }

    #[test]
    fn layout_by_width() {
        let wide = Layout::for_width(1400.);
        assert!(wide.sidebar_beside && wide.panel_beside && !wide.compact);
        let mid = Layout::for_width(1000.);
        assert!(mid.sidebar_beside && !mid.panel_beside);
        let small = Layout::for_width(600.);
        assert!(!small.sidebar_beside && !small.panel_beside && small.compact);
        assert!(Layout::sidebar_squeezed(Some(wide), small));
        assert!(!Layout::sidebar_squeezed(Some(small), small));
        assert!(!Layout::sidebar_squeezed(None, small));
    }

    #[test]
    fn tabs_open_focus_and_close() {
        let (a, b) = (SessionId::new(), SessionId::new());
        let mut t = Tabs::default();
        t.set_active(a, "RAG");
        t.add();
        assert_eq!(t.active, 1);
        assert!(t.open(b, "Transformers"));
        assert_eq!(t.list.len(), 2);
        // Opening a chat that has a tab focuses it.
        assert!(t.open(a, "RAG"));
        assert_eq!(t.active, 0);
        assert!(!t.open(a, "RAG"));
        t.retitle(b, "Attention");
        assert_eq!(t.list[1].title, "Attention");
        // Closing a tab behind the active one keeps the chat shown.
        assert_eq!(t.focus(1).map(|x| x.id), Some(Some(b)));
        assert!(t.close(0).is_none());
        assert_eq!(t.active, 0);
        assert_eq!(t.active_tab().id, Some(b));
        // The last tab turns fresh instead of closing.
        assert_eq!(t.close(0).map(|x| x.id), Some(None));
        assert_eq!(t.list.len(), 1);
    }

    #[test]
    fn closing_the_active_tab_shows_its_neighbour() {
        let ids: Vec<_> = (0..3).map(|_| SessionId::new()).collect();
        let mut t = Tabs::default();
        t.set_active(ids[0], "a");
        for (i, id) in ids.iter().enumerate().skip(1) {
            t.add();
            t.set_active(*id, &i.to_string());
        }
        assert_eq!(t.active, 2);
        assert_eq!(t.close(2).map(|x| x.id), Some(Some(ids[1])));
        assert_eq!(t.forget(ids[0]).map(|x| x.id), None);
        assert_eq!(t.list.len(), 1);
        assert_eq!(t.active_tab().id, Some(ids[1]));
    }
}
