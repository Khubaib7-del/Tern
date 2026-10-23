//! Message IDs (`docs/01-product-spec.md`): `Q12` is the twelfth turn of a
//! chat, `Q12.3` the third message of the sub-sessions branched from `Q12`.
//!
//! Sub-session messages are numbered across every sub-session hung on the
//! same parent message, in the order their turns started, so an ID never
//! changes once given and never names two messages. The same IDs work in
//! `@` mentions, the CLI, `tern://` links, and Library search.

use tern_core::{EventEnvelope, MessageRef, SessionId};
use tern_store::{SessionSummary, Store};
use time::OffsetDateTime;

use crate::attach::{Attachment, FileText, Role, Source};
use crate::transcript::{PastTurn, past_turns};

/// Where a message ID points.
#[derive(Clone, Debug, PartialEq)]
pub struct Located {
    /// The session holding the message.
    pub session: SessionId,
    /// Its turn number in that session (1-based, as in [`PastTurn::number`]).
    pub number: u32,
    pub reference: MessageRef,
    pub turn: PastTurn,
}

/// Reads one ID or a selection: `Q3`, `3`, `Q3-Q5`, `Q3..5`, `Q3,Q7,Q9`,
/// `Q12.1-Q12.3`. Ranges stay inside one parent (`Q12.1-Q12.3`, not
/// `Q11.2-Q12.1`).
pub fn parse_selection(text: &str) -> Result<Vec<MessageRef>, String> {
    let mut out: Vec<MessageRef> = Vec::new();
    for part in text.split([',', ' ']).filter(|p| !p.trim().is_empty()) {
        let part = part.trim();
        let (a, b) = match part.split_once("..").or_else(|| part.split_once('-')) {
            Some((a, b)) => (a, Some(b)),
            None => (part, None),
        };
        let first = one(a)?;
        let Some(b) = b else {
            push_unique(&mut out, first);
            continue;
        };
        // `Q12.1-3` reads as `Q12.1-Q12.3`; `Q3-5` as `Q3-Q5`.
        let last = match (first.sub, b.trim_start_matches(['Q', 'q']).contains('.')) {
            (Some(_), false) => {
                let n: u32 = b
                    .trim_start_matches(['Q', 'q'])
                    .parse()
                    .map_err(|_| bad(part))?;
                MessageRef::sub(first.number, n)
            }
            _ => one(b)?,
        };
        match (first.sub, last.sub) {
            (None, None) if last.number >= first.number => {
                for n in first.number..=last.number {
                    push_unique(&mut out, MessageRef::top(n));
                }
            }
            (Some(x), Some(y)) if first.number == last.number && y >= x => {
                for n in x..=y {
                    push_unique(&mut out, MessageRef::sub(first.number, n));
                }
            }
            _ => return Err(bad(part)),
        }
    }
    if out.is_empty() {
        return Err("Name the messages: Q3, Q3-Q5, or Q3,Q7.".into());
    }
    Ok(out)
}

fn one(s: &str) -> Result<MessageRef, String> {
    let s = s.trim();
    let with_q = if s.starts_with(['Q', 'q']) {
        s.to_owned()
    } else {
        format!("Q{s}")
    };
    with_q.parse().map_err(|_| bad(s))
}

fn bad(s: &str) -> String {
    format!("`{s}` is not a message ID. They look like Q3, Q12.3, or Q3-Q5.")
}

fn push_unique(out: &mut Vec<MessageRef>, r: MessageRef) {
    if !out.contains(&r) {
        out.push(r);
    }
}

/// A link to a message: `tern://chat/<session>/Q12.3`.
pub fn link(session: SessionId, r: &MessageRef) -> String {
    format!("tern://chat/{session}/{r}")
}

/// Reads a [`link`].
pub fn parse_link(text: &str) -> Option<(SessionId, MessageRef)> {
    let rest = text.trim().strip_prefix("tern://chat/")?;
    let (session, r) = rest.split_once('/')?;
    Some((session.parse().ok()?, r.trim_end_matches('/').parse().ok()?))
}

/// The top-level chat of `session`: its parent for a sub-session.
pub fn top_of(store: &Store, session: SessionId) -> Result<SessionSummary, String> {
    let s = store
        .session(session)
        .map_err(|e| e.to_string())?
        .ok_or("That chat is no longer in the database.")?;
    match s.parent {
        Some(p) => store
            .session(p)
            .map_err(|e| e.to_string())?
            .ok_or_else(|| "The chat this sub-session hangs under is gone.".to_owned()),
        None => Ok(s),
    }
}

