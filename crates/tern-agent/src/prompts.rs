//! System prompts: Tern's built-in ones, shown read-only, and the user's
//! own instructions, one for every chat and one per harness (Research,
//! Study), saved in the preferences and added to the system prompt after
//! Tern's own text and before the project's `TERN.md` rules.
//!
//! The user's prompts are set process-wide from the preferences
//! ([`set_user`], called by `Preferences::apply_runtime`), so every chat,
//! quick ask, and the effective-instructions preview read the same text.

use std::collections::BTreeMap;
use std::sync::{OnceLock, RwLock};

use tern_store::Harness;

/// Which user prompt: every chat, or one harness.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Scope {
    Global,
    Research,
    Study,
}

impl Scope {
    pub const ALL: [Self; 3] = [Self::Global, Self::Research, Self::Study];

    /// The key in `Preferences::system_prompts`.
    pub fn key(self) -> &'static str {
        match self {
            Self::Global => "global",
            Self::Research => "research",
            Self::Study => "study",
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Self::Global => "Every chat",
            Self::Research => "Research",
            Self::Study => "Study",
        }
    }

    pub fn of(h: Harness) -> Self {
        match h {
            Harness::Research => Self::Research,
            Harness::Study => Self::Study,
        }
    }
}

/// The most text kept from one user prompt, so a pasted book cannot eat
/// the context window.
pub const MAX_CHARS: usize = 8_000;

/// One built-in prompt, for the read-only list.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Builtin {
    pub title: &'static str,
    /// When Tern sends it.
    pub when: &'static str,
    pub text: String,
}

/// Tern's own prompts that apply to `scope` (Global lists the ones every
/// harness shares).
pub fn builtins(scope: Scope) -> Vec<Builtin> {
    let b = |title, when, text: &str| Builtin {
        title,
        when,
        text: text.to_owned(),
    };
    match scope {
        Scope::Global => vec![
            b("Tern", "Opens every system prompt.", crate::protocol::BASE),
            b(
                "Untrusted data",
                "Added when a tool returns outside text (web pages, MCP, files).",
                crate::untrusted::RULE,
            ),
            b(
                "Tool protocol",
                "Added when the chat has tools.",
                crate::protocol::TOOL_PROTOCOL,
            ),
            b(
                "Side question",
                "A side chat about the conversation.",
                crate::side::SYSTEM,
            ),
        ],
        Scope::Research => vec![
            b(
                "Web research",
                "Research chats with a search service.",
                crate::chat::WEB_INSTRUCTIONS,
            ),
            b(
                "Plan mode",
                "When the composer is in Plan mode.",
                crate::workspace::TurnMode::Plan
                    .instructions()
                    .unwrap_or_default(),
            ),
            b(
                "Act mode",
                "When the composer is in Act mode.",
                crate::workspace::TurnMode::Act
                    .instructions()
                    .unwrap_or_default(),
            ),
            b(
                "Quick ask",
                "Answers from numbered sources (quick ask, deep research writer).",
                crate::ask::SYSTEM,
            ),
        ],
        Scope::Study => {
            let mut v = vec![b(
                "Study tutor",
                "Study chats: the tutor teaches from your materials, citing [S1], [S2], and marks anything from outside them.",
                "You are Tern's study tutor. You teach the topic from the student's own materials, given as numbered passages. Ground everything in the passages and cite them inline as [S1], [S2]. Anything the passages do not say starts with \"Beyond your materials:\". If the passages do not cover what the student asks, say so plainly.",
            )];
            for m in tern_study::TutorMode::ALL {
                v.push(Builtin {
                    title: m.label(),
                    when: "The tutor style picked in Study.",
                    text: m.instructions().to_owned(),
                });
            }
            v
        }
    }
}

fn store() -> &'static RwLock<BTreeMap<String, String>> {
    static S: OnceLock<RwLock<BTreeMap<String, String>>> = OnceLock::new();
    S.get_or_init(Default::default)
}

/// Sets the user's prompts (from the preferences).
pub fn set_user(prompts: &BTreeMap<String, String>) {
    *store().write().unwrap_or_else(|e| e.into_inner()) = prompts.clone();
}

