//! Continuity state for the window, kept apart from GPUI so it can be
//! tested without a display: message IDs on the transcript, multi-select
//! and range actions, the waiting move, the chat's trash, the highlight
//! popover, the side chat, and the Library page.

use std::collections::BTreeSet;

use tern_agent::library::{ItemState, LibraryItem, LibraryKind, LibraryQuery, LibrarySort};
use tern_core::SessionId;

/// Message IDs and range actions for the open chat.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ChatIds {
    /// The turn number of each user message in the transcript, in order.
    pub numbers: Vec<u32>,
    /// Their IDs as shown (`Q3`, or `Q12.4` in a sub-session), when the
    /// chat's database has said.
    pub labels: Vec<String>,
    /// The number the next turn gets.
    pub next: u32,
    /// For a sub-session: the parent message it hangs under.
    pub anchor: Option<u32>,
    /// Messages picked for a range action, by turn number.
    pub selected: BTreeSet<u32>,
    /// A move waiting to be confirmed: what moved and where.
    pub pending: Option<(Vec<u32>, SessionId)>,
    /// Removed messages (the chat's trash): number and first line.
    pub removed: Vec<(u32, String)>,
    /// The answer whose Branch menu is open, by transcript index.
    pub branch_menu: Option<usize>,
}

impl ChatIds {
    pub fn reset(&mut self) {
        *self = Self {
            next: 1,
            ..Self::default()
        };
    }

    /// The transcript was rebuilt from these live turn numbers.
    pub fn restored(&mut self, numbers: Vec<u32>) {
        self.next = numbers
            .iter()
            .max()
            .map(|n| n + 1)
            .unwrap_or(1)
            .max(self.next);
        self.numbers = numbers;
        self.labels.clear();
        self.selected.clear();
        self.branch_menu = None;
    }

    /// A new user message went out.
    pub fn started(&mut self) {
        let n = self.next.max(1);
        self.numbers.push(n);
        if !self.labels.is_empty() {
            self.labels.push(self.label_for(n));
        }
        self.next = n + 1;
    }

    fn label_for(&self, n: u32) -> String {
        match self.anchor {
            Some(a) => {
                // A sub-session's IDs count across its siblings; the next
                // one follows the last shown.
                let last = self
                    .labels
                    .last()
                    .and_then(|l| l.rsplit_once('.'))
                    .and_then(|(_, s)| s.parse::<u32>().ok())
                    .unwrap_or(0);
                format!("Q{a}.{}", last + 1)
            }
            None => format!("Q{n}"),
        }
    }

    /// The ID of the `k`-th user message.
    pub fn label(&self, k: usize) -> Option<String> {
        if let Some(l) = self.labels.get(k) {
            return Some(l.clone());
        }
        let n = *self.numbers.get(k)?;
        Some(match self.anchor {
            Some(a) => format!("Q{a}.{n}"),
            None => format!("Q{n}"),
        })
    }

    pub fn toggle(&mut self, n: u32) {
        if !self.selected.remove(&n) {
            self.selected.insert(n);
        }
    }

    /// "Q3–Q5" or "Q3, Q7" for the selection.
    pub fn selection_label(&self) -> String {
        let v: Vec<u32> = self.selected.iter().copied().collect();
        tern_agent::range::label(&v)
    }
}

/// The sidebar's order: each chat followed by its sub-sessions (by the
/// message they hang under, then age). A sub-session whose chat is not in
/// the list shows where it falls.
pub fn tree(sessions: &[tern_agent::SessionSummary]) -> Vec<usize> {
    let present = |id: &SessionId| sessions.iter().any(|s| s.id == *id);
    let mut out = Vec::new();
    for (i, s) in sessions.iter().enumerate() {
        if s.parent.as_ref().is_some_and(present) {
            continue;
        }
        out.push(i);
        let mut subs: Vec<usize> = sessions
            .iter()
            .enumerate()
            .filter(|(_, c)| c.parent == Some(s.id))
            .map(|(j, _)| j)
            .collect();
        subs.sort_by_key(|j| (sessions[*j].anchor, sessions[*j].created_at.clone()));
        out.extend(subs);
    }
    out
}

/// One side chat exchange as shown.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SideLine {
    pub question: String,
    /// `None` while it is being answered.
    pub answer: Option<String>,
    pub failed: bool,
}

/// The throwaway side chat.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct SideState {
    pub open: bool,
    /// Shown in its own window instead of the right pane.
    pub popped: bool,
    /// The chat it reads.
    pub session: Option<SessionId>,
    pub lines: Vec<SideLine>,
    /// Highlighted text the next question is about.
    pub focus: Option<String>,
    /// The side chat is reading the thread and connecting.
    pub opening: bool,
}

impl SideState {
    pub fn busy(&self) -> bool {
        self.opening || self.lines.last().is_some_and(|l| l.answer.is_none())
    }

