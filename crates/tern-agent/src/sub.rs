//! Sub-sessions (`docs/02-ui-definition.md`, sidebar and message actions):
//! a chat hung under one message of another (`↳` in the sidebar), whose
//! messages read `Q12.1`, `Q12.2`, ….
//!
//! A sub-session can be **planned** (made with a question it has not run
//! yet) and run later in the background. When a run ends, a short summary
//! is posted back to the parent as `sub.reported`, so the parent's
//! transcript and its model both see what the branch found.

use serde_json::json;
use tern_core::{EffectClass, EventEnvelope, MessageRef, SessionId};
use tern_store::{SessionSummary, Store};

use crate::chat::{ChatOptions, ChatSession};
use crate::engine::{Approver, CancelFlag, TurnOutcome};
use crate::range::{clip, context_before};
use crate::transcript::past_turns;

/// One row of `tern sub list` and the sidebar's `↳` rows.
#[derive(Clone, Debug, PartialEq, serde::Serialize)]
pub struct SubRow {
    pub session: SessionSummary,
    /// The parent message it hangs under.
    pub anchor: String,
    /// Its messages' IDs.
    pub refs: Vec<String>,
    /// It has a question waiting to run.
    pub planned: bool,
    /// It has posted a summary back to the parent.
    pub reported: bool,
}

fn err(e: tern_store::StoreError) -> String {
    e.to_string()
}

/// Makes a sub-session of `parent` hung under message `anchor`. It starts
/// with the parent's conversation up to and including that message as
/// carried-over context. `plan` makes it a planned sub-session.
pub fn create(
    store: &mut Store,
    parent: SessionId,
    anchor: u32,
    title: Option<&str>,
    plan: Option<&str>,
) -> Result<SessionSummary, String> {
    let top = crate::refs::top_of(store, parent)?;
    let events = store.replay(top.id, 0).map_err(err)?;
    let turns = past_turns(&events);
    if !turns.iter().any(|t| t.number == anchor && !t.removed) {
        return Err(format!(
            "This chat has no Q{anchor} to branch from (it has {} turns).",
            turns.len()
        ));
    }
    let plan = plan.map(str::trim).filter(|p| !p.is_empty());
    let title = title
        .map(str::to_owned)
        .or_else(|| plan.map(crate::chat::title_from))
        .unwrap_or_else(|| format!("From Q{anchor}"));
    let id = store
        .create_sub_session(top.id, anchor, &title, plan)
        .map_err(err)?;
    store
        .append(&EventEnvelope::new(
            id,
            "session.context",
            EffectClass::Analysis,
            format!("Context carried over from {} through Q{anchor}", top.title),
            json!({
                "markdown": context_before(&top.title, &turns, anchor + 1),
                "from": top.id.to_string(),
                "anchor": anchor,
            }),
        ))
        .map_err(err)?;
    store
        .session(id)
        .map_err(err)?
        .ok_or_else(|| "The sub-session was not saved.".to_owned())
}

/// The sub-sessions of a chat (or of the chat a sub-session hangs under),
/// oldest first.
pub fn list(store: &Store, session: SessionId) -> Result<Vec<SubRow>, String> {
    let top = crate::refs::top_of(store, session)?;
    let parent_events = store.replay(top.id, 0).map_err(err)?;
    let reported: Vec<String> = parent_events
        .iter()
        .filter(|e| e.kind == "sub.reported")
        .filter_map(|e| e.payload["session"].as_str().map(str::to_owned))
        .collect();
    let mut out = Vec::new();
    for s in store.sub_sessions(top.id).map_err(err)? {
        let refs = crate::refs::refs_of(store, s.id)?
            .iter()
            .map(|r| r.to_string())
            .collect();
        out.push(SubRow {
            anchor: format!("Q{}", s.anchor.unwrap_or_default()),
            refs,
            planned: s.plan.is_some(),
            reported: reported.contains(&s.id.to_string()),
            session: s,
        });
    }
    Ok(out)
}

/// The sub-session a `tern sub run` argument names: a sub-session id (or
/// the start of one), or a message ID such as `Q12.3` in `chat`.
pub fn find(store: &Store, chat: SessionId, which: &str) -> Result<SessionId, String> {
    if let Ok(r) = which.parse::<MessageRef>() {
        if r.sub.is_some() {
            return Ok(crate::refs::resolve(store, chat, &r)?.session);
        }
        // `Q12`: the newest sub-session hung under it.
        let top = crate::refs::top_of(store, chat)?;
        return store
            .sub_sessions(top.id)
            .map_err(err)?
            .into_iter()
            .rev()
            .find(|s| s.anchor == Some(r.number))
            .map(|s| s.id)
            .ok_or_else(|| format!("{r} has no sub-sessions."));
    }
    crate::report::find_session(store, which)
}

/// A summary for the parent: the answer's opening, without its link list.
pub fn summarize(answer: &str) -> String {
    let body: Vec<&str> = answer
        .lines()
        .take_while(|l| {
            let l = l.trim().to_ascii_lowercase();
            !(l.starts_with("sources") || l.starts_with("## sources") || l.starts_with("links"))
        })
        .collect();
    clip(&body.join("\n"), 600)
}

