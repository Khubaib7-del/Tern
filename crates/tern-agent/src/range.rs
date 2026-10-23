//! Cleaning a chat by message ID range (`docs/04-roadmap.md`, Phase 9).
//!
//! The log is append-only, so nothing here deletes an event:
//!
//! - **Remove** appends `range.removed`; the turns leave the transcript and
//!   the model's history and sit in the chat's trash until `range.restored`.
//! - **Move** copies the turns, with the context before them, into a new
//!   session or a sub-session and appends `range.move_pending`. The
//!   original stays exactly as it was until the move is confirmed
//!   (`range.removed` with reason `moved`, so it is still restorable) or
//!   undone (the copy is archived).

use serde_json::json;
use tern_core::{EffectClass, EventEnvelope, EventId, MessageRef, SessionId};
use tern_store::Store;

use crate::chat::INBOX;
use crate::transcript::{PastTurn, past_turns, removed_numbers};

/// Where a move puts the messages.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum MoveTarget {
    /// A new top-level session.
    NewSession { title: Option<String> },
    /// A sub-session of the chat. `anchor` is the message it hangs under;
    /// `None` picks the message just before the first one moved.
    SubSession {
        anchor: Option<u32>,
        title: Option<String>,
    },
}

/// A move waiting for the user to confirm or undo it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PendingMove {
    pub from: SessionId,
    pub to: SessionId,
    pub numbers: Vec<u32>,
}

/// `Q3–Q5`, `Q3, Q7`, or `Q3` for messages.
pub fn label(numbers: &[u32]) -> String {
    let mut sorted = numbers.to_vec();
    sorted.sort_unstable();
    sorted.dedup();
    let contiguous = sorted.windows(2).all(|w| w[1] == w[0] + 1);
    match sorted.as_slice() {
        [] => String::new(),
        [one] => format!("Q{one}"),
        [first, .., last] if contiguous => format!("Q{first}–Q{last}"),
        _ => sorted
            .iter()
            .map(|n| format!("Q{n}"))
            .collect::<Vec<_>>()
            .join(", "),
    }
}

/// Top-level numbers from parsed IDs. Sub-IDs are refused: open the
/// sub-session to clean it.
pub fn numbers_of(refs: &[MessageRef]) -> Result<Vec<u32>, String> {
    refs.iter()
        .map(|r| match r.sub {
            None => Ok(r.number),
            Some(_) => Err(format!(
                "{r} is in a sub-session. Clean it from there (`tern range rm Q12.1 --chat <sub-session>` uses its own numbers)."
            )),
        })
        .collect()
}

fn store_err(e: tern_store::StoreError) -> String {
    e.to_string()
}

fn log(
    store: &mut Store,
    session: SessionId,
    kind: &str,
    summary: String,
    payload: serde_json::Value,
) -> Result<(), String> {
    store
        .append(&EventEnvelope::new(
            session,
            kind,
            EffectClass::Analysis,
            summary,
            payload,
        ))
        .map(|_| ())
        .map_err(store_err)
}

/// The turns `numbers` name, all present and not removed yet.
fn live_selection(events: &[EventEnvelope], numbers: &[u32]) -> Result<Vec<PastTurn>, String> {
    if numbers.is_empty() {
        return Err("Name the messages: Q3, Q3-Q5, or Q3,Q7.".into());
    }
    let turns = past_turns(events);
    let mut picked = Vec::new();
    for n in numbers {
        let t = turns
            .iter()
            .find(|t| t.number == *n)
            .ok_or_else(|| format!("This chat has no Q{n} (it has {} turns).", turns.len()))?;
        if t.removed {
            return Err(format!("Q{n} is already in the trash."));
        }
        picked.push(t.clone());
    }
    Ok(picked)
}

/// Removes messages to the chat's trash. Returns the numbers removed.
pub fn remove(store: &mut Store, session: SessionId, numbers: &[u32]) -> Result<Vec<u32>, String> {
    let events = store.replay(session, 0).map_err(store_err)?;
    live_selection(&events, numbers)?;
    let mut nums = numbers.to_vec();
    nums.sort_unstable();
    nums.dedup();
    log(
        store,
        session,
        "range.removed",
        format!("Removed {} to the trash", label(&nums)),
        json!({"numbers": nums, "reason": "removed"}),
    )?;
    Ok(nums)
}

