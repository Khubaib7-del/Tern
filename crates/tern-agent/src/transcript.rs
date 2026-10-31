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

use tern_core::{EventEnvelope, TurnDigest};

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
    /// The user message and assistant message this turn adds to the
    /// model's history on reopen, or `None` when it adds nothing (a turn
    /// logged before Tern kept the message).
    ///
    /// A turn that stopped before its answer (stopped, failed, over
    /// budget, or cut off when Tern closed) still joins (#85): its
    /// assistant message is the text that streamed before the stop, then
    /// a note saying how it stopped, which sources it read and which
    /// tools it ran ([`PastTurn::digest`]). "Continue" can then pick it
    /// up instead of starting over.
    pub fn exchange(&self) -> Option<(String, String)> {
        let m = self.message.as_deref().filter(|m| !m.is_empty())?;
        match (&self.answer, &self.stop) {
            (Some(a), None) if !a.is_empty() => Some((m.to_owned(), a.clone())),
            (_, Some(_)) => Some((m.to_owned(), self.digest()?.history_text())),
            _ => None,
        }
    }

    /// The digest of a turn that stopped before its answer: its question,
    /// the partial answer, the sources it read and the tools it ran.
    /// `None` for a turn that finished.
    pub fn digest(&self) -> Option<TurnDigest> {
        let stop = self.stop.as_ref()?;
        Some(TurnDigest {
            question: self.message.clone().unwrap_or_default(),
            partial: self.answer.clone().filter(|a| !a.trim().is_empty()),
            sources: self
                .pages
                .iter()
                .take(DIGEST_SOURCES)
                .map(|p| {
                    if p.title.trim().is_empty() {
                        p.url.clone()
                    } else {
                        format!("{} ({})", p.title.trim(), p.url)
                    }
                })
                .collect(),
            tools: self
                .run
                .steps
                .iter()
                .filter(|s| s.event == "tool.executed")
                .filter_map(|s| s.payload["tool"].as_str().map(str::to_owned))
                .collect(),
            stop: stop.clone(),
        })
    }
}

/// Sources an interrupted turn's digest names at most.
const DIGEST_SOURCES: usize = 12;

/// The event that carries reply text as it streams (#85), every second
/// or 200 characters, so a turn cut off by a crash keeps what arrived.
pub const ANSWER_PROGRESS: &str = "answer.progress";

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
    // Reply text that streamed in the open turn (#85): the model call it
    // came from and the text so far.
    let mut progress: (u64, String) = (0, String::new());
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
                if t.answer.is_none() && !progress.1.trim().is_empty() {
                    t.answer = Some(std::mem::take(&mut progress.1));
                }
                out.push(t);
            }
            progress = (0, String::new());
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
        if e.kind == ANSWER_PROGRESS {
            let call = e.payload["call"].as_u64().unwrap_or_default();
            if call != progress.0 {
                progress = (call, String::new());
            }
            if let Some(t) = e.payload["text"].as_str() {
                progress.1.push_str(t);
            }
            continue;
        }
        turn.run.push(e);
        if ENDS.contains(&e.kind.as_str()) {
            // A run stopped mid-reply keeps what streamed up to the stop
            // (#71), with the stop still recorded.
            turn.answer = text_at(&e.payload, "answer").or_else(|| text_at(&e.payload, "partial"));
            progress = (0, String::new());
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
        if t.answer.is_none() && !progress.1.trim().is_empty() {
            t.answer = Some(progress.1);
        }
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
        assert_eq!(
            t[0].exchange(),
            Some(("What is RAG?".to_owned(), "Retrieval...".to_owned()))
        );
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
        // A stopped turn still joins the history, with a note (#85).
        let (m, a) = t[1].exchange().expect("a stopped turn joins the history");
        assert_eq!(m, "More");
        assert!(
            a.contains("stopped before it finished: You stopped the run."),
            "{a}"
        );
        assert_eq!(t[2].exchange(), None, "no message logged");
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

    /// #85 (A1): a turn cut off by a crash keeps the text that streamed,
    /// and an interrupted turn joins the history with its sources and the
    /// tools it ran.
    #[test]
    fn interrupted_turns_keep_their_partial_text_and_join_the_history() {
        let s = SessionId::new();
        let events = vec![
            ev(
                s,
                "turn.started",
                "Started",
                json!({"message": "What is FLARE?"}),
            ),
            ev(
                s,
                "tool.executed",
                "Ran `read_url`",
                json!({"tool": "read_url", "meta": {"page": {"title": "FLARE", "url": "https://a.org/flare", "site": "a.org"}}}),
            ),
            ev(
                s,
                ANSWER_PROGRESS,
                "",
                json!({"call": 1, "text": "Draft that is "}),
            ),
            ev(
                s,
                ANSWER_PROGRESS,
                "",
                json!({"call": 2, "text": "FLARE retrieves "}),
            ),
            ev(
                s,
                ANSWER_PROGRESS,
                "",
                json!({"call": 2, "text": "when unsure."}),
            ),
        ];
        let t = past_turns(&events);
        assert_eq!(t.len(), 1);
        assert_eq!(t[0].answer.as_deref(), Some("FLARE retrieves when unsure."));
        assert_eq!(t[0].stop.as_deref(), Some("This turn did not finish."));
        assert!(
            t[0].run.steps.iter().all(|s| s.event != ANSWER_PROGRESS),
            "progress is not a step"
        );
        let (m, a) = t[0].exchange().unwrap();
        assert_eq!(m, "What is FLARE?");
        assert!(
            a.starts_with("FLARE retrieves when unsure.\n\n(Tern note:"),
            "{a}"
        );
        assert!(a.contains("FLARE (https://a.org/flare)"), "{a}");
        assert!(a.contains("Tools run: read_url."), "{a}");

        let failed = vec![
            ev(s, "turn.started", "Started", json!({"message": "Q"})),
            ev(
                s,
                "turn.finished",
                "Mistral is rate limiting this key.",
                json!({"state": "failed", "partial": "Half an answer"}),
            ),
        ];
        let t = past_turns(&failed);
        assert_eq!(t[0].answer.as_deref(), Some("Half an answer"));
        let (_, a) = t[0].exchange().unwrap();
        assert!(a.starts_with("Half an answer"), "{a}");
        assert!(a.contains("Mistral is rate limiting this key"), "{a}");
    }
}
