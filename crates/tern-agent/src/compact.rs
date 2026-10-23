//! Context compaction: when a chat's context passes a share of the model's
//! window (80 % unless the user changed it), the older turns are folded
//! into one pinned summary written by the chat's own model, and the model
//! sees that summary, the turns that carry attachments, and the last few
//! turns verbatim. The system prompt and tools are rebuilt on every call,
//! so they always go in whole.
//!
//! Nothing is deleted: the event log is append-only, so the folded turns
//! stay in the transcript and in the database. A `session.compacted`
//! event records the summary and which turns (by their stable numbers) it
//! stands for; reopening the chat rebuilds the same history from it.
//!
//! See `docs/context.md`.

use serde::{Deserialize, Serialize};
use tern_core::EventEnvelope;
use tern_providers::{ChatMessage, ChatRequest};

use crate::transcript::PastTurn;

/// The event kind.
pub const EVENT: &str = "session.compacted";

/// The pinned summary's opening line, as the model and the context
/// breakdown see it.
pub const PINNED_HEAD: &str = "Earlier in this chat (a summary of the turns before these):";
const PINNED_REPLY: &str = "Understood. I will continue from this summary.";

/// When and how much to compact.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Compaction {
    /// Compact on its own when the context passes `at`.
    pub auto: bool,
    /// The share of the window that starts a compaction, 0 to 1.
    pub at: f32,
    /// Turns kept word for word at the end.
    pub keep_turns: usize,
}

impl Default for Compaction {
    fn default() -> Self {
        Self {
            auto: true,
            at: 0.8,
            keep_turns: 4,
        }
    }
}

impl Compaction {
    /// From the preference fields: off, a percent, and a turn count.
    pub fn from_prefs(off: bool, percent: Option<u8>, keep: Option<usize>) -> Self {
        let d = Self::default();
        Self {
            auto: !off,
            at: percent.map_or(d.at, |p| f32::from(p.clamp(30, 98)) / 100.),
            keep_turns: keep.map_or(d.keep_turns, |k| k.clamp(1, 20)),
        }
    }

    /// Whether a context this full should compact on its own.
    pub fn due(&self, fraction: Option<f32>) -> bool {
        self.auto && fraction.is_some_and(|f| f >= self.at)
    }
}

/// One compaction, as logged.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Compacted {
    /// The summary the model wrote; it covers every turn in `turns`.
    pub summary: String,
    /// The turns the summary stands for, by number (earlier compactions'
    /// turns included, so the last event alone rebuilds the history).
    pub turns: Vec<u32>,
    /// Turns the model still sees word for word.
    pub kept: usize,
    /// Started by the threshold (else by "Compact now").
    pub auto: bool,
}

impl Compacted {
    pub fn event(&self, session: tern_core::SessionId) -> EventEnvelope {
        EventEnvelope::new(
            session,
            EVENT,
            tern_core::EffectClass::Analysis,
            format!("Compacted {} earlier turns", self.turns.len()),
            serde_json::to_value(self).unwrap_or_default(),
        )
    }
}

/// The last compaction in a chat's log.
pub fn last(events: &[EventEnvelope]) -> Option<Compacted> {
    events
        .iter()
        .rev()
        .find(|e| e.kind == EVENT)
        .and_then(|e| serde_json::from_value(e.payload.clone()).ok())
}

fn has_attachment(t: &PastTurn) -> bool {
    t.message
        .as_deref()
        .is_some_and(|m| m.contains("<attachment") || m.contains("<untrusted-data"))
}

/// The turns to fold now: finished exchanges not yet summarised, except
/// the last `keep` and those that carry attachments (kept verbatim).
pub fn plan(turns: &[PastTurn], prior: Option<&Compacted>, keep: usize) -> Vec<u32> {
    let done = |n: u32| prior.is_some_and(|c| c.turns.contains(&n));
    let live: Vec<&PastTurn> = turns
        .iter()
        .filter(|t| !t.removed && t.exchange().is_some() && !done(t.number))
        .collect();
    let cut = live.len().saturating_sub(keep);
    live[..cut]
        .iter()
        .filter(|t| !has_attachment(t))
        .map(|t| t.number)
        .collect()
}

/// The pinned pair the model sees in place of the folded turns.
pub fn pinned(summary: &str) -> [ChatMessage; 2] {
    [
        ChatMessage::user(format!("{PINNED_HEAD}\n\n{}", summary.trim())),
        ChatMessage::assistant(PINNED_REPLY),
    ]
}

/// Whether a history message is the pinned summary.
pub fn is_pinned(text: &str) -> bool {
    text.starts_with(PINNED_HEAD)
}

