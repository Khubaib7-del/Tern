//! Branching a chat (`docs/01-product-spec.md`, multi-branch): a new chat
//! that starts with a copy of the first turns of another, so the user can
//! take the conversation a different way. The source chat is not touched;
//! its log stays append-only.

use tern_core::{EventEnvelope, EventId, SessionId};
use tern_platform::AppPaths;
use tern_store::Store;

use crate::chat::INBOX;

const ENDS: [&str; 3] = ["turn.finished", "run.cancelled", "budget.exceeded"];

/// The events of the first `turns` turns of a log, ready to copy.
pub fn first_turns(events: &[EventEnvelope], turns: usize) -> &[EventEnvelope] {
    let mut seen = 0;
    for (i, e) in events.iter().enumerate() {
        if ENDS.contains(&e.kind.as_str()) {
            seen += 1;
            if seen == turns {
                return &events[..=i];
            }
        }
    }
    events
}

/// The title of a branch of `title`.
pub fn branch_title(title: &str) -> String {
    let base = title.strip_prefix("Branch of ").unwrap_or(title);
    format!("Branch of {base}")
}

/// Makes the branch in `store`: a new session holding a copy of the first
/// `turns` turns of `from`. Returns its id.
pub fn branch_in(store: &mut Store, from: SessionId, turns: usize) -> Result<SessionId, String> {
    let source = store
        .session(from)
        .map_err(|e| e.to_string())?
        .ok_or("That chat is no longer in the database.")?;
    let events = store.replay(from, 0).map_err(|e| e.to_string())?;
    let project = store.ensure_project(INBOX).map_err(|e| e.to_string())?;
    let id = store
        .create_session(project, source.harness, &branch_title(&source.title))
        .map_err(|e| e.to_string())?;
    for e in first_turns(&events, turns) {
        let mut copy = e.clone();
        copy.session_id = id;
        copy.event_id = EventId::new();
        store.append(&copy).map_err(|e| e.to_string())?;
    }
    Ok(id)
}

/// [`branch_in`] on the user's database.
pub fn branch_session(from: SessionId, turns: usize) -> Result<SessionId, String> {
    let paths = AppPaths::discover().map_err(|e| e.to_string())?;
    let mut store = Store::open(&paths.database()).map_err(|e| e.to_string())?;
    branch_in(&mut store, from, turns)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::transcript::past_turns;
    use serde_json::json;
    use tern_core::EffectClass;
    use tern_store::Harness;

    fn ev(session: SessionId, kind: &str, payload: serde_json::Value) -> EventEnvelope {
        EventEnvelope::new(session, kind, EffectClass::Analysis, kind, payload)
    }

    fn turn(store: &mut Store, s: SessionId, q: &str, a: &str) {
        store
            .append(&ev(s, "turn.started", json!({ "message": q })))
            .unwrap();
        store
            .append(&ev(
                s,
                "turn.finished",
                json!({ "state": "completed", "answer": a }),
            ))
            .unwrap();
    }

    #[test]
    fn a_branch_copies_the_first_turns_only() {
        let mut store = Store::open_in_memory().unwrap();
        let p = store.ensure_project(INBOX).unwrap();
        let s = store.create_session(p, Harness::Research, "RAG").unwrap();
        turn(&mut store, s, "q1", "a1");
        turn(&mut store, s, "q2", "a2");
        turn(&mut store, s, "q3", "a3");

        let b = branch_in(&mut store, s, 2).unwrap();
        let summary = store.session(b).unwrap().unwrap();
        assert_eq!(summary.title, "Branch of RAG");
        let turns = past_turns(&store.replay(b, 0).unwrap());
        let answers: Vec<_> = turns.iter().filter_map(|t| t.answer.clone()).collect();
        assert_eq!(answers, vec!["a1", "a2"]);
        assert_eq!(
            store.replay(s, 0).unwrap().len(),
            6,
            "the source is untouched"
        );
        assert_eq!(branch_title("Branch of RAG"), "Branch of RAG");
    }
}
