//! Session handoff packets (`docs/04-roadmap.md`, Phase 9): everything a
//! fresh session needs to carry on, and nothing else. A packet holds the
//! goal, decisions, open questions, key sources and claims, a ledger
//! snapshot, and next steps, as Markdown and JSON. A new session started
//! from a packet reads the packet alone; it never opens the old log.

use serde::{Deserialize, Serialize};
use serde_json::json;
use tern_core::{EffectClass, EventEnvelope, SessionId};
use tern_store::{Harness, Store};

use crate::chat::INBOX;
use crate::range::clip;
use crate::transcript::{PastTurn, live_turns};

pub const PACKET_VERSION: u32 = 1;

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct PacketSource {
    pub title: String,
    pub url: String,
    /// Turns that read it.
    pub reads: usize,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct PacketClaim {
    pub text: String,
    /// Supported, Partly supported, Not supported, No source.
    pub verdict: String,
}

/// What the session used, at the moment of the handoff.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct LedgerSnapshot {
    pub turns: usize,
    pub model_calls: usize,
    pub searches: usize,
    pub pages_read: usize,
    pub tokens_in: u64,
    pub tokens_out: u64,
}

/// A handoff packet.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Packet {
    pub version: u32,
    pub title: String,
    pub goal: String,
    #[serde(default)]
    pub decisions: Vec<String>,
    #[serde(default)]
    pub open_questions: Vec<String>,
    #[serde(default)]
    pub sources: Vec<PacketSource>,
    #[serde(default)]
    pub claims: Vec<PacketClaim>,
    #[serde(default)]
    pub ledger: LedgerSnapshot,
    #[serde(default)]
    pub next_steps: Vec<String>,
    /// The session it came from, for the record only.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub from: Option<String>,
    /// Unix seconds.
    #[serde(default)]
    pub created_at: i64,
}

const DECISION_WORDS: [&str; 9] = [
    "decided",
    "we'll ",
    "we will",
    "let's ",
    "go with",
    "chose",
    "choose ",
    "settled on",
    "use ",
];

const NEXT_WORDS: [&str; 6] = ["next step", "next,", "then ", "todo", "to do:", "follow up"];

fn sentences(text: &str) -> Vec<String> {
    let mut out = Vec::new();
    for line in text.lines() {
        let line = line
            .trim()
            .trim_start_matches(['-', '*', '•', '#', '>'])
            .trim();
        if line.is_empty() {
            continue;
        }
        let mut cur = String::new();
        for c in line.chars() {
            cur.push(c);
            if matches!(c, '.' | '?' | '!') && cur.trim().len() > 1 {
                out.push(cur.trim().to_owned());
                cur.clear();
            }
        }
        if !cur.trim().is_empty() {
            out.push(cur.trim().to_owned());
        }
    }
    out
}

fn push_unique(list: &mut Vec<String>, s: String, max: usize) {
    if list.len() < max && !list.iter().any(|x| x.eq_ignore_ascii_case(&s)) {
        list.push(s);
    }
}

/// Drafts a packet from a chat's live turns: the goal from the first
/// question (or the title), decisions and next steps from what the user
/// and the answers said, open questions from stopped turns and questions
/// left in the last answer, sources by how often they were read, checked
/// claims with their verdicts, and the run ledgers summed.
pub fn draft(title: &str, turns: &[PastTurn], from: Option<SessionId>, at: i64) -> Packet {
    let live: Vec<&PastTurn> = turns.iter().filter(|t| !t.removed).collect();
    let message = |t: &PastTurn| {
        t.message.as_deref().map(|m| {
            crate::reply::split(crate::attach::split(m).0)
                .1
                .trim()
                .to_owned()
        })
    };
    let goal = live
        .iter()
        .find_map(|t| message(t))
        .filter(|g| !g.is_empty())
        .map(|g| clip(&g, 300))
        .unwrap_or_else(|| title.to_owned());
    let mut p = Packet {
        version: PACKET_VERSION,
        title: title.to_owned(),
        goal,
        from: from.map(|s| s.to_string()),
        created_at: at,
        ..Packet::default()
    };
    for t in &live {
        let user = message(t).unwrap_or_default();
        for s in sentences(&user) {
            let l = s.to_lowercase();
            if DECISION_WORDS.iter().any(|w| l.contains(w)) && !s.ends_with('?') {
                push_unique(&mut p.decisions, clip(&s, 240), 12);
            }
        }
        if let Some(a) = &t.answer {
            for s in sentences(a) {
                let l = s.to_lowercase();
                if l.contains("recommend") || l.starts_with("decision") {
                    push_unique(&mut p.decisions, clip(&s, 240), 12);
                }
                if NEXT_WORDS.iter().any(|w| l.contains(w)) {
                    push_unique(&mut p.next_steps, clip(&s, 240), 8);
                }
            }
        }
        if t.stop.is_some() && !user.is_empty() {
            push_unique(
                &mut p.open_questions,
                format!("{} (Q{} did not finish)", clip(&user, 200), t.number),
                10,
            );
        }
        for c in &t.run.checks {
            if p.claims.len() < 20 {
                p.claims.push(PacketClaim {
                    text: clip(&c.text, 240),
                    verdict: c.verdict.label().to_owned(),
                });
            }
        }
        for page in &t.pages {
            match p.sources.iter_mut().find(|s| s.url == page.url) {
                Some(s) => s.reads += 1,
                None => p.sources.push(PacketSource {
                    title: page.label().to_owned(),
                    url: page.url.clone(),
                    reads: 1,
                }),
            }
        }
        let sum = t.run.summary();
        p.ledger.model_calls += sum.model_calls;
        p.ledger.searches += sum.searches;
        p.ledger.pages_read += t.pages.len();
        p.ledger.tokens_in += sum.tokens_in;
        p.ledger.tokens_out += sum.tokens_out;
    }
    p.ledger.turns = live.len();
    p.sources.sort_by_key(|a| std::cmp::Reverse(a.reads));
    p.sources.truncate(12);
    if let Some(last) = live.iter().rev().find_map(|t| t.answer.as_deref()) {
        for s in sentences(last) {
            if s.ends_with('?') {
                push_unique(&mut p.open_questions, clip(&s, 240), 10);
            }
        }
    }
    if p.next_steps.is_empty() {
        p.next_steps = p
            .open_questions
            .iter()
            .take(3)
            .map(|q| format!("Answer: {q}"))
            .collect();
    }
    if p.next_steps.is_empty() {
        p.next_steps
            .push(format!("Carry on with the goal: {}", clip(&p.goal, 200)));
    }
    p
}

