//! What the window showed when Tern last closed, put back on launch:
//! the harness (Research or Study), the Study tab, the right panel's view
//! and whether it was open, and whether the sidebar was open. GPUI-free so
//! it is unit tested.
//!
//! The chat tabs and the chat per harness are saved as before
//! (`open_tabs`, `last_chats`); [`start`] reads them to pick the chat that
//! opens first, so the tab that was in front comes back in front. A chat
//! deleted since falls back to the last chat with messages, then to a new
//! one. Everything is kept in the preferences (`session`), saved shortly
//! after a change and again when the window closes.

use std::time::Duration;

use tern_agent::Harness;
use tern_agent::prefs::{OpenTabs, SessionState};
use tern_core::SessionId;

use crate::model::RunView;
use crate::study::StudyTab;

/// How long the window must stay the same before it is saved.
pub const SAVE_AFTER: Duration = Duration::from_millis(800);

/// What the window shows, as far as a restart puts it back.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Shown {
    pub harness: Harness,
    pub study_tab: StudyTab,
    pub right_view: RunView,
    pub right_open: bool,
    pub sidebar_open: bool,
}

impl Default for Shown {
    fn default() -> Self {
        Self {
            harness: Harness::Research,
            study_tab: StudyTab::Chat,
            right_view: RunView::Summary,
            right_open: false,
            sidebar_open: true,
        }
    }
}

pub fn harness_key(h: Harness) -> &'static str {
    crate::harness::key(h)
}

pub fn parse_harness(s: &str) -> Option<Harness> {
    match s {
        "research" => Some(Harness::Research),
        "study" => Some(Harness::Study),
        _ => None,
    }
}

pub fn study_tab_key(t: StudyTab) -> &'static str {
    match t {
        StudyTab::Chat => "chat",
        StudyTab::Cards => "cards",
        StudyTab::Quiz => "quiz",
        StudyTab::Viva => "viva",
        StudyTab::Explain => "explain",
    }
}

pub fn parse_study_tab(s: &str) -> Option<StudyTab> {
    [
        StudyTab::Chat,
        StudyTab::Cards,
        StudyTab::Quiz,
        StudyTab::Viva,
        StudyTab::Explain,
    ]
    .into_iter()
    .find(|t| study_tab_key(*t) == s)
}

const RUN_VIEWS: [RunView; 8] = [
    RunView::Summary,
    RunView::Detailed,
    RunView::Raw,
    RunView::Report,
    RunView::Workers,
    RunView::Ledger,
    RunView::Decisions,
    RunView::Graph,
];

pub fn run_view_key(v: RunView) -> &'static str {
    match v {
        RunView::Summary => "summary",
        RunView::Detailed => "detailed",
        RunView::Raw => "raw",
        RunView::Report => "report",
        RunView::Workers => "workers",
        RunView::Ledger => "ledger",
        RunView::Decisions => "decisions",
        RunView::Graph => "graph",
    }
}

pub fn parse_run_view(s: &str) -> Option<RunView> {
    RUN_VIEWS.into_iter().find(|v| run_view_key(*v) == s)
}

impl Shown {
    /// The saved state; anything missing or unknown keeps its default.
    pub fn from_prefs(s: &SessionState) -> Self {
        let d = Self::default();
        Self {
            harness: s
                .harness
                .as_deref()
                .and_then(parse_harness)
                .unwrap_or(d.harness),
            study_tab: s
                .study_tab
                .as_deref()
                .and_then(parse_study_tab)
                .unwrap_or(d.study_tab),
            right_view: s
                .right_view
                .as_deref()
                .and_then(parse_run_view)
                .unwrap_or(d.right_view),
            right_open: s.right_open.unwrap_or(d.right_open),
            sidebar_open: s.sidebar_open.unwrap_or(d.sidebar_open),
        }
    }

    pub fn to_prefs(self) -> SessionState {
        SessionState {
            harness: Some(harness_key(self.harness).to_owned()),
            study_tab: Some(study_tab_key(self.study_tab).to_owned()),
            right_view: Some(run_view_key(self.right_view).to_owned()),
            right_open: Some(self.right_open),
            sidebar_open: Some(self.sidebar_open),
        }
    }
}

/// The chat a harness opens with on launch.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Start {
    /// The chat of the tab that was in front.
    Chat(SessionId),
    /// The tab in front was a new chat: open a new one.
    Fresh,
    /// Nothing saved: the last chat with messages, else a new one.
    Last,
}

/// What harness `h`'s saved tabs say to open first.
pub fn start(saved: Option<&OpenTabs>) -> Start {
    let Some(tabs) = saved.filter(|t| !t.tabs.is_empty()) else {
        return Start::Last;
    };
    let front = &tabs.tabs[tabs.active.min(tabs.tabs.len() - 1)];
    match front.chat.as_deref() {
        None => Start::Fresh,
        Some(id) => id.parse().map_or(Start::Last, Start::Chat),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tern_agent::prefs::OpenTab;

    #[test]
    fn the_window_round_trips_through_the_preferences() {
        let s = Shown {
            harness: Harness::Study,
            study_tab: StudyTab::Explain,
            right_view: RunView::Graph,
            right_open: true,
            sidebar_open: false,
        };
        assert_eq!(Shown::from_prefs(&s.to_prefs()), s);
        for t in [
            StudyTab::Chat,
            StudyTab::Cards,
            StudyTab::Quiz,
            StudyTab::Viva,
            StudyTab::Explain,
        ] {
            assert_eq!(parse_study_tab(study_tab_key(t)), Some(t));
        }
        for v in RUN_VIEWS {
            assert_eq!(parse_run_view(run_view_key(v)), Some(v));
        }
    }

    #[test]
    fn nothing_saved_or_nonsense_keeps_the_defaults() {
        assert_eq!(
            Shown::from_prefs(&SessionState::default()),
            Shown::default()
        );
        let odd = SessionState {
            harness: Some("garden".into()),
            study_tab: Some("??".into()),
            right_view: Some("".into()),
            right_open: None,
            sidebar_open: Some(false),
        };
        let s = Shown::from_prefs(&odd);
        assert_eq!(s.harness, Harness::Research);
        assert_eq!(s.study_tab, StudyTab::Chat);
        assert_eq!(s.right_view, RunView::Summary);
        assert!(!s.right_open && !s.sidebar_open);
    }

    #[test]
    fn the_tab_in_front_decides_the_first_chat() {
        let id = SessionId::new();
        let tabs = |active, chats: Vec<Option<String>>| OpenTabs {
            tabs: chats
                .into_iter()
                .map(|chat| OpenTab {
                    chat,
                    title: "t".into(),
                })
                .collect(),
            active,
        };
        assert_eq!(start(None), Start::Last);
        assert_eq!(start(Some(&tabs(0, vec![]))), Start::Last);
        let saved = tabs(1, vec![None, Some(id.to_string())]);
        assert_eq!(start(Some(&saved)), Start::Chat(id));
        let saved = tabs(0, vec![None, Some(id.to_string())]);
        assert_eq!(start(Some(&saved)), Start::Fresh);
        // An index past the end means the last tab; a bad id, the last chat.
        let saved = tabs(9, vec![None, Some(id.to_string())]);
        assert_eq!(start(Some(&saved)), Start::Chat(id));
        assert_eq!(start(Some(&tabs(0, vec![Some("x".into())]))), Start::Last);
    }
}