/// The model's history for a chat: as [`crate::chat::history_of`], with
/// the folded turns replaced by the pinned summary when the chat was
/// compacted.
pub fn history(
    turns: &[PastTurn],
    compacted: Option<&Compacted>,
    trailing: Option<&str>,
) -> Vec<ChatMessage> {
    match compacted {
        None => crate::chat::history_of(turns, trailing),
        Some(c) => {
            let rest: Vec<PastTurn> = turns
                .iter()
                .filter(|t| !c.turns.contains(&t.number))
                .cloned()
                .collect();
            let mut out: Vec<ChatMessage> = pinned(&c.summary).into();
            out.extend(crate::chat::history_of(&rest, trailing));
            out
        }
    }
}

/// The model call that writes the summary: the earlier summary, if any,
/// and the turns being folded, oldest first.
pub fn request(model: &str, prior: Option<&Compacted>, fold: &[&PastTurn]) -> ChatRequest {
    let mut text = String::new();
    if let Some(c) = prior {
        text.push_str("Summary so far:\n\n");
        text.push_str(c.summary.trim());
        text.push_str("\n\n");
    }
    text.push_str("Turns to fold in:\n\n");
    for t in fold {
        if let Some(c) = &t.context {
            text.push_str(&format!("(Context carried in)\n{}\n\n", c.trim()));
        }
        if let Some((m, a)) = t.exchange() {
            text.push_str(&format!(
                "Q{}. User: {}\nAssistant: {}\n\n",
                t.number,
                m.trim(),
                a.trim()
            ));
        }
    }
    let mut req = ChatRequest::new(model, vec![ChatMessage::user(text)]);
    req.system = Some(
        "Summarise the earlier part of this conversation so it can continue without it. \
         Keep every fact, figure, decision, open question, source link, and name the user \
         may rely on; keep the user's goals and preferences. Write compact Markdown bullets \
         grouped by topic, no preamble, under 400 words."
            .into(),
    );
    req.max_tokens = 1024;
    req
}

/// Everything the summary stands for after folding `fold` on top of
/// `prior`, in turn order.
pub fn merged(prior: Option<&Compacted>, fold: &[u32]) -> Vec<u32> {
    let mut all: Vec<u32> = prior.map(|c| c.turns.clone()).unwrap_or_default();
    all.extend_from_slice(fold);
    all.sort_unstable();
    all.dedup();
    all
}

#[cfg(test)]
mod tests {
    use super::*;

    fn turn(n: u32, message: &str) -> PastTurn {
        PastTurn {
            message: Some(message.into()),
            answer: Some(format!("answer {n}")),
            number: n,
            ..PastTurn::default()
        }
    }

    #[test]
    fn plan_keeps_the_last_turns_and_attachments() {
        let mut turns: Vec<PastTurn> = (1..=8).map(|n| turn(n, &format!("q{n}"))).collect();
        turns[1].message = Some("see <attachment name=\"a.pdf\">x</attachment>".into());
        turns[2].removed = true;
        let fold = plan(&turns, None, 3);
        // 1..8 minus removed 3 = 7 live; keep the last 3 (6, 7, 8); 2 has an
        // attachment.
        assert_eq!(fold, [1, 4, 5]);
        let prior = Compacted {
            turns: vec![1, 4, 5],
            ..Default::default()
        };
        assert!(
            plan(&turns, Some(&prior), 3).is_empty(),
            "nothing new to fold"
        );
        assert_eq!(merged(Some(&prior), &[6, 1]), [1, 4, 5, 6]);
    }

    #[test]
    fn history_pins_the_summary_first() {
        let turns: Vec<PastTurn> = (1..=3).map(|n| turn(n, &format!("q{n}"))).collect();
        let c = Compacted {
            summary: "- earlier".into(),
            turns: vec![1, 2],
            kept: 1,
            auto: true,
        };
        let h = history(&turns, Some(&c), None);
        assert_eq!(h.len(), 4);
        assert!(is_pinned(&h[0].content));
        assert!(h[0].content.contains("- earlier"));
        assert_eq!(h[2].content, "q3");
        assert_eq!(history(&turns, None, None).len(), 6);
    }

    #[test]
    fn settings_come_from_prefs_and_decide_when() {
        let d = Compaction::default();
        assert!(d.auto && (d.at - 0.8).abs() < 1e-6);
        assert!(d.due(Some(0.81)) && !d.due(Some(0.5)) && !d.due(None));
        let off = Compaction::from_prefs(true, Some(90), Some(2));
        assert!(!off.due(Some(0.99)));
        assert_eq!(off.keep_turns, 2);
        assert!((Compaction::from_prefs(false, Some(5), None).at - 0.3).abs() < 1e-6);
    }

    #[test]
    fn the_event_round_trips() {
        let c = Compacted {
            summary: "s".into(),
            turns: vec![1, 2],
            kept: 4,
            auto: false,
        };
        let e = c.event(tern_core::SessionId::new());
        assert_eq!(e.kind, EVENT);
        assert_eq!(last(&[e]), Some(c));
    }
}