fn list(md: &mut String, heading: &str, items: &[String]) {
    md.push_str(&format!("\n## {heading}\n\n"));
    if items.is_empty() {
        md.push_str("- None yet.\n");
    }
    for i in items {
        md.push_str(&format!("- {i}\n"));
    }
}

/// The packet as Markdown, in a shape [`parse`] reads back.
pub fn to_markdown(p: &Packet) -> String {
    let mut md = format!("# Handoff: {}\n\n## Goal\n\n{}\n", p.title, p.goal);
    list(&mut md, "Decisions", &p.decisions);
    list(&mut md, "Open questions", &p.open_questions);
    let sources: Vec<String> = p
        .sources
        .iter()
        .map(|s| format!("[{}]({}) · read {}×", s.title, s.url, s.reads))
        .collect();
    list(&mut md, "Key sources", &sources);
    let claims: Vec<String> = p
        .claims
        .iter()
        .map(|c| format!("{} — {}", c.text, c.verdict))
        .collect();
    list(&mut md, "Key claims", &claims);
    let l = &p.ledger;
    md.push_str(&format!(
        "\n## Ledger\n\n- Turns: {}\n- Model calls: {}\n- Searches: {}\n- Pages read: {}\n- Tokens in: {}\n- Tokens out: {}\n",
        l.turns, l.model_calls, l.searches, l.pages_read, l.tokens_in, l.tokens_out
    ));
    list(&mut md, "Next steps", &p.next_steps);
    md
}

/// The packet as pretty JSON.
pub fn to_json(p: &Packet) -> String {
    serde_json::to_string_pretty(p).unwrap_or_default()
}

/// Reads a packet from JSON or from the Markdown [`to_markdown`] writes.
pub fn parse(text: &str) -> Result<Packet, String> {
    let t = text.trim_start_matches('\u{feff}').trim();
    if t.starts_with('{') {
        return serde_json::from_str(t)
            .map_err(|e| format!("That packet's JSON is not valid: {e}"));
    }
    let mut p = Packet {
        version: PACKET_VERSION,
        ..Packet::default()
    };
    let mut section = String::new();
    let mut goal = Vec::new();
    for line in t.lines() {
        let line = line.trim_end();
        if let Some(title) = line.strip_prefix("# Handoff:") {
            p.title = title.trim().to_owned();
            continue;
        }
        if let Some(h) = line.strip_prefix("## ") {
            section = h.trim().to_lowercase();
            continue;
        }
        let item = line.trim().strip_prefix("- ").map(str::trim);
        match (section.as_str(), item) {
            ("goal", _) if !line.trim().is_empty() => goal.push(line.trim().to_owned()),
            (_, Some("None yet.")) => {}
            ("decisions", Some(i)) => p.decisions.push(i.to_owned()),
            ("open questions", Some(i)) => p.open_questions.push(i.to_owned()),
            ("next steps", Some(i)) => p.next_steps.push(i.to_owned()),
            ("key claims", Some(i)) => {
                let (text, verdict) = i.rsplit_once(" — ").unwrap_or((i, ""));
                p.claims.push(PacketClaim {
                    text: text.to_owned(),
                    verdict: verdict.to_owned(),
                });
            }
            ("key sources", Some(i)) => {
                let title = i
                    .strip_prefix('[')
                    .and_then(|r| r.split_once("](").map(|x| x.0))
                    .unwrap_or(i);
                let url = i
                    .split_once("](")
                    .and_then(|x| x.1.split_once(')'))
                    .map(|x| x.0)
                    .unwrap_or_default();
                let reads = i
                    .rsplit_once("read ")
                    .and_then(|x| x.1.trim_end_matches('×').parse().ok())
                    .unwrap_or(1);
                p.sources.push(PacketSource {
                    title: title.to_owned(),
                    url: url.to_owned(),
                    reads,
                });
            }
            ("ledger", Some(i)) => {
                let (k, v) = i.split_once(':').unwrap_or((i, ""));
                let v: u64 = v.trim().parse().unwrap_or(0);
                let l = &mut p.ledger;
                match k.trim().to_lowercase().as_str() {
                    "turns" => l.turns = v as usize,
                    "model calls" => l.model_calls = v as usize,
                    "searches" => l.searches = v as usize,
                    "pages read" => l.pages_read = v as usize,
                    "tokens in" => l.tokens_in = v,
                    "tokens out" => l.tokens_out = v,
                    _ => {}
                }
            }
            _ => {}
        }
    }
    p.goal = goal.join(" ");
    if p.goal.is_empty() && p.title.is_empty() {
        return Err("That file is not a handoff packet (no goal or title).".into());
    }
    if p.title.is_empty() {
        p.title = clip(&p.goal, 60);
    }
    Ok(p)
}

