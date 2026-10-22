//! Rebuilds a chat's transcript from its event log, so a past chat can be
//! reopened. Since schema v2 the log keeps each turn's message and answer;
//! older turns come back without their text.

use std::time::Duration;

use tern_core::EventEnvelope;

use crate::run::Run;
use crate::sources::{PageRef, page_of, push_unique};

/// One past turn.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct PastTurn {
    /// What the user sent. `None` for turns logged before Tern kept it.
    pub message: Option<String>,
    pub answer: Option<String>,
    /// Why the turn ended without a full answer (stopped, failed, over budget).
    pub stop: Option<String>,
    /// The turn's events, start to end, for the run panel.
    pub run: Run,
    /// Pages the turn read, in order, without repeats.
    pub pages: Vec<PageRef>,
    pub took: Duration,
}

impl PastTurn {
    /// Whether this turn can join the model's history on reopen.
    pub fn exchange(&self) -> Option<(&str, &str)> {
        match (&self.message, &self.answer) {
            (Some(m), Some(a)) if !m.is_empty() && !a.is_empty() && self.stop.is_none() => {
                Some((m, a))
            }
            _ => None,
        }
    }
}

const ENDS: [&str; 3] = ["turn.finished", "run.cancelled", "budget.exceeded"];

/// Groups a session's events into turns, oldest first. A turn the log shows
/// starting but never ending (the app closed mid-turn) is kept with a note.
pub fn past_turns(events: &[EventEnvelope]) -> Vec<PastTurn> {
    let mut out = Vec::new();
    let mut open: Option<(PastTurn, time::OffsetDateTime)> = None;
    for e in events {
        if e.kind == "turn.started" {
            if let Some((mut t, _)) = open.take() {
                t.stop = Some("This turn did not finish.".into());
                out.push(t);
            }
            let mut turn = PastTurn {
                message: text_at(&e.payload, "message"),
                ..PastTurn::default()
            };
            turn.run.push(e);
            open = Some((turn, e.ts));
            continue;
        }
        let Some((turn, started)) = open.as_mut() else {
            continue;
        };
        turn.run.push(e);
        if ENDS.contains(&e.kind.as_str()) {
            turn.answer = text_at(&e.payload, "answer");
            let completed = e.kind == "turn.finished"
                && e.payload
                    .get("state")
                    .and_then(|s| s.as_str())
                    .is_some_and(|s| s.starts_with("completed"));
            if !completed {
                turn.stop = Some(e.user_summary.clone());
            }
            turn.took = (e.ts - *started).try_into().unwrap_or_default();
            out.push(std::mem::take(turn));
            open = None;
        } else {
            if let Some(p) = page_of(&e.kind, &e.payload) {
                push_unique(&mut turn.pages, p);
            }
        }
    }
    if let Some((mut t, _)) = open {
        t.stop = Some("This turn did not finish.".into());
        out.push(t);
    }
    out
}

fn text_at(v: &serde_json::Value, key: &str) -> Option<String> {
    v.get(key)
        .and_then(|s| s.as_str())
        .filter(|s| !s.is_empty())
        .map(str::to_owned)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use tern_core::{EffectClass, SessionId};

    fn ev(ses: SessionId, kind: &str, summary: &str, payload: serde_json::Value) -> EventEnvelope {
        EventEnvelope::new(ses, kind, EffectClass::Analysis, summary, payload)
    }

    #[test]
    fn groups_turns_and_keeps_stops() {
        let s = SessionId::new();
        let events = vec![
            ev(s, "session.opened", "Opened", json!({})),
            ev(
                s,
                "turn.started",
                "Started",
                json!({"message": "What is RAG?"}),
            ),
            ev(s, "tool.executed", "Searched the web", json!({})),
            ev(
                s,
                "tool.executed",
                "Ran `read_url`",
                json!({"meta": {"page": {"title": "RAG", "url": "https://a.org/rag", "site": "a.org"}}}),
            ),
            ev(
                s,
                "turn.finished",
                "Answered",
                json!({"state": "completed", "answer": "Retrieval..."}),
            ),
            ev(s, "turn.started", "Started", json!({"message": "More"})),
            ev(
                s,
                "run.cancelled",
                "You stopped the run.",
                json!({"state": "cancelled"}),
            ),
            ev(s, "turn.started", "Started", json!({})),
            ev(
                s,
                "turn.finished",
                "Answered",
                json!({"state": "completed", "answer_chars": 10}),
            ),
            ev(s, "turn.started", "Started", json!({"message": "Crash"})),
        ];
        let t = past_turns(&events);
        assert_eq!(t.len(), 4);
        assert_eq!(t[0].exchange(), Some(("What is RAG?", "Retrieval...")));
        assert_eq!(t[0].run.steps[0].event, "turn.started");
        assert_eq!(t[0].run.steps[1].summary, "Searched the web");
        assert_eq!(t[0].run.end, Some(crate::run::RunEnd::Answered));
        assert_eq!(t[0].pages.len(), 1);
        assert_eq!(t[0].pages[0].url, "https://a.org/rag");
        assert_eq!(t[1].stop.as_deref(), Some("You stopped the run."));
        assert_eq!(t[1].exchange(), None);
        assert_eq!(
            (t[2].message.as_deref(), t[2].answer.as_deref()),
            (None, None)
        );
        assert_eq!(t[3].stop.as_deref(), Some("This turn did not finish."));
    }
}
