//! The tutor: teaching one topic from the user's materials.
//!
//! [`tutor_system`] builds the instructions for a mode (Explain, Socratic,
//! or Explain like I'm new), pitched at the topic's mastery, with the
//! topic's passages numbered `[S1]`, `[S2]` to cite. The tutor must flag
//! anything it says beyond the materials. [`fallback_reply`] answers
//! without a model by quoting the passage that best matches and asking a
//! check question.

use serde::{Deserialize, Serialize};

use crate::mastery::{Level, Mastery};
use crate::material::{Source, sources_block};
use crate::text::{cited_sources, clip, overlap, sentences};
use crate::topics::Topic;

/// The marker the tutor puts before anything not in the materials.
pub const BEYOND: &str = "Beyond your materials:";

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TutorMode {
    /// A clear explanation, then one check question.
    #[default]
    Explain,
    /// Teaching by questions: the student does the thinking.
    Socratic,
    /// Plain words, an everyday analogy, no assumed background.
    New,
}

impl TutorMode {
    pub const ALL: [Self; 3] = [Self::Explain, Self::Socratic, Self::New];

    pub fn label(self) -> &'static str {
        match self {
            Self::Explain => "Explain",
            Self::Socratic => "Socratic",
            Self::New => "Explain like I'm new",
        }
    }

    pub fn parse(s: &str) -> Option<Self> {
        Some(match s.trim().to_lowercase().as_str() {
            "explain" | "e" => Self::Explain,
            "socratic" | "s" | "socrates" => Self::Socratic,
            "new" | "eli5" | "beginner" | "like-new" | "like_new" | "simple" => Self::New,
            _ => return None,
        })
    }

    pub fn instructions(self) -> &'static str {
        match self {
            Self::Explain => {
                "Explain the topic clearly in short paragraphs, building from the core idea to the details, with one concrete example from the passages. End with exactly one check-for-understanding question."
            }
            Self::Socratic => {
                "Teach by asking. Do not lecture: ask one focused question at a time that leads the student to the idea, give a hint when they are stuck, and confirm or correct their answer before the next question. Keep each turn under 120 words."
            }
            Self::New => {
                "Explain as if to someone meeting the subject for the first time: plain words, no jargon without a one-line definition, one everyday analogy (flag the analogy as yours, not the material's). End with one simple check question."
            }
        }
    }
}

/// One message of a tutor chat.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct TutorTurn {
    /// `user` or `tutor`.
    pub role: String,
    pub text: String,
}

impl TutorTurn {
    pub fn user(text: impl Into<String>) -> Self {
        Self {
            role: "user".into(),
            text: text.into(),
        }
    }

    pub fn tutor(text: impl Into<String>) -> Self {
        Self {
            role: "tutor".into(),
            text: text.into(),
        }
    }

    pub fn is_user(&self) -> bool {
        self.role == "user"
    }
}

/// How deep to go for a mastery level.
fn depth(m: &Mastery) -> &'static str {
    match m.level() {
        Level::New | Level::Learning => {
            "The student is new to this topic: start from the basics, define terms, and go one step at a time."
        }
        Level::Shaky => {
            "The student knows the basics but is shaky: reinforce the core idea, then fill the gaps they are likely to have."
        }
        Level::Solid => {
            "The student is solid on this topic: skip the basics, focus on subtleties, edge cases, and connections between ideas."
        }
        Level::Mastered => {
            "The student has mastered this topic: challenge them with application, comparison, and critique."
        }
    }
}

/// The tutor's instructions for a topic.
pub fn tutor_system(
    mode: TutorMode,
    topic: &Topic,
    mastery: &Mastery,
    sources: &[Source],
) -> String {
    let mut s = format!(
        "You are Tern's study tutor. You teach the topic \"{}\" from the student's own materials, given below as numbered passages.\n\nRules:\n- Ground everything in the passages and cite them inline as [S1], [S2]. Cite the passage each claim comes from.\n- If you add anything the passages do not say (background, an analogy, a correction), start that sentence with \"{BEYOND}\" so the student knows it is not from their materials. Never present outside knowledge as if it were in the materials.\n- If the passages do not cover what the student asks, say so plainly.\n- {}\n- {}\n",
        topic.title,
        mode.instructions(),
        depth(mastery),
    );
    if !topic.objectives.is_empty() {
        s.push_str("\nLearning objectives for this topic:\n");
        for o in &topic.objectives {
            s.push_str(&format!("- {o}\n"));
        }
    }
    s.push_str("\nPassages:\n\n");
    s.push_str(&sources_block(sources));
    s
}

/// The first message that starts a session, written for the user.
pub fn opening(mode: TutorMode, topic: &Topic) -> String {
    match mode {
        TutorMode::Explain => format!("Teach me {}.", topic.title),
        TutorMode::Socratic => format!("Help me work out {} by asking me questions.", topic.title),
        TutorMode::New => format!("Explain {} like I'm new to it.", topic.title),
    }
}

/// The passages a reply cites, as their labels.
pub fn citations(reply: &str, sources: &[Source]) -> Vec<String> {
    cited_sources(reply)
        .into_iter()
        .filter_map(|n| sources.iter().find(|s| s.n == n))
        .map(|s| format!("[S{}] {}", s.n, s.label()))
        .collect()
}

