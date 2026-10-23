//! Rebuilds a chat's transcript from its event log, so a past chat can be
//! reopened. Since schema v2 the log keeps each turn's message and answer;
//! older turns come back without their text.
//!
//! Each turn has a stable number (`Q3` is the third turn the log shows
//! starting). Removing messages by ID range appends `range.removed` and
//! never touches the log's earlier events, so [`live_turns`] is what the
//! transcript and the model's history read, and the numbers of the turns
//! that stay never change. A sub-session's report back (`sub.reported`)
//! joins the turn it branched from, and carried-over context
//! (`session.context`, from a range move or a handoff packet) joins the
//! first turn after it.

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
    /// Its stable number in this session: 1 for the first turn logged.
    pub number: u32,
    /// Removed by ID range (in the trash, restorable).
    pub removed: bool,
    /// Sub-sessions that branched from this turn and reported back.
    pub reports: Vec<SubReport>,
    /// Context carried over just before this turn (a range move or a
    /// handoff packet), as Markdown.
    pub context: Option<String>,
}

/// A sub-session's summary posted back to its parent.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct SubReport {
    pub session: String,
    /// The sub-session's message ids, `Q12.1`, ….
    pub refs: Vec<String>,
    pub question: String,
    pub summary: String,
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
    let mut out = group(events);
    for (i, t) in out.iter_mut().enumerate() {
        t.number = i as u32 + 1;
    }
    let removed = removed_numbers(events);
    let mut context: Option<String> = None;
    let mut started = 0u32;
    for e in events {
        match e.kind.as_str() {
            "turn.started" => {
                started += 1;
                if let Some(c) = context.take()
                    && let Some(t) = out.get_mut(started as usize - 1)
                {
                    t.context = Some(c);
                }
            }
            "session.context" => {
                context = text_at(&e.payload, "markdown");
            }
            "sub.reported" => {
                let anchor = e.payload["anchor"].as_u64().unwrap_or_default() as usize;
                if let Some(t) = anchor.checked_sub(1).and_then(|i| out.get_mut(i)) {
                    t.reports.push(SubReport {
                        session: text_at(&e.payload, "session").unwrap_or_default(),
                        refs: e.payload["refs"]
                            .as_array()
                            .map(|a| {
                                a.iter()
                                    .filter_map(|r| r.as_str().map(str::to_owned))
                                    .collect()
                            })
                            .unwrap_or_default(),
                        question: text_at(&e.payload, "question").unwrap_or_default(),
                        summary: text_at(&e.payload, "summary").unwrap_or_default(),
                    });
                }
            }
            _ => {}
        }
    }
    for t in &mut out {
        t.removed = removed.contains(&t.number);
    }
    out
}

/// The turns the transcript shows and the model reads: [`past_turns`]
/// without the ones removed by ID range.
pub fn live_turns(events: &[EventEnvelope]) -> Vec<PastTurn> {
    past_turns(events)
        .into_iter()
        .filter(|t| !t.removed)
        .collect()
}

/// Context carried into the session (`session.context`) that no turn
/// follows yet, such as a session started from a handoff packet.
pub fn trailing_context(events: &[EventEnvelope]) -> Option<String> {
    let mut context = None;
    for e in events {
        match e.kind.as_str() {
            "turn.started" => context = None,
            "session.context" => context = text_at(&e.payload, "markdown"),
            _ => {}
        }
    }
    context
}

/// Turn numbers removed by ID range and not restored, in the order the
/// log removed them.
pub fn removed_numbers(events: &[EventEnvelope]) -> Vec<u32> {
    let mut out: Vec<u32> = Vec::new();
    for e in events {
        let nums = || {
            e.payload["numbers"]
                .as_array()
                .map(|a| {
                    a.iter()
                        .filter_map(|n| n.as_u64().map(|n| n as u32))
                        .collect::<Vec<_>>()
                })
                .unwrap_or_default()
        };
        match e.kind.as_str() {
            "range.removed" => {
                for n in nums() {
                    if !out.contains(&n) {
                        out.push(n);
                    }
                }
            }
            "range.restored" => {
                let back = nums();
                out.retain(|n| !back.contains(n));
            }
            _ => {}
        }
    }
    out
}

fn group(events: &[EventEnvelope]) -> Vec<PastTurn> {
    let mut out = Vec::new();
    let mut open: Option<(PastTurn, time::OffsetDateTime)> = None;
    // Decisions made just before a turn starts (the first-tool hint)
    // belong to that turn.
    let mut early: Vec<&EventEnvelope> = Vec::new();
    for e in events {
        if e.kind == "decision.made" && open.is_none() {
            early.push(e);
            continue;
        }
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
            for d in early.drain(..) {
                turn.run.push(d);
            }
            open = Some((turn, e.ts));
            continue;
        }
        let Some((turn, started)) = open.as_mut() else {
            // A turn's claims are checked after it ends.
            if e.kind.starts_with("claims.")
                && let Some(last) = out.last_mut()
            {
                last.run.push(e);
            }
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
            ev(
                s,
                "claims.checked",
                "Checked 0 claims",
                json!({"checks": [], "report": {"markdown": "/r/turn-1.md", "json": "/r/turn-1.json"}}),
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
        assert_eq!(
            t[0].run.report.as_ref().map(|r| r.markdown.clone()),
            Some("/r/turn-1.md".into()),
            "checks logged after the turn ends join it"
        );
        assert_eq!(t[1].stop.as_deref(), Some("You stopped the run."));
        assert_eq!(t[1].exchange(), None);
        assert_eq!(
            (t[2].message.as_deref(), t[2].answer.as_deref()),
            (None, None)
        );
        assert_eq!(t[3].stop.as_deref(), Some("This turn did not finish."));
        assert_eq!(t.iter().map(|t| t.number).collect::<Vec<_>>(), [1, 2, 3, 4]);
    }

    #[test]
    fn removed_turns_keep_the_others_numbers_and_come_back() {
        let s = SessionId::new();
        let mut events = Vec::new();
        for q in ["a", "b", "c"] {
            events.push(ev(s, "turn.started", "Started", json!({ "message": q })));
            events.push(ev(
                s,
                "turn.finished",
                "Answered",
                json!({"state": "completed", "answer": q}),
            ));
        }
        events.push(ev(
            s,
            "range.removed",
            "Removed Q2",
            json!({"numbers": [2], "reason": "removed"}),
        ));
        events.push(ev(
            s,
            "sub.reported",
            "Q3.1 reported back",
            json!({"anchor": 3, "session": "ses_x", "refs": ["Q3.1"], "question": "q", "summary": "s"}),
        ));
        let live = live_turns(&events);
        assert_eq!(live.iter().map(|t| t.number).collect::<Vec<_>>(), [1, 3]);
        assert_eq!(live[1].reports[0].refs, ["Q3.1"]);
        assert_eq!(removed_numbers(&events), [2]);
        events.push(ev(
            s,
            "range.restored",
            "Restored Q2",
            json!({"numbers": [2]}),
        ));
        assert_eq!(live_turns(&events).len(), 3);
        assert!(removed_numbers(&events).is_empty());
        let mut with_context = vec![ev(
            s,
            "session.context",
            "Carried over",
            json!({"markdown": "# Context"}),
        )];
        assert_eq!(
            trailing_context(&with_context).as_deref(),
            Some("# Context")
        );
        with_context.extend(events);
        let t = past_turns(&with_context);
        assert_eq!(t[0].context.as_deref(), Some("# Context"));
        assert_eq!(trailing_context(&with_context), None);
    }
}