/// Brings removed messages back; `None` restores every one.
pub fn restore(
    store: &mut Store,
    session: SessionId,
    numbers: Option<&[u32]>,
) -> Result<Vec<u32>, String> {
    let events = store.replay(session, 0).map_err(store_err)?;
    let removed = removed_numbers(&events);
    let mut back: Vec<u32> = match numbers {
        None => removed.clone(),
        Some(ns) => {
            for n in ns {
                if !removed.contains(n) {
                    return Err(format!("Q{n} is not in this chat's trash."));
                }
            }
            ns.to_vec()
        }
    };
    back.sort_unstable();
    back.dedup();
    if back.is_empty() {
        return Err("This chat's trash is empty.".into());
    }
    log(
        store,
        session,
        "range.restored",
        format!("Restored {}", label(&back)),
        json!({"numbers": back}),
    )?;
    Ok(back)
}

/// The chat's removed turns, oldest first.
pub fn trashed(store: &Store, session: SessionId) -> Result<Vec<PastTurn>, String> {
    let events = store.replay(session, 0).map_err(store_err)?;
    Ok(past_turns(&events)
        .into_iter()
        .filter(|t| t.removed)
        .collect())
}

/// The move waiting on this chat, if any.
pub fn pending(store: &Store, session: SessionId) -> Result<Option<PendingMove>, String> {
    let events = store.replay(session, 0).map_err(store_err)?;
    Ok(pending_in(session, &events))
}

fn pending_in(session: SessionId, events: &[EventEnvelope]) -> Option<PendingMove> {
    let mut out = None;
    for e in events {
        match e.kind.as_str() {
            "range.move_pending" => {
                let to = e.payload["to"].as_str().and_then(|s| s.parse().ok());
                let numbers = e.payload["numbers"]
                    .as_array()
                    .map(|a| {
                        a.iter()
                            .filter_map(|n| n.as_u64().map(|n| n as u32))
                            .collect()
                    })
                    .unwrap_or_default();
                out = to.map(|to| PendingMove {
                    from: session,
                    to,
                    numbers,
                });
            }
            "range.move_confirmed" | "range.move_undone" => out = None,
            _ => {}
        }
    }
    out
}

/// Each turn's events, by turn number: from `turn.started` through its
/// end and the claim checks logged after it.
pub(crate) fn turn_events(events: &[EventEnvelope]) -> Vec<Vec<&EventEnvelope>> {
    let mut out: Vec<Vec<&EventEnvelope>> = Vec::new();
    let mut open = false;
    for e in events {
        if e.kind == "turn.started" {
            out.push(vec![e]);
            open = true;
            continue;
        }
        let Some(cur) = out.last_mut() else { continue };
        if open {
            cur.push(e);
            if matches!(
                e.kind.as_str(),
                "turn.finished" | "run.cancelled" | "budget.exceeded"
            ) {
                open = false;
            }
        } else if e.kind.starts_with("claims.") {
            cur.push(e);
        }
    }
    out
}

/// What a moved range carries: the live turns before it, briefly.
pub fn context_before(title: &str, turns: &[PastTurn], first: u32) -> String {
    let before: Vec<&PastTurn> = turns
        .iter()
        .filter(|t| t.number < first && !t.removed)
        .collect();
    let mut md = format!("Carried over from “{title}”");
    if before.is_empty() {
        md.push_str(". Nothing came before these messages.\n");
        return md;
    }
    md.push_str(&format!(" (the conversation before Q{first}):\n\n"));
    let skip = before.len().saturating_sub(8);
    if skip > 0 {
        md.push_str(&format!("- ({skip} earlier turns not shown)\n"));
    }
    for t in &before[skip..] {
        let q = t.message.as_deref().unwrap_or("(message not kept)");
        let a = t
            .answer
            .as_deref()
            .map(|a| clip(a, 400))
            .unwrap_or_else(|| t.stop.clone().unwrap_or_default());
        md.push_str(&format!("- **Q{}** {}\n  {}\n", t.number, clip(q, 200), a));
    }
    md
}

pub(crate) fn clip(text: &str, max: usize) -> String {
    let line = text.split_whitespace().collect::<Vec<_>>().join(" ");
    if line.chars().count() <= max {
        return line;
    }
    let cut: String = line.chars().take(max).collect();
    format!("{}…", cut.trim_end())
}