/// Posts a finished sub-session run's summary back to its parent.
pub fn report_back(
    store: &mut Store,
    sub: SessionId,
    question: &str,
    out: &TurnOutcome,
) -> Result<Option<String>, String> {
    let s = store
        .session(sub)
        .map_err(err)?
        .ok_or("That sub-session is no longer in the database.")?;
    let (Some(parent), Some(anchor)) = (s.parent, s.anchor) else {
        return Ok(None);
    };
    let summary = match (&out.answer, &out.reason) {
        (Some(a), _) if !a.trim().is_empty() => summarize(a),
        (_, Some(why)) => format!("The run stopped: {why}"),
        _ => "The run ended without an answer.".to_owned(),
    };
    let refs: Vec<String> = crate::refs::refs_of(store, sub)?
        .iter()
        .map(|r| r.to_string())
        .collect();
    let id = refs
        .last()
        .cloned()
        .unwrap_or_else(|| format!("Q{anchor}.?"));
    store
        .append(&EventEnvelope::new(
            parent,
            "sub.reported",
            EffectClass::Analysis,
            format!("{id} reported back: {}", clip(&summary, 80)),
            json!({
                "anchor": anchor,
                "session": sub.to_string(),
                "title": s.title,
                "refs": refs,
                "question": question,
                "summary": summary,
            }),
        ))
        .map_err(err)?;
    store.set_session_plan(sub, None).map_err(err)?;
    Ok(Some(summary))
}

/// What a sub-session run did.
#[derive(Clone, Debug)]
pub struct SubRun {
    pub session: SessionId,
    pub question: String,
    pub outcome: TurnOutcome,
    /// The summary posted to the parent, when it has one.
    pub summary: Option<String>,
}

/// Runs a sub-session's planned question (or `question`) in its own chat
/// and reports back to the parent. The parent can keep going meanwhile.
pub async fn run_in(
    mut chat: ChatSession,
    question: Option<&str>,
    approver: &dyn Approver,
    cancel: CancelFlag,
    observer: &(dyn Fn(&EventEnvelope) + Send + Sync),
) -> Result<SubRun, String> {
    let sub = chat.session_id();
    let planned = chat
        .with_store(|s| s.session(sub))?
        .map_err(err)?
        .and_then(|s| s.plan);
    let question = question
        .map(str::to_owned)
        .or(planned)
        .ok_or("This sub-session has no planned question; give one.")?;
    let outcome = chat.send(&question, approver, cancel, observer).await;
    let summary = chat
        .with_store_mut(|s| report_back(s, sub, &question, &outcome))?
        .ok()
        .flatten();
    Ok(SubRun {
        session: sub,
        question,
        outcome,
        summary,
    })
}

/// [`run_in`] on the user's database with their saved choices.
pub async fn run(
    opts: ChatOptions,
    sub: SessionId,
    question: Option<&str>,
    approver: &dyn Approver,
    cancel: CancelFlag,
    observer: &(dyn Fn(&EventEnvelope) + Send + Sync),
) -> Result<SubRun, String> {
    let (chat, _) = ChatSession::resume(opts, sub).await?;
    run_in(chat, question, approver, cancel, observer).await
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::chat::INBOX;
    use crate::transcript::live_turns;
    use tern_core::{TurnId, TurnState};
    use tern_store::Harness;

    fn turn(store: &mut Store, s: SessionId, q: &str, a: &str) {
        for (kind, payload) in [
            ("turn.started", json!({ "message": q })),
            ("turn.finished", json!({"state": "completed", "answer": a})),
        ] {
            store
                .append(&EventEnvelope::new(
                    s,
                    kind,
                    EffectClass::Analysis,
                    kind,
                    payload,
                ))
                .unwrap();
        }
    }

    #[test]
    fn planned_sub_sessions_report_back_to_the_parent() {
        let mut store = Store::open_in_memory().unwrap();
        let p = store.ensure_project(INBOX).unwrap();
        let top = store.create_session(p, Harness::Research, "RAG").unwrap();
        turn(
            &mut store,
            top,
            "What is RAG?",
            "Retrieval plus generation.",
        );
        turn(&mut store, top, "Costs?", "It depends.");
        assert!(create(&mut store, top, 7, None, None).is_err());
        let sub = create(&mut store, top, 2, None, Some("Compare vector DB prices")).unwrap();
        assert_eq!(sub.title, "Compare vector DB prices");
        assert_eq!(sub.anchor, Some(2));
        let rows = list(&store, top).unwrap();
        assert_eq!(rows.len(), 1);
        assert!(rows[0].planned && !rows[0].reported);
        assert_eq!(rows[0].anchor, "Q2");

        // The run itself (a model call) is covered in tests/continuity.rs;
        // here the turn is logged by hand and reported back.
        turn(
            &mut store,
            sub.id,
            "Compare vector DB prices",
            "Pinecone costs more.\n\nSources\n[1] x",
        );
        let out = TurnOutcome {
            turn_id: TurnId::new(),
            state: TurnState::Completed,
            answer: Some("Pinecone costs more.\n\nSources\n[1] x".into()),
            reason: None,
            states: Vec::new(),
            usage: tern_providers::Usage::measured(0, 0),
            model_calls: 1,
            tool_calls: 0,
        };
        let summary = report_back(&mut store, sub.id, "Compare vector DB prices", &out)
            .unwrap()
            .unwrap();
        assert_eq!(summary, "Pinecone costs more.");
        let parent = live_turns(&store.replay(top, 0).unwrap());
        assert_eq!(parent[1].reports.len(), 1);
        assert_eq!(parent[1].reports[0].refs, ["Q2.1"]);
        let rows = list(&store, top).unwrap();
        assert!(!rows[0].planned && rows[0].reported);
        assert_eq!(find(&store, top, "Q2.1").unwrap(), sub.id);
        assert_eq!(find(&store, top, "Q2").unwrap(), sub.id);
        assert!(find(&store, top, "Q1").is_err());
        // The sub-session opens with the parent's context through Q2.
        let mine = past_turns(&store.replay(sub.id, 0).unwrap());
        let ctx = mine[0].context.clone().unwrap();
        assert!(ctx.contains("Costs?") && ctx.contains("What is RAG?"));
    }
}