/// Whether a reply flags content beyond the materials.
pub fn flags_beyond(reply: &str) -> bool {
    reply.contains(BEYOND)
}

/// A reply without a model: the passage that best matches the last
/// message (or the next passage in order), quoted with its anchor, and a
/// check question.
pub fn fallback_reply(
    mode: TutorMode,
    topic: &Topic,
    sources: &[Source],
    history: &[TutorTurn],
) -> String {
    if sources.is_empty() {
        return format!(
            "Your materials have nothing on {} yet. Add a file or note to study from.",
            topic.title
        );
    }
    let asked = history
        .iter()
        .rev()
        .find(|t| t.is_user())
        .map(|t| t.text.as_str())
        .unwrap_or("");
    let tutor_turns = history.iter().filter(|t| !t.is_user()).count();
    let best = sources
        .iter()
        .map(|s| (overlap(asked, &s.text), s))
        .filter(|(o, _)| *o >= 0.34)
        .max_by(|a, b| a.0.total_cmp(&b.0))
        .map(|(_, s)| s)
        .unwrap_or(&sources[tutor_turns % sources.len()]);
    let sents: Vec<String> = sentences(&best.text)
        .into_iter()
        .filter(|s| crate::material::heading(s).is_none())
        .collect();
    let excerpt = clip(
        &sents.iter().take(3).cloned().collect::<Vec<_>>().join(" "),
        600,
    );
    let question = match mode {
        TutorMode::Socratic => format!(
            "Before reading on: in your own words, what do you think this passage is saying about {}?",
            topic.title
        ),
        TutorMode::New => {
            "Can you say that back in one plain sentence, as if to a friend?".to_owned()
        }
        TutorMode::Explain => match topic
            .objectives
            .get(tutor_turns % topic.objectives.len().max(1))
        {
            Some(o) => format!("Check: can you {}?", lower_first(o)),
            None => "Check: what is the key idea here?".to_owned(),
        },
    };
    format!(
        "(Offline tutor: no model is connected, so here is the passage from your materials.)\n\nFrom {} [S{}]:\n\n> {excerpt}\n\n{question}",
        best.label(),
        best.n
    )
}

fn lower_first(s: &str) -> String {
    let mut c = s.chars();
    match c.next() {
        Some(f) => f.to_lowercase().collect::<String>() + c.as_str(),
        None => String::new(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::material::Anchor;

    fn topic() -> Topic {
        Topic {
            id: "osmosis".into(),
            title: "Osmosis".into(),
            objectives: vec!["Explain osmosis in your own words".into()],
            chunks: vec!["m1#1".into()],
            anchors: vec![],
        }
    }

    fn sources() -> Vec<Source> {
        vec![
            Source {
                n: 1,
                chunk: "m1#1".into(),
                anchor: Anchor {
                    material: "m1".into(),
                    file: "bio.pdf".into(),
                    page: Some(4),
                    section: None,
                },
                text: "Osmosis is the movement of water across a membrane. Water moves toward higher solute concentration.".into(),
            },
            Source {
                n: 2,
                chunk: "m1#2".into(),
                anchor: Anchor {
                    material: "m1".into(),
                    file: "bio.pdf".into(),
                    page: Some(5),
                    section: None,
                },
                text: "Turgor pressure keeps plant cells firm when water enters the vacuole.".into(),
            },
        ]
    }

    #[test]
    fn system_prompt_carries_rules_mode_depth_and_passages() {
        let s = tutor_system(
            TutorMode::Socratic,
            &topic(),
            &Mastery::default(),
            &sources(),
        );
        assert!(s.contains(BEYOND));
        assert!(s.contains("Teach by asking"));
        assert!(s.contains("new to this topic"));
        assert!(s.contains("[S2] bio.pdf p. 5"));
        assert!(s.contains("- Explain osmosis"));
        let solid = Mastery {
            rating: 0.7,
            attempts: 3,
            credit: 3.0,
            ..Mastery::default()
        };
        assert!(
            tutor_system(TutorMode::Explain, &topic(), &solid, &sources())
                .contains("skip the basics")
        );
    }

    #[test]
    fn citations_and_beyond_flag() {
        let r = "Water moves toward solute [S1]. Beyond your materials: this is like a sponge.";
        assert_eq!(citations(r, &sources()), vec!["[S1] bio.pdf p. 4"]);
        assert!(flags_beyond(r));
        assert!(!flags_beyond("plain"));
    }

    #[test]
    fn fallback_quotes_the_matching_passage() {
        let h = vec![TutorTurn::user(
            "what keeps plant cells firm? turgor pressure?",
        )];
        let r = fallback_reply(TutorMode::Explain, &topic(), &sources(), &h);
        assert!(r.contains("bio.pdf p. 5 [S2]"), "{r}");
        assert!(r.contains("Check: can you explain osmosis"));
        let r = fallback_reply(
            TutorMode::Socratic,
            &topic(),
            &sources(),
            &[TutorTurn::user("teach me")],
        );
        assert!(r.contains("[S1]"));
        assert!(fallback_reply(TutorMode::New, &topic(), &[], &[]).contains("nothing on Osmosis"));
        assert_eq!(TutorMode::parse("ELI5"), Some(TutorMode::New));
    }
}