/// Copies `numbers` with the context before them into a new session or a
/// sub-session. The original is untouched until [`confirm`].
pub fn move_to(
    store: &mut Store,
    session: SessionId,
    numbers: &[u32],
    target: MoveTarget,
) -> Result<PendingMove, String> {
    let source = store
        .session(session)
        .map_err(store_err)?
        .ok_or("That chat is no longer in the database.")?;
    let events = store.replay(session, 0).map_err(store_err)?;
    if let Some(p) = pending_in(session, &events) {
        return Err(format!(
            "A move of {} is waiting. Confirm it or undo it first.",
            label(&p.numbers)
        ));
    }
    live_selection(&events, numbers)?;
    let mut nums = numbers.to_vec();
    nums.sort_unstable();
    nums.dedup();
    let turns = past_turns(&events);
    let first = nums[0];
    let to = match &target {
        MoveTarget::NewSession { title } => {
            let project = store.ensure_project(INBOX).map_err(store_err)?;
            let title = title
                .clone()
                .unwrap_or_else(|| format!("{} from {}", label(&nums), source.title));
            store
                .create_session(project, source.harness, &title)
                .map_err(store_err)?
        }
        MoveTarget::SubSession { anchor, title } => {
            if source.parent.is_some() {
                return Err(
                    "Messages of a sub-session move to a new session, not another sub-session."
                        .into(),
                );
            }
            let anchor = anchor.unwrap_or_else(|| {
                turns
                    .iter()
                    .filter(|t| t.number < first && !t.removed)
                    .map(|t| t.number)
                    .next_back()
                    .unwrap_or(first)
            });
            let title = title
                .clone()
                .unwrap_or_else(|| format!("{} moved from Q{anchor}", label(&nums)));
            store
                .create_sub_session(session, anchor, &title, None)
                .map_err(store_err)?
        }
    };
    log(
        store,
        to,
        "session.context",
        format!("Context carried over from {}", source.title),
        json!({
            "markdown": context_before(&source.title, &turns, first),
            "from": session.to_string(),
            "numbers": nums,
        }),
    )?;
    let spans = turn_events(&events);
    let selected: Vec<&EventEnvelope> = nums
        .iter()
        .filter_map(|n| spans.get(*n as usize - 1))
        .flatten()
        .copied()
        .collect();
    // Copies keep their spacing but start now, so sub-session IDs (ordered
    // by start time) stay stable for messages already there.
    let shift = selected
        .first()
        .map(|e| time::OffsetDateTime::now_utc() - e.ts)
        .unwrap_or_default();
    for e in selected {
        let mut copy = e.clone();
        copy.session_id = to;
        copy.event_id = EventId::new();
        copy.ts += shift;
        store.append(&copy).map_err(store_err)?;
    }
    log(
        store,
        session,
        "range.move_pending",
        format!(
            "Copied {} to another session; the originals stay until you confirm",
            label(&nums)
        ),
        json!({"numbers": nums, "to": to.to_string()}),
    )?;
    Ok(PendingMove {
        from: session,
        to,
        numbers: nums,
    })
}

/// Confirms the waiting move: the originals go to the chat's trash
/// (restorable) and the copy stays.
pub fn confirm(store: &mut Store, session: SessionId) -> Result<PendingMove, String> {
    let p = pending(store, session)?.ok_or("No move is waiting in this chat.")?;
    log(
        store,
        session,
        "range.removed",
        format!("Moved {} to another session", label(&p.numbers)),
        json!({"numbers": p.numbers, "reason": "moved", "to": p.to.to_string()}),
    )?;
    log(
        store,
        session,
        "range.move_confirmed",
        format!("Confirmed the move of {}", label(&p.numbers)),
        json!({"to": p.to.to_string()}),
    )?;
    Ok(p)
}