/// When each turn of a log started, in order.
fn starts(events: &[EventEnvelope]) -> Vec<OffsetDateTime> {
    events
        .iter()
        .filter(|e| e.kind == "turn.started")
        .map(|e| e.ts)
        .collect()
}

/// Every turn of the sub-sessions of `parent` hung on message `anchor`,
/// in ID order: `(sub-session, turn number in it, start)`.
fn sub_turns(
    store: &Store,
    parent: SessionId,
    anchor: u32,
) -> Result<Vec<(SessionId, u32, OffsetDateTime)>, String> {
    let mut all = Vec::new();
    let subs = store.sub_sessions(parent).map_err(|e| e.to_string())?;
    for (order, s) in subs.iter().filter(|s| s.anchor == Some(anchor)).enumerate() {
        let events = store.replay(s.id, 0).map_err(|e| e.to_string())?;
        for (i, at) in starts(&events).into_iter().enumerate() {
            all.push((s.id, i as u32 + 1, at, order));
        }
    }
    all.sort_by_key(|(_, n, at, order)| (*at, *order, *n));
    Ok(all.into_iter().map(|(s, n, at, _)| (s, n, at)).collect())
}

/// The ID of each turn of `session`, aligned with [`past_turns`]: `Q3`
/// in a top-level chat, `Q12.4` in a sub-session.
pub fn refs_of(store: &Store, session: SessionId) -> Result<Vec<MessageRef>, String> {
    let s = store
        .session(session)
        .map_err(|e| e.to_string())?
        .ok_or("That chat is no longer in the database.")?;
    let events = store.replay(session, 0).map_err(|e| e.to_string())?;
    let count = starts(&events).len() as u32;
    match (s.parent, s.anchor) {
        (Some(parent), Some(anchor)) => {
            let all = sub_turns(store, parent, anchor)?;
            let mut out = Vec::new();
            for n in 1..=count {
                let pos = all
                    .iter()
                    .position(|(id, k, _)| *id == session && *k == n)
                    .map(|p| p as u32 + 1)
                    .unwrap_or(n);
                out.push(MessageRef::sub(anchor, pos));
            }
            Ok(out)
        }
        _ => Ok((1..=count).map(MessageRef::top).collect()),
    }
}

/// Finds the message `r` names, seen from `session` (a chat or one of its
/// sub-sessions).
pub fn resolve(store: &Store, session: SessionId, r: &MessageRef) -> Result<Located, String> {
    let top = top_of(store, session)?;
    let (target, number) = match r.sub {
        None => (top.id, r.number),
        Some(sub) => {
            let all = sub_turns(store, top.id, r.number)?;
            let (id, n, _) = all.get(sub as usize - 1).copied().ok_or_else(|| {
                if all.is_empty() {
                    format!("{} has no sub-sessions.", MessageRef::top(r.number))
                } else {
                    format!(
                        "There is no {r}; the sub-sessions of {} go up to {}.",
                        MessageRef::top(r.number),
                        MessageRef::sub(r.number, all.len() as u32)
                    )
                }
            })?;
            (id, n)
        }
    };
    let events = store.replay(target, 0).map_err(|e| e.to_string())?;
    let turns = past_turns(&events);
    let turn = turns
        .into_iter()
        .find(|t| t.number == number)
        .ok_or_else(|| format!("There is no {r} in this chat."))?;
    Ok(Located {
        session: target,
        number,
        reference: r.clone(),
        turn,
    })
}