    /// Opens over `session`; a different chat starts over.
    pub fn open_for(&mut self, session: SessionId) -> bool {
        self.open = true;
        if self.session == Some(session) {
            return false;
        }
        self.session = Some(session);
        self.lines.clear();
        self.focus = None;
        self.opening = true;
        true
    }

    pub fn asked(&mut self, question: &str) -> bool {
        if self.busy() || question.trim().is_empty() {
            return false;
        }
        self.lines.push(SideLine {
            question: question.trim().to_owned(),
            answer: None,
            failed: false,
        });
        true
    }

    pub fn answered(&mut self, result: Result<String, String>) {
        let Some(last) = self.lines.last_mut().filter(|l| l.answer.is_none()) else {
            return;
        };
        match result {
            Ok(a) => last.answer = Some(a),
            Err(e) => {
                last.answer = Some(e);
                last.failed = true;
            }
        }
        self.focus = None;
    }

    pub fn close(&mut self) {
        self.open = false;
        self.popped = false;
    }
}

/// Text picked in an answer, and where the popover sits.
#[derive(Clone, Debug, PartialEq)]
pub struct Highlight {
    pub entry: usize,
    pub text: String,
    pub at: (f32, f32),
    /// What the last action said ("Added to Highlights").
    pub note: Option<String>,
}

/// Library tabs: All, one per kind, and the trash.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LibraryTab {
    All,
    Kind(LibraryKind),
    Trash,
}

impl LibraryTab {
    pub const SHOWN: [Self; 9] = [
        Self::All,
        Self::Kind(LibraryKind::Report),
        Self::Kind(LibraryKind::Source),
        Self::Kind(LibraryKind::Note),
        Self::Kind(LibraryKind::Deck),
        Self::Kind(LibraryKind::File),
        Self::Kind(LibraryKind::Scene),
        Self::Kind(LibraryKind::Session),
        Self::Trash,
    ];

    pub fn label(self) -> &'static str {
        match self {
            Self::All => "All",
            Self::Kind(k) => k.plural(),
            Self::Trash => "Trash",
        }
    }
}

/// Rows a Library page shows.
pub const LIBRARY_PAGE: usize = 60;

/// The Library page.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LibraryState {
    pub tab: LibraryTab,
    pub text: String,
    pub sort: LibrarySort,
    pub grid: bool,
    pub page: usize,
    pub items: Vec<LibraryItem>,
    pub total: usize,
    pub counts: Vec<(LibraryKind, usize)>,
    pub trash: usize,
    pub selected: BTreeSet<String>,
    pub loading: bool,
    pub new_menu: bool,
    pub note: Option<String>,
    /// Bumped per request so a slow answer cannot overwrite a newer one.
    pub request: u64,
}

impl Default for LibraryState {
    fn default() -> Self {
        Self {
            tab: LibraryTab::All,
            text: String::new(),
            sort: LibrarySort::Newest,
            grid: false,
            page: 0,
            items: Vec::new(),
            total: 0,
            counts: Vec::new(),
            trash: 0,
            selected: BTreeSet::new(),
            loading: false,
            new_menu: false,
            note: None,
            request: 0,
        }
    }
}

impl LibraryState {
    /// The query for the page shown.
    pub fn query(&self) -> LibraryQuery {
        LibraryQuery {
            kinds: match self.tab {
                LibraryTab::Kind(k) => vec![k],
                _ => Vec::new(),
            },
            text: self.text.clone(),
            state: if self.tab == LibraryTab::Trash {
                ItemState::Trash
            } else {
                ItemState::Live
            },
            sort: self.sort,
            offset: self.page * LIBRARY_PAGE,
            limit: LIBRARY_PAGE,
        }
    }

    /// Starts a request; returns its number.
    pub fn begin(&mut self) -> u64 {
        self.request += 1;
        self.loading = true;
        self.request
    }

    pub fn loaded(
        &mut self,
        request: u64,
        items: Vec<LibraryItem>,
        total: usize,
        counts: Option<(Vec<(LibraryKind, usize)>, usize)>,
    ) {
        if request != self.request {
            return;
        }
        self.loading = false;
        self.items = items;
        self.total = total;
        if let Some((c, t)) = counts {
            self.counts = c;
            self.trash = t;
        }
        let shown: BTreeSet<String> = self.items.iter().map(|i| i.id.clone()).collect();
        self.selected.retain(|id| shown.contains(id));
    }

    pub fn set_tab(&mut self, tab: LibraryTab) {
        self.tab = tab;
        self.page = 0;
        self.selected.clear();
    }

    pub fn pages(&self) -> usize {
        self.total.div_ceil(LIBRARY_PAGE).max(1)
    }

    pub fn count(&self, tab: LibraryTab) -> usize {
        match tab {
            LibraryTab::All => self.counts.iter().map(|c| c.1).sum(),
            LibraryTab::Kind(k) => self
                .counts
                .iter()
                .find(|c| c.0 == k)
                .map(|c| c.1)
                .unwrap_or(0),
            LibraryTab::Trash => self.trash,
        }
    }

