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

/// The share of the model's window the verbatim tail may take: the most
/// recent turns stay word for word only while they fit in this much, so a
/// small window (4k) still has older turns to fold even in a short chat.
pub const TAIL_SHARE: f32 = 0.35;

/// How full a known window must be before a compaction folds every turn
/// but the newest when that is what it takes to free room.
pub const PRESSURE: f32 = 0.5;

/// The tokens the verbatim tail may take in a window of `window` tokens.
pub fn tail_budget(window: Option<u64>) -> Option<u64> {
    window.map(|w| ((w as f32) * TAIL_SHARE) as u64)
}

/// A turn's size as the model sees it, at four characters a token (the
/// same estimate the context ring uses before the provider counts).
pub fn turn_tokens(t: &PastTurn) -> u64 {
    let chars = t.message.as_deref().map_or(0, str::len)
        + t.answer.as_deref().map_or(0, str::len)
        + t.context.as_deref().map_or(0, str::len);
    (chars as u64).div_ceil(4)
}

/// Why a compaction found nothing to fold, in words the user reads.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Skip {
    /// No finished exchange yet.
    NoTurns,
    /// Every earlier turn is already in the summary; only the last is left.
    OnlyLast,
    /// Every unsummarised turn is in the recent tail and fits.
    AllFit,
    /// The older turns carry attachments that fit and stay word for word.
    Attachments,
}

impl Skip {
    /// The toast. `overhead` is the share of the window Tern's own
    /// instructions and tools take, when known: when they fill most of a
    /// small window, folding the chat cannot free much, and the user should
    /// know why the ring stays high.
    pub fn message(self, overhead: Option<f32>) -> String {
        let base = match self {
            Self::NoTurns => "Nothing to compact yet: this chat has no finished turns.",
            Self::OnlyLast => {
                "Nothing to compact: only your last message is left, and it already fits."
            }
            Self::AllFit => "Nothing to compact yet: your recent turns already fit word for word.",
            Self::Attachments => {
                "Nothing to compact: the older turns hold attachments, and they still fit word for word."
            }
        };
        match overhead {
            Some(o) if o >= 0.5 => format!(
                "{base} Tern's instructions and tools take about {}% of this model's window; \
                 a model with a larger window leaves more room.",
                (o * 100.).round() as u32
            ),
            _ => base.to_owned(),
        }
    }
}

/// What one compaction folds, and how many turns stay word for word.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Plan {
    /// Turns to fold now, oldest first.
    pub fold: Vec<u32>,
    /// Turns kept word for word at the end (the recent tail).
    pub kept: usize,
    /// Why nothing folds, when `fold` is empty.
    pub skip: Option<Skip>,
}

/// The turns to fold now: finished exchanges not yet summarised, except
/// a recent tail kept word for word and attachments that still fit.
///
/// The tail is bounded twice: at most `keep` turns, and at most
/// [`tail_budget`] tokens of the model's `window` when it is known. The
/// newest turn always stays. Under `pressure` (the context is at least
/// [`PRESSURE`] of a known window, or the threshold started it) and with
/// two or more turns unsummarised, the tail never takes them all, so a
/// compaction always frees something in a short chat on a small window.
/// Without a known window it is the turn count alone, as before.
pub fn plan_for(
    turns: &[PastTurn],
    prior: Option<&Compacted>,
    keep: usize,
    window: Option<u64>,
    pressure: bool,
) -> Plan {
    let done = |n: u32| prior.is_some_and(|c| c.turns.contains(&n));
    let live: Vec<&PastTurn> = turns
        .iter()
        .filter(|t| !t.removed && t.exchange().is_some() && !done(t.number))
        .collect();
    if live.is_empty() {
        let skip = if prior.is_some_and(|c| !c.turns.is_empty()) {
            Skip::OnlyLast
        } else {
            Skip::NoTurns
        };
        return Plan {
            skip: Some(skip),
            ..Plan::default()
        };
    }
    let budget = tail_budget(window);
    let cap = keep.max(1);
    // The tail, newest first: the newest always, then while it fits.
    let mut used = 0u64;
    let mut kept = 0usize;
    for t in live.iter().rev() {
        let tok = turn_tokens(t);
        let fits = budget.is_none_or(|b| used + tok <= b);
        if kept == 0 || (kept < cap && fits) {
            used += tok;
            kept += 1;
        } else {
            break;
        }
    }
    // Under pressure a tail that would keep every turn (a short chat)
    // keeps only the newest, so the compaction frees room.
    if pressure && budget.is_some() && live.len() >= 2 && kept == live.len() {
        kept = 1;
        used = live.last().map_or(0, |t| turn_tokens(t));
    }
    let cut = live.len() - kept;
    // Older turns with attachments stay while they fit too (newest first).
    let mut fold: Vec<u32> = Vec::new();
    let mut held = false;
    for t in live[..cut].iter().rev() {
        if has_attachment(t) {
            let tok = turn_tokens(t);
            if budget.is_none_or(|b| used + tok <= b) {
                used += tok;
                held = true;
                continue;
            }
        }
        fold.push(t.number);
    }
    fold.reverse();
    let skip = if !fold.is_empty() {
        None
    } else if held {
        Some(Skip::Attachments)
    } else if live.len() == 1 {
        Some(Skip::OnlyLast)
    } else {
        Some(Skip::AllFit)
    };
    Plan { fold, kept, skip }
}