/// The `@Q12` and `@Q12.3` mentions in a message's own text, resolved
/// into attachments named for the ID that quote the question and answer.
/// Unknown IDs are left alone.
pub fn mention_attachments(store: &Store, session: SessionId, text: &str) -> Vec<Attachment> {
    let (text, _) = crate::attach::split(text);
    let mut out = Vec::new();
    let mut seen: Vec<MessageRef> = Vec::new();
    for word in text.split_whitespace() {
        let Some(id) = word.strip_prefix('@') else {
            continue;
        };
        let id = id.trim_end_matches(|c: char| !c.is_ascii_alphanumeric());
        let Ok(r) = id.parse::<MessageRef>() else {
            continue;
        };
        if seen.contains(&r) {
            continue;
        }
        if let Ok(found) = resolve(store, session, &r) {
            let mut body = String::new();
            if let Some(m) = &found.turn.message {
                body.push_str(&format!("Question: {}\n", crate::attach::split(m).0));
            }
            if let Some(a) = &found.turn.answer {
                body.push_str(&format!("Answer: {a}\n"));
            }
            out.push(Attachment {
                name: r.to_string(),
                source: Source::Link,
                role: Role::Context,
                files: vec![FileText {
                    path: format!("{r}.md"),
                    text: body,
                    cut: false,
                }],
                skipped: Vec::new(),
                images: Vec::new(),
            });
            seen.push(r);
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::chat::INBOX;
    use serde_json::json;
    use tern_core::EffectClass;
    use tern_store::Harness;

    pub(crate) fn turn(store: &mut Store, s: SessionId, q: &str, a: &str) {
        store
            .append(&EventEnvelope::new(
                s,
                "turn.started",
                EffectClass::Analysis,
                "Started",
                json!({ "message": q }),
            ))
            .unwrap();
        store
            .append(&EventEnvelope::new(
                s,
                "turn.finished",
                EffectClass::Analysis,
                "Answered",
                json!({"state": "completed", "answer": a}),
            ))
            .unwrap();
    }

    #[test]
    fn selections_parse() {
        let ids = |s: &str| {
            parse_selection(s)
                .unwrap()
                .iter()
                .map(|r| r.to_string())
                .collect::<Vec<_>>()
        };
        assert_eq!(ids("Q3-Q5"), ["Q3", "Q4", "Q5"]);
        assert_eq!(ids("3..5"), ["Q3", "Q4", "Q5"]);
        assert_eq!(ids("Q3,Q7, q9"), ["Q3", "Q7", "Q9"]);
        assert_eq!(ids("Q12.1-Q12.3"), ["Q12.1", "Q12.2", "Q12.3"]);
        assert_eq!(ids("Q12.1-3"), ["Q12.1", "Q12.2", "Q12.3"]);
        assert!(parse_selection("Q5-Q3").is_err());
        assert!(parse_selection("Q11.2-Q12.1").is_err());
        assert!(parse_selection("x").is_err());
        assert!(parse_selection("").is_err());
        let s = SessionId::new();
        let r = MessageRef::sub(12, 3);
        assert_eq!(parse_link(&link(s, &r)), Some((s, r)));
        assert_eq!(parse_link("https://x"), None);
    }

    #[test]
    fn sub_ids_resolve_across_sub_sessions_in_start_order() {
        let mut store = Store::open_in_memory().unwrap();
        let p = store.ensure_project(INBOX).unwrap();
        let top = store.create_session(p, Harness::Research, "RAG").unwrap();
        for i in 1..=3 {
            turn(&mut store, top, &format!("q{i}"), &format!("a{i}"));
        }
        let a = store.create_sub_session(top, 2, "A", None).unwrap();
        let b = store.create_sub_session(top, 2, "B", None).unwrap();
        turn(&mut store, a, "a-1", "A1");
        std::thread::sleep(std::time::Duration::from_millis(5));
        turn(&mut store, b, "b-1", "B1");
        std::thread::sleep(std::time::Duration::from_millis(5));
        turn(&mut store, a, "a-2", "A2");

        let ids = |s| {
            refs_of(&store, s)
                .unwrap()
                .iter()
                .map(|r| r.to_string())
                .collect::<Vec<_>>()
        };
        assert_eq!(ids(top), ["Q1", "Q2", "Q3"]);
        assert_eq!(ids(a), ["Q2.1", "Q2.3"]);
        assert_eq!(ids(b), ["Q2.2"]);

        let found = resolve(&store, top, &"Q2.2".parse().unwrap()).unwrap();
        assert_eq!(found.session, b);
        assert_eq!(found.turn.answer.as_deref(), Some("B1"));
        // From inside a sub-session, top-level IDs reach the parent.
        let q3 = resolve(&store, a, &"Q3".parse().unwrap()).unwrap();
        assert_eq!((q3.session, q3.number), (top, 3));
        assert!(resolve(&store, top, &"Q2.4".parse().unwrap()).is_err());
        assert!(resolve(&store, top, &"Q1.1".parse().unwrap()).is_err());

        let quoted = mention_attachments(&store, top, "compare @Q2.3 with @Q1, and @Q9");
        assert_eq!(quoted.len(), 2);
        assert_eq!(quoted[0].name, "Q2.3");
        assert!(quoted[0].files[0].text.contains("A2"));
        assert!(quoted[1].files[0].text.contains("a1"));
        let sent = crate::attach::append("compare @Q2.3", &quoted);
        let (text, chips) = crate::attach::split(&sent);
        assert_eq!(text, "compare @Q2.3");
        assert_eq!(chips.len(), 2);
    }
}