    pub fn next_sort(&mut self) {
        self.sort = match self.sort {
            LibrarySort::Newest => LibrarySort::Oldest,
            LibrarySort::Oldest => LibrarySort::Title,
            LibrarySort::Title => LibrarySort::Kind,
            LibrarySort::Kind => LibrarySort::Largest,
            LibrarySort::Largest => LibrarySort::Newest,
        };
        self.page = 0;
    }

    pub fn toggle(&mut self, id: &str) {
        if !self.selected.remove(id) {
            self.selected.insert(id.to_owned());
        }
    }

    /// Selects every row on the page, or clears when all are selected.
    pub fn toggle_all(&mut self) {
        if !self.items.is_empty() && self.items.iter().all(|i| self.selected.contains(&i.id)) {
            self.selected.clear();
        } else {
            self.selected = self.items.iter().map(|i| i.id.clone()).collect();
        }
    }
}

/// Everything continuity adds to the window.
#[derive(Clone, Debug, Default)]
pub struct Continuity {
    pub ids: ChatIds,
    pub side: SideState,
    pub highlight: Option<Highlight>,
    pub library: LibraryState,
    /// Background branches still running, by sub-session.
    pub background: Vec<SessionId>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ids_follow_the_transcript_and_sub_sessions() {
        let mut ids = ChatIds::default();
        ids.reset();
        ids.restored(vec![1, 3, 4]);
        assert_eq!(ids.next, 5);
        ids.started();
        assert_eq!(ids.label(3).as_deref(), Some("Q5"));
        ids.toggle(3);
        ids.toggle(4);
        ids.toggle(5);
        assert_eq!(ids.selection_label(), "Q3–Q5");
        ids.toggle(4);
        assert_eq!(ids.selection_label(), "Q3, Q5");

        let mut sub = ChatIds {
            anchor: Some(12),
            ..ChatIds::default()
        };
        sub.restored(vec![1, 2]);
        sub.labels = vec!["Q12.1".into(), "Q12.3".into()];
        sub.started();
        assert_eq!(sub.label(2).as_deref(), Some("Q12.4"));
    }

    #[test]
    fn sub_sessions_sit_under_their_chat() {
        let row =
            |parent: Option<SessionId>, anchor: Option<u32>, at: &str| tern_agent::SessionSummary {
                id: SessionId::new(),
                title: at.into(),
                harness: tern_agent::Harness::Research,
                created_at: at.into(),
                events: 1,
                parent,
                anchor,
                plan: None,
            };
        let a = row(None, None, "3");
        let b = row(None, None, "2");
        let a2 = row(Some(a.id), Some(5), "4");
        let a1 = row(Some(a.id), Some(2), "5");
        let orphan = row(Some(SessionId::new()), Some(1), "1");
        let list = vec![a1, a.clone(), b, orphan, a2];
        let order: Vec<String> = tree(&list).iter().map(|i| list[*i].title.clone()).collect();
        assert_eq!(order, ["3", "5", "4", "2", "1"]);
    }

    #[test]
    fn the_side_chat_takes_one_question_at_a_time() {
        let mut s = SideState::default();
        let id = SessionId::new();
        assert!(s.open_for(id));
        assert!(s.busy(), "busy while it opens");
        s.opening = false;
        assert!(s.asked("Why?"));
        assert!(!s.asked("And?"));
        s.answered(Ok("Because.".into()));
        assert_eq!(s.lines[0].answer.as_deref(), Some("Because."));
        assert!(!s.open_for(id), "same chat keeps its lines");
        assert!(s.open_for(SessionId::new()));
        assert!(s.lines.is_empty());
    }

    #[test]
    fn library_pages_and_selection() {
        let mut l = LibraryState::default();
        l.set_tab(LibraryTab::Kind(LibraryKind::Note));
        l.text = "rag".into();
        l.page = 2;
        let q = l.query();
        assert_eq!(q.kinds, [LibraryKind::Note]);
        assert_eq!(q.offset, 2 * LIBRARY_PAGE);
        let r = l.begin();
        let items: Vec<LibraryItem> = (0..3)
            .map(|i| LibraryItem::new(LibraryKind::Note, format!("note:{i}"), "n"))
            .collect();
        l.loaded(r - 1, Vec::new(), 0, None);
        assert!(l.loading, "a stale answer is ignored");
        l.loaded(r, items, 130, Some((vec![(LibraryKind::Note, 130)], 2)));
        assert_eq!(l.pages(), 3);
        assert_eq!(l.count(LibraryTab::All), 130);
        assert_eq!(l.count(LibraryTab::Trash), 2);
        l.toggle_all();
        assert_eq!(l.selected.len(), 3);
        l.toggle_all();
        assert!(l.selected.is_empty());
        l.set_tab(LibraryTab::Trash);
        assert_eq!(l.query().state, ItemState::Trash);
        l.next_sort();
        assert_eq!(l.sort, LibrarySort::Oldest);
    }
}
