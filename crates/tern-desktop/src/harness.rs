//! Research and Study kept apart, GPUI-free so it is unit tested.
//!
//! Each harness has its own chats: the sidebar lists only the active
//! harness's chats, and a chat of the other harness never opens inside
//! this one (opening it switches harness first). The switcher remembers
//! the last chat opened in each harness, with its unsent draft, where the
//! transcript was scrolled, and whether the right panel or the browser
//! was open, so Research → Study → Research comes back to the same place.
//! The memory is kept in the preferences (`last_chats`), so it survives a
//! restart.

use tern_agent::prefs::LastChat;
use tern_agent::{Harness, SessionSummary};
use tern_core::SessionId;

/// The preference key for a harness.
pub fn key(h: Harness) -> &'static str {
    match h {
        Harness::Research => "research",
        Harness::Study => "study",
    }
}

/// The other harness.
pub fn other(h: Harness) -> Harness {
    match h {
        Harness::Research => Harness::Study,
        Harness::Study => Harness::Research,
    }
}

/// The switcher's two rows: name and one line on what it is for.
pub fn label(h: Harness) -> (&'static str, &'static str) {
    match h {
        Harness::Research => ("Research", "Find, read and cite sources"),
        Harness::Study => ("Study", "Learn, quiz and review your material"),
    }
}

/// Only the chats of harness `h`, in the order given.
pub fn only(all: &[SessionSummary], h: Harness) -> Vec<SessionSummary> {
    all.iter().filter(|s| s.harness == h).cloned().collect()
}

/// Which harness a chat belongs to, when the list knows it.
pub fn harness_of(all: &[SessionSummary], id: SessionId) -> Option<Harness> {
    all.iter().find(|s| s.id == id).map(|s| s.harness)
}

/// What opening chat `id` from harness `active` takes: open it here, or
/// switch harness first and open it there.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Open {
    Here,
    SwitchTo(Harness),
}

pub fn open(all: &[SessionSummary], active: Harness, id: SessionId) -> Open {
    match harness_of(all, id) {
        Some(h) if h != active => Open::SwitchTo(h),
        _ => Open::Here,
    }
}

/// The last place in each harness.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Memory {
    pub research: LastChat,
    pub study: LastChat,
}

impl Memory {
    pub fn from_prefs(prefs: &tern_agent::Preferences) -> Self {
        let get = |h| prefs.last_chats.get(key(h)).cloned().unwrap_or_default();
        Self {
            research: get(Harness::Research),
            study: get(Harness::Study),
        }
    }

    pub fn get(&self, h: Harness) -> &LastChat {
        match h {
            Harness::Research => &self.research,
            Harness::Study => &self.study,
        }
    }

    /// Records where harness `h` was left.
    pub fn record(&mut self, h: Harness, last: LastChat) {
        match h {
            Harness::Research => self.research = last,
            Harness::Study => self.study = last,
        }
    }

    /// The chat to reopen in `h`, when it was a saved one.
    pub fn chat(&self, h: Harness) -> Option<SessionId> {
        self.get(h).chat.as_deref().and_then(|s| s.parse().ok())
    }

    /// A deleted chat is forgotten wherever it was remembered.
    pub fn forget(&mut self, id: SessionId) {
        let id = id.to_string();
        for slot in [&mut self.research, &mut self.study] {
            if slot.chat.as_deref() == Some(id.as_str()) {
                *slot = LastChat::default();
            }
        }
    }

    /// Writes both slots into the preferences.
    pub fn save_into(&self, prefs: &mut tern_agent::Preferences) {
        for h in [Harness::Research, Harness::Study] {
            let slot = self.get(h);
            if *slot == LastChat::default() {
                prefs.last_chats.remove(key(h));
            } else {
                prefs.last_chats.insert(key(h).to_owned(), slot.clone());
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn chat(h: Harness, title: &str) -> SessionSummary {
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
    fn the_sidebar_lists_only_the_active_harness() {
        let all = vec![
            chat(Harness::Research, "RAG survey"),
            chat(Harness::Study, "Viva prep"),
            chat(Harness::Research, "Crawler design"),
        ];
        let r = only(&all, Harness::Research);
        assert_eq!(
            r.iter().map(|s| s.title.as_str()).collect::<Vec<_>>(),
            ["RAG survey", "Crawler design"]
        );
        let s = only(&all, Harness::Study);
        assert_eq!(s.len(), 1);
        assert_eq!(s[0].title, "Viva prep");
    }

    #[test]
    fn a_chat_of_the_other_harness_switches_first() {
        let all = vec![chat(Harness::Research, "a"), chat(Harness::Study, "b")];
        assert_eq!(open(&all, Harness::Research, all[0].id), Open::Here);
        assert_eq!(
            open(&all, Harness::Research, all[1].id),
            Open::SwitchTo(Harness::Study)
        );
        assert_eq!(
            open(&all, Harness::Study, all[0].id),
            Open::SwitchTo(Harness::Research)
        );
        // Unknown (a fresh draft): open where you are.
        assert_eq!(open(&all, Harness::Study, SessionId::new()), Open::Here);
        assert_eq!(other(Harness::Study), Harness::Research);
    }

    #[test]
    fn research_study_research_comes_back_to_the_same_place() {
        let r = SessionId::new();
        let s = SessionId::new();
        let mut m = Memory::default();
        m.record(
            Harness::Research,
            LastChat {
                chat: Some(r.to_string()),
                draft: "half a question".into(),
                scroll: 840,
                side_open: true,
                browser_open: false,
            },
        );
        m.record(
            Harness::Study,
            LastChat {
                chat: Some(s.to_string()),
                draft: String::new(),
                scroll: 0,
                side_open: false,
                browser_open: true,
            },
        );
        assert_eq!(m.chat(Harness::Research), Some(r));
        assert_eq!(m.chat(Harness::Study), Some(s));
        assert_eq!(m.get(Harness::Research).draft, "half a question");
        assert_eq!(m.get(Harness::Research).scroll, 840);
        assert!(m.get(Harness::Research).side_open);
        assert!(m.get(Harness::Study).browser_open);

        // Through the preferences file and back: it survives a restart.
        let mut prefs = tern_agent::Preferences::default();
        m.save_into(&mut prefs);
        let json = serde_json::to_string(&prefs).unwrap();
        let back: tern_agent::Preferences = serde_json::from_str(&json).unwrap();
        assert_eq!(Memory::from_prefs(&back), m);

        // A deleted chat is forgotten; the other harness keeps its place.
        m.forget(r);
        assert_eq!(m.chat(Harness::Research), None);
        assert_eq!(m.get(Harness::Research).draft, "");
        assert_eq!(m.chat(Harness::Study), Some(s));
        m.save_into(&mut prefs);
        assert!(!prefs.last_chats.contains_key("research"));
    }

    #[test]
    fn the_switcher_names_both_harnesses() {
        assert_eq!(label(Harness::Research).0, "Research");
        assert!(label(Harness::Study).1.starts_with("Learn"));
    }
}