/// Undoes the waiting move: the copy is archived and the chat is as it was.
pub fn undo(store: &mut Store, session: SessionId) -> Result<PendingMove, String> {
    let p = pending(store, session)?.ok_or("No move is waiting in this chat.")?;
    store.archive_session(p.to).map_err(store_err)?;
    log(
        store,
        session,
        "range.move_undone",
        format!("Undid the move of {}", label(&p.numbers)),
        json!({"to": p.to.to_string()}),
    )?;
    Ok(p)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::transcript::live_turns;
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

    fn chat(n: usize) -> (Store, SessionId) {
        let mut store = Store::open_in_memory().unwrap();
        let p = store.ensure_project(INBOX).unwrap();
        let s = store.create_session(p, Harness::Research, "RAG").unwrap();
        for i in 1..=n {
            turn(&mut store, s, &format!("q{i}"), &format!("a{i}"));
        }
        (store, s)
    }

    fn live(store: &Store, s: SessionId) -> Vec<u32> {
        live_turns(&store.replay(s, 0).unwrap())
            .iter()
            .map(|t| t.number)
            .collect()
    }

    #[test]
    fn remove_and_restore_a_range() {
        let (mut store, s) = chat(5);
        assert_eq!(remove(&mut store, s, &[3, 2, 4]).unwrap(), [2, 3, 4]);
        assert_eq!(live(&store, s), [1, 5], "Q5 keeps its number");
        assert_eq!(trashed(&store, s).unwrap().len(), 3);
        assert!(remove(&mut store, s, &[3]).is_err(), "already in the trash");
        assert!(remove(&mut store, s, &[9]).is_err());
        assert_eq!(restore(&mut store, s, Some(&[3])).unwrap(), [3]);
        assert_eq!(live(&store, s), [1, 3, 5]);
        assert!(restore(&mut store, s, Some(&[1])).is_err());
        assert_eq!(restore(&mut store, s, None).unwrap(), [2, 4]);
        assert_eq!(live(&store, s), [1, 2, 3, 4, 5]);
        assert!(restore(&mut store, s, None).is_err());
        assert_eq!(label(&[3, 4, 5]), "Q3–Q5");
        assert_eq!(label(&[3, 7]), "Q3, Q7");
    }

    #[test]
    fn a_move_keeps_the_original_until_confirmed_and_carries_context() {
        let (mut store, s) = chat(5);
        let before = store.replay(s, 0).unwrap().len();
        let p = move_to(
            &mut store,
            s,
            &[3, 4],
            MoveTarget::NewSession { title: None },
        )
        .unwrap();
        assert_eq!(p.numbers, [3, 4]);
        // The original is untouched (one note in the log, no turn hidden).
        assert_eq!(live(&store, s), [1, 2, 3, 4, 5]);
        assert_eq!(store.replay(s, 0).unwrap().len(), before + 1);
        let copy = store.replay(p.to, 0).unwrap();
        let moved = live_turns(&copy);
        assert_eq!(
            moved
                .iter()
                .map(|t| t.answer.clone().unwrap())
                .collect::<Vec<_>>(),
            ["a3", "a4"]
        );
        let ctx = moved[0].context.clone().unwrap();
        assert!(ctx.contains("q1") && ctx.contains("a2") && !ctx.contains("a3"));
        assert_eq!(
            store.session(p.to).unwrap().unwrap().title,
            "Q3–Q4 from RAG"
        );
        assert!(
            move_to(&mut store, s, &[1], MoveTarget::NewSession { title: None }).is_err(),
            "one move at a time"
        );

        confirm(&mut store, s).unwrap();
        assert_eq!(live(&store, s), [1, 2, 5]);
        assert!(confirm(&mut store, s).is_err());
        // Moved messages are restorable from the trash.
        restore(&mut store, s, Some(&[3, 4])).unwrap();
        assert_eq!(live(&store, s), [1, 2, 3, 4, 5]);
    }

    #[test]
    fn undo_archives_the_copy_and_sub_session_moves_hang_under_the_chat() {
        let (mut store, s) = chat(4);
        let p = move_to(
            &mut store,
            s,
            &[3, 4],
            MoveTarget::SubSession {
                anchor: None,
                title: None,
            },
        )
        .unwrap();
        let sub = store.session(p.to).unwrap().unwrap();
        assert_eq!((sub.parent, sub.anchor), (Some(s), Some(2)));
        let ids = crate::refs::refs_of(&store, p.to).unwrap();
        assert_eq!(
            ids.iter().map(|r| r.to_string()).collect::<Vec<_>>(),
            ["Q2.1", "Q2.2"]
        );
        undo(&mut store, s).unwrap();
        assert!(store.is_archived(p.to).unwrap());
        assert_eq!(live(&store, s), [1, 2, 3, 4]);
        assert!(pending(&store, s).unwrap().is_none());
    }
}