/// Drafts the packet for a chat in `store`.
pub fn for_session(store: &Store, session: SessionId, at: i64) -> Result<Packet, String> {
    let s = store
        .session(session)
        .map_err(|e| e.to_string())?
        .ok_or("That chat is no longer in the database.")?;
    let events = store.replay(session, 0).map_err(|e| e.to_string())?;
    Ok(draft(&s.title, &live_turns(&events), Some(session), at))
}

/// Starts a new session whose only content is the packet. Returns its id.
pub fn start_from(store: &mut Store, packet: &Packet) -> Result<SessionId, String> {
    let project = store.ensure_project(INBOX).map_err(|e| e.to_string())?;
    let title = format!(
        "{} (continued)",
        packet.title.trim_end_matches(" (continued)")
    );
    let id = store
        .create_session(project, Harness::Research, &title)
        .map_err(|e| e.to_string())?;
    store
        .append(&EventEnvelope::new(
            id,
            "session.context",
            EffectClass::Analysis,
            format!("Started from a handoff packet: {}", packet.title),
            json!({
                "kind": "handoff",
                "markdown": to_markdown(packet),
                "packet": packet,
            }),
        ))
        .map_err(|e| e.to_string())?;
    Ok(id)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::transcript::{past_turns, trailing_context};

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
    fn a_packet_round_trips_and_a_new_session_holds_only_the_packet() {
        let mut old = Store::open_in_memory().unwrap();
        let p = old.ensure_project(INBOX).unwrap();
        let s = old
            .create_session(p, Harness::Research, "Vector DBs")
            .unwrap();
        turn(
            &mut old,
            s,
            "Which vector database should my app use?",
            "I recommend pgvector for a small project. Should it run on the lab server?",
        );
        turn(
            &mut old,
            s,
            "We decided to use pgvector. What indexes?",
            "HNSW. Next step: benchmark recall at 10.",
        );
        let packet = for_session(&old, s, 7).unwrap();
        assert_eq!(packet.goal, "Which vector database should my app use?");
        assert!(
            packet
                .decisions
                .iter()
                .any(|d| d.contains("decided to use pgvector"))
        );
        assert!(
            packet
                .decisions
                .iter()
                .any(|d| d.contains("recommend pgvector"))
        );
        assert!(
            packet
                .next_steps
                .iter()
                .any(|n| n.contains("benchmark recall"))
        );
        assert_eq!(packet.ledger.turns, 2);

        let md = to_markdown(&packet);
        let back = parse(&md).unwrap();
        assert_eq!(back.goal, packet.goal);
        assert_eq!(back.decisions, packet.decisions);
        assert_eq!(back.next_steps, packet.next_steps);
        assert_eq!(back.ledger, packet.ledger);
        assert_eq!(parse(&to_json(&packet)).unwrap(), packet);
        assert!(parse("hello").is_err());

        // A different database: the old chat does not exist there at all.
        let mut fresh = Store::open_in_memory().unwrap();
        let id = start_from(&mut fresh, &back).unwrap();
        let events = fresh.replay(id, 0).unwrap();
        assert_eq!(events.len(), 1, "the packet is the whole session");
        assert!(past_turns(&events).is_empty());
        let ctx = trailing_context(&events).unwrap();
        assert!(ctx.contains("pgvector") && ctx.contains("## Next steps"));
        assert_eq!(
            fresh.session(id).unwrap().unwrap().title,
            "Vector DBs (continued)"
        );
        assert!(fresh.session(s).unwrap().is_none());
    }
}
