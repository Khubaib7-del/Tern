//! Session tabs and the responsive layout (`docs/02-ui-definition.md`,
//! shell). GPUI-free so it is unit tested.
//!
//! | Window width | Sidebar | Run panel |
//! | --- | --- | --- |
//! | 1100 px and wider | Beside the chat | Beside the chat |
//! | 900 to 1099 px | Beside the chat | Over the chat |
//! | Under 900 px | Over the chat, closed when the window narrows | Over the chat |

use tern_agent::prefs::{LastChat, OpenTab, OpenTabs};
use tern_agent::{Harness, SessionSummary};
use tern_core::SessionId;

/// Narrower than this, the sidebar floats over the chat.
pub const SIDEBAR_MIN_W: f32 = 900.;
/// Narrower than this, the run panel floats over the chat.
pub const PANEL_MIN_W: f32 = 1100.;
/// Narrower than this, the title bar drops its labels.
pub const COMPACT_W: f32 = 640.;
/// Space between icons on the left rail; groups are split by a short rule.
pub const RAIL_GAP: f32 = 4.;

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
    /// Where this tab was left when another came to the front: its unsent
    /// draft and how far it was scrolled, put back when it returns.
    pub place: LastChat,
}

impl Tab {
    fn fresh() -> Self {
        Self {
            id: None,
            title: "New chat".into(),
            explain: None,
            place: LastChat::default(),
        }
    }

    fn chat(id: SessionId, title: &str) -> Self {
        Self {
            id: Some(id),
            title: title.to_owned(),
            explain: None,
            place: LastChat::default(),
        }
    }

    pub fn is_explain(&self) -> bool {
        self.explain.is_some()
    }

    fn titled(mut self, title: &str) -> Self {
        self.title = title.to_owned();
        self
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
        self.list[self.active] = Tab::chat(id, title);
        true
    }

    /// Opens an explainer in a new tab after the active one and focuses it.
    pub fn open_explain(&mut self, key: u32, title: &str) {
        self.active += 1;
        self.list.insert(
            self.active,
            Tab {
                explain: Some(key),
                ..Tab::fresh().titled(title)
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

    /// Keeps where the front tab was left (draft, scroll) before another
    /// tab comes to the front.
    pub fn stash_place(&mut self, place: LastChat) {
        let i = self.active;
        if !self.list[i].is_explain() {
            self.list[i].place = place;
        }
    }

    /// Opens a saved chat in a new tab after the front one, or focuses
    /// its tab when it is open. Returns true when the chat must load.
    pub fn open_new(&mut self, id: SessionId, title: &str) -> bool {
        if let Some(i) = self.list.iter().position(|t| t.id == Some(id)) {
            let load = i != self.active;
            self.active = i;
            return load;
        }
        self.active += 1;
        self.list.insert(self.active, Tab::chat(id, title));
        true
    }

    /// The chat tabs to save for the next launch. Explainers are not kept
    /// (their scenes are in the Library); the front tab maps to the
    /// nearest chat tab.
    pub fn to_saved(&self) -> OpenTabs {
        let mut tabs = Vec::new();
        let mut active = 0;
        for (i, t) in self.list.iter().enumerate() {
            if t.is_explain() {
                continue;
            }
            if i <= self.active {
                active = tabs.len();
            }
            tabs.push(OpenTab {
                chat: t.id.map(|id| id.to_string()),
                title: t.title.clone(),
            });
        }
        if tabs.is_empty() {
            return OpenTabs::default();
        }
        OpenTabs {
            active: active.min(tabs.len() - 1),
            tabs,
        }
    }

    /// The tabs a launch restores. Chats that no longer exist (deleted, or
    /// moved to the other harness) are dropped; `known` is this harness's
    /// chat list. Always at least one tab.
    pub fn from_saved(saved: &OpenTabs, known: &[SessionSummary]) -> Self {
        let mut list = Vec::new();
        let mut active = 0;
        for (i, t) in saved.tabs.iter().enumerate() {
            let tab = match t.chat.as_deref().and_then(|s| s.parse::<SessionId>().ok()) {
                Some(id) => match known.iter().find(|s| s.id == id) {
                    Some(s) => Tab::chat(id, &s.title),
                    None => continue,
                },
                None if t.chat.is_none() => Tab::fresh(),
                None => continue,
            };
            if list.iter().any(|x: &Tab| x.id.is_some() && x.id == tab.id) {
                continue;
            }
            if i <= saved.active {
                active = list.len();
            }
            list.push(tab);
        }
        if list.is_empty() {
            return Self::default();
        }
        Self {
            active: active.min(list.len() - 1),
            list,
        }
    }

    /// A chat was archived: its tabs close.
    pub fn forget(&mut self, id: SessionId) -> Option<&Tab> {
        let i = self.list.iter().position(|t| t.id == Some(id))?;
        self.close(i)
    }
}

/// The other harness's tabs while this one is in front, so Research and
/// Study each keep their own (matching each harness's own last place).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ParkedTabs {
    pub research: Option<Tabs>,
    pub study: Option<Tabs>,
}

impl ParkedTabs {
    fn slot(&mut self, h: Harness) -> &mut Option<Tabs> {
        match h {
            Harness::Research => &mut self.research,
            Harness::Study => &mut self.study,
        }
    }

    /// Parks `current` (harness `from`'s tabs) and returns harness `to`'s,
    /// or `None` when `to` has none yet (the caller restores them from the
    /// preferences or starts fresh).
    pub fn swap(&mut self, from: Harness, to: Harness, current: Tabs) -> Option<Tabs> {
        if from == to {
            return Some(current);
        }
        *self.slot(from) = Some(current);
        self.slot(to).take()
    }

    /// What to save for the harness that is not in front.
    pub fn saved(&self, h: Harness) -> Option<OpenTabs> {
        match h {
            Harness::Research => self.research.as_ref(),
            Harness::Study => self.study.as_ref(),
        }
        .map(Tabs::to_saved)
    }
}

/// A row in the + popover's "Open existing chat" list.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PickRow {
    pub id: SessionId,
    pub title: String,
    /// Already open in a tab (picking it focuses that tab).
    pub open: bool,
}

/// This harness's chats matching `query` (every word, any case, in the
/// title), newest first as listed, at most `max`. Sub-sessions are kept:
/// they are chats too.
pub fn pick_rows(
    sessions: &[SessionSummary],
    tabs: &Tabs,
    query: &str,
    max: usize,
) -> Vec<PickRow> {
    let words: Vec<String> = query.split_whitespace().map(str::to_lowercase).collect();
    sessions
        .iter()
        .filter(|s| {
            let t = s.title.to_lowercase();
            words.iter().all(|w| t.contains(w.as_str()))
        })
        .take(max)
        .map(|s| PickRow {
            id: s.id,
            title: s.title.clone(),
            open: tabs.list.iter().any(|t| t.id == Some(s.id)),
        })
        .collect()
}

/// Below this chat-column width the composer's chip row goes compact
/// (the mode shows its icon only, the model chip is capped) so it stays
/// on one row.
pub const COMPOSER_COMPACT_W: f32 = 760.;

pub fn composer_compact(centre_w: f32) -> bool {
    centre_w > 0. && centre_w < COMPOSER_COMPACT_W
}

#[cfg(test)]
mod tests {
    use super::*;