/// [`plan_for`] by turn count alone (no known window).
pub fn plan(turns: &[PastTurn], prior: Option<&Compacted>, keep: usize) -> Vec<u32> {
    plan_for(turns, prior, keep, None, false).fold
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

    fn long_turn(n: u32, chars: usize) -> PastTurn {
        PastTurn {
            message: Some("q".repeat(chars / 2)),
            answer: Some("a".repeat(chars / 2)),
            number: n,
            ..PastTurn::default()
        }
    }

    #[test]
    fn a_4k_window_keeps_the_tail_by_tokens_not_turns() {
        // allam-2-7b: 4,096 tokens. The tail may take 35 %: 1,433 tokens.
        let w = Some(4096);
        assert_eq!(tail_budget(w), Some(1433));
        // Four turns of ~500 tokens each. Keep-4 by count would keep them
        // all; by tokens only two fit.
        let turns: Vec<PastTurn> = (1..=4).map(|n| long_turn(n, 2000)).collect();
        let p = plan_for(&turns, None, 4, w, false);
        assert_eq!(p.fold, [1, 2]);
        assert_eq!(p.kept, 2);
        assert_eq!(p.skip, None);
        // A last turn over the budget on its own still stays, and
        // everything before it folds.
        let mut turns = turns;
        turns[3] = long_turn(4, 8000);
        assert_eq!(plan_for(&turns, None, 4, w, false).fold, [1, 2, 3]);
    }

    #[test]
    fn a_short_chat_on_a_full_small_window_still_frees_room() {
        // Three short turns fit the 4k tail budget; at 77 % of the window
        // a press must still fold the older ones.
        let turns: Vec<PastTurn> = (1..=3).map(|n| turn(n, &format!("q{n}"))).collect();
        let w = Some(4096);
        let calm = plan_for(&turns, None, 4, w, false);
        assert!(calm.fold.is_empty());
        assert_eq!(calm.skip, Some(Skip::AllFit));
        let p = plan_for(&turns, None, 4, w, true);
        assert_eq!(p.fold, [1, 2]);
        assert_eq!(p.kept, 1);
        // Only the newest left: nothing to fold, and it says why.
        let prior = Compacted {
            turns: vec![1, 2],
            ..Default::default()
        };
        let p = plan_for(&turns, Some(&prior), 4, w, true);
        assert!(p.fold.is_empty());
        assert_eq!(p.skip, Some(Skip::OnlyLast));
        assert!(
            Skip::OnlyLast
                .message(None)
                .contains("only your last message")
        );
        // When Tern's own prompt fills most of the window, it says so.
        let m = Skip::OnlyLast.message(Some(0.7));
        assert!(m.contains("70%") && m.contains("larger window"), "{m}");
        assert_eq!(plan_for(&[], None, 4, w, true).skip, Some(Skip::NoTurns));
    }

    #[test]
    fn attachments_stay_only_while_they_fit_a_small_window() {
        let mut turns: Vec<PastTurn> = (1..=4).map(|n| turn(n, &format!("q{n}"))).collect();
        let big = format!(
            "<attachment name=\"a.pdf\">{}</attachment>",
            "x".repeat(20_000)
        );
        turns[0].message = Some(big);
        // Unknown window: kept word for word, as before.
        assert_eq!(plan_for(&turns, None, 2, None, false).fold, [2]);
        // 4k window: ~5,000 tokens cannot stay; it folds.
        assert_eq!(plan_for(&turns, None, 2, Some(4096), false).fold, [1, 2]);
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