/// The user's prompt for `scope`, trimmed, if any.
pub fn user(scope: Scope) -> Option<String> {
    store()
        .read()
        .unwrap_or_else(|e| e.into_inner())
        .get(scope.key())
        .map(|s| s.trim().to_owned())
        .filter(|s| !s.is_empty())
}

/// The block the user's prompts add for a chat in `harness`: the global
/// one, then the harness's own, each under a heading so the model can tell
/// them from Tern's text. `None` when the user wrote none.
pub fn user_block_from(prompts: &BTreeMap<String, String>, harness: Harness) -> Option<String> {
    let get = |s: Scope| {
        prompts
            .get(s.key())
            .map(|t| t.trim())
            .filter(|t| !t.is_empty())
            .map(|t| t.chars().take(MAX_CHARS).collect::<String>())
    };
    let parts: Vec<String> = [Scope::Global, Scope::of(harness)]
        .into_iter()
        .filter_map(|s| {
            get(s).map(|t| match s {
                Scope::Global => format!("## The user's instructions\n\n{t}"),
                _ => format!("## The user's instructions for {}\n\n{t}", s.label()),
            })
        })
        .collect();
    (!parts.is_empty()).then(|| {
        format!(
            "The user wrote the instructions below. Follow them unless they conflict with Tern's safety rules or the untrusted-data rule.\n\n{}",
            parts.join("\n\n")
        )
    })
}

/// [`user_block_from`] with the prompts set by [`set_user`].
pub fn user_block(harness: Harness) -> Option<String> {
    let s = store().read().unwrap_or_else(|e| e.into_inner());
    user_block_from(&s, harness)
}

/// Puts the user's block after the harness text: Tern's own instructions
/// first, then the user's, so the user's come later and win on style.
pub fn compose(harness_text: Option<&str>, user: Option<&str>) -> Option<String> {
    match (harness_text, user) {
        (Some(h), Some(u)) => Some(format!("{h}\n\n{u}")),
        (Some(h), None) => Some(h.to_owned()),
        (None, Some(u)) => Some(u.to_owned()),
        (None, None) => None,
    }
}

/// Saves `text` as the user's prompt for `scope` in `prompts`; empty text
/// resets it to none (Tern's defaults only).
pub fn save(prompts: &mut BTreeMap<String, String>, scope: Scope, text: &str) {
    let t = text.trim();
    if t.is_empty() {
        prompts.remove(scope.key());
    } else {
        prompts.insert(scope.key().to_owned(), t.chars().take(MAX_CHARS).collect());
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn builtins_are_listed_per_harness() {
        for s in Scope::ALL {
            let b = builtins(s);
            assert!(!b.is_empty(), "{s:?}");
            assert!(b.iter().all(|x| !x.text.trim().is_empty()), "{s:?}");
        }
        assert!(builtins(Scope::Global)[0].text.starts_with("You are Tern"));
        assert!(builtins(Scope::Study).len() >= 4);
    }

    #[test]
    fn user_prompts_compose_global_then_harness() {
        let mut p = BTreeMap::new();
        assert_eq!(user_block_from(&p, Harness::Research), None);
        save(&mut p, Scope::Global, "  Use British spelling.  ");
        save(&mut p, Scope::Study, "Quiz me after every answer.");
        let r = user_block_from(&p, Harness::Research).unwrap();
        assert!(r.contains("Use British spelling."));
        assert!(!r.contains("Quiz me"), "study prompt stays out of research");
        let s = user_block_from(&p, Harness::Study).unwrap();
        let g = s.find("British").unwrap();
        let q = s.find("Quiz me").unwrap();
        assert!(g < q, "global first");
        assert!(s.contains("for Study"));
        // Reset to default.
        save(&mut p, Scope::Study, "   ");
        assert!(!p.contains_key("study"));
        // Composition keeps Tern's text first.
        assert_eq!(compose(Some("H"), Some("U")).as_deref(), Some("H\n\nU"));
        assert_eq!(compose(None, Some("U")).as_deref(), Some("U"));
        assert_eq!(compose(Some("H"), None).as_deref(), Some("H"));
        assert_eq!(compose(None, None), None);
        // Long prompts are capped.
        save(&mut p, Scope::Global, &"x".repeat(MAX_CHARS + 50));
        assert_eq!(p["global"].len(), MAX_CHARS);
    }
}