    fn summary(h: Harness, title: &str) -> SessionSummary {
        SessionSummary {
            id: SessionId::new(),
            title: title.into(),
            harness: h,
            created_at: String::new(),
            events: 1,
            parent: None,
            anchor: None,
            plan: None,
            project: None,
        }
    }

    #[test]
    fn chat_tabs_keep_their_draft_and_scroll_and_open_beside_each_other() {
        let (a, b) = (SessionId::new(), SessionId::new());
        let mut t = Tabs::default();
        t.set_active(a, "RAG");
        t.stash_place(LastChat {
            chat: Some(a.to_string()),
            draft: "half a question".into(),
            scroll: 420,
            ..Default::default()
        });
        assert!(t.open_new(b, "Crawler"));
        assert_eq!(t.active, 1);
        assert_eq!(t.list[0].place.draft, "half a question");
        assert_eq!(t.list[0].place.scroll, 420);
        // Picking an open chat focuses it instead of opening it twice.
        assert!(t.open_new(a, "RAG"));
        assert_eq!(t.active, 0);
        assert_eq!(t.list.len(), 2);
        assert!(!t.open_new(a, "RAG"));
        // Closing a tab never deletes the chat: it is only gone from tabs.
        t.close(0);
        assert_eq!(t.list.len(), 1);
        assert_eq!(t.active_tab().id, Some(b));
    }

    #[test]
    fn tabs_save_and_restore_dropping_explainers_and_lost_chats() {
        let rag = summary(Harness::Research, "RAG survey");
        let crawl = summary(Harness::Research, "Crawler");
        let mut t = Tabs::default();
        t.set_active(rag.id, "RAG survey");
        t.open_new(crawl.id, "Crawler");
        t.open_explain(3, "3D · RAG");
        t.add(); // a fresh chat
        t.focus(2); // the explainer in front
        let saved = t.to_saved();
        assert_eq!(saved.tabs.len(), 3);
        assert_eq!(
            saved.active, 1,
            "front explainer maps to the chat before it"
        );
        assert_eq!(saved.tabs[2].chat, None);
        // Crawler was deleted since: its tab is dropped on restore.
        let back = Tabs::from_saved(&saved, std::slice::from_ref(&rag));
        assert_eq!(back.list.len(), 2);
        assert_eq!(back.list[0].id, Some(rag.id));
        assert_eq!(back.list[1].id, None);
        assert_eq!(back.active, 0);
        // Nothing saved: one fresh tab.
        assert_eq!(Tabs::from_saved(&OpenTabs::default(), &[]), Tabs::default());
    }

    #[test]
    fn each_harness_keeps_its_own_tabs() {
        let mut parked = ParkedTabs::default();
        let mut research = Tabs::default();
        let a = SessionId::new();
        research.set_active(a, "RAG");
        research.add();
        // Research → Study: Study has none yet.
        assert_eq!(
            parked.swap(Harness::Research, Harness::Study, research.clone()),
            None
        );
        assert_eq!(
            parked.saved(Harness::Research).map(|s| s.tabs.len()),
            Some(2)
        );
        let study = Tabs::default();
        // Study → Research: Research's tabs come back as they were.
        let back = parked.swap(Harness::Study, Harness::Research, study);
        assert_eq!(back, Some(research));
        assert!(parked.study.is_some());
        assert!(parked.research.is_none());
    }

    #[test]
    fn the_picker_filters_by_every_word_and_marks_open_chats() {
        let all = vec![
            summary(Harness::Research, "Agentic RAG survey"),
            summary(Harness::Research, "Crawler design"),
            summary(Harness::Research, "RAG eval plan"),
        ];
        let mut t = Tabs::default();
        t.set_active(all[2].id, "RAG eval plan");
        let rows = pick_rows(&all, &t, "rag", 10);
        assert_eq!(rows.len(), 2);
        assert!(!rows[0].open && rows[1].open);
        assert_eq!(pick_rows(&all, &t, "RAG survey", 10).len(), 1);
        assert_eq!(pick_rows(&all, &t, "", 2).len(), 2);
        assert!(pick_rows(&all, &t, "zzz", 10).is_empty());
    }

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
