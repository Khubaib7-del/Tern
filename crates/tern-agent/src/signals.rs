//! Signal tap and incident store (#85, A16).
//!
//! Every event a chat records passes the tap. Provider trouble (retries,
//! model switches, failed calls), tool failures and refused arguments,
//! verifier findings, loops (a repeated or refused reply, a write or read
//! asked for again) and the user correcting Tern each become a signal with
//! a deterministic signature, such as `tool:read_url:http #` or
//! `loop:model.repeated`. The store counts signatures across chats, so a
//! fault that keeps coming back is visible as one incident with a count.
//!
//! No model is asked and nothing here is a user setting. The store feeds
//! the memory inbox and the later learning core; what it may teach the
//! harness goes through [`learned_context`], which keeps only entries
//! made for this [`HARNESS_VERSION`] and is switched off entirely by
//! `TERN_NO_LEARNED=1`. At this version no entries are learned yet.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::{LazyLock, Mutex};

use regex::Regex;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use tern_core::EventEnvelope;

/// The harness version learned entries are made for: an entry from
/// another version is never used (a snapshot is kept per version).
pub const HARNESS_VERSION: &str = "85a.1";

/// Chats kept per incident, newest last.
const SESSIONS_KEPT: usize = 5;

/// Where a signal came from.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Source {
    /// The provider: a retry, a model switch, a failed call.
    Provider,
    /// A tool failed or its arguments were refused.
    Tool,
    /// The claim checker found a claim the sources counter.
    Verifier,
    /// The model went round: a repeated, empty or refused reply, a write
    /// or read asked for again.
    Loop,
    /// The user corrected Tern.
    Correction,
}

/// One signal from one event.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Signal {
    pub source: Source,
    /// Deterministic: the same fault gives the same signature in any chat.
    pub signature: String,
    pub session: String,
    /// Unix seconds of the event.
    pub at: i64,
}

/// A signature seen, with how often and where.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Incident {
    pub source: Source,
    pub signature: String,
    pub count: u32,
    /// Unix seconds, first and last time seen.
    pub first: i64,
    pub last: i64,
    /// The last few chats it happened in.
    pub sessions: Vec<String>,
    /// The harness version that last saw it.
    pub harness: String,
}

/// The signal an event gives, if any.
pub fn tap(e: &EventEnvelope) -> Option<Signal> {
    let p = &e.payload;
    let s = |k: &str| p.get(k).and_then(Value::as_str).unwrap_or_default();
    let (source, signature) = match e.kind.as_str() {
        "provider.retry" => (
            Source::Provider,
            format!("provider:{}:{}", s("provider"), norm(s("reason"))),
        ),
        "model.switched" => (
            Source::Provider,
            format!("provider:switched:{}", norm(s("reason"))),
        ),
        "model.failed" => (
            Source::Provider,
            format!("provider:failed:{}", norm(first_of(p, &["class", "error"]))),
        ),
        "turn.finished" if matches!(s("state"), "failed" | "Failed") => (
            Source::Provider,
            format!(
                "turn:failed:{}",
                norm(first_of(p, &["error_class", "class", "reason"]))
            ),
        ),
        "turn.receipt" => {
            let class = s("error");
            if class.is_empty() {
                return None;
            }
            (Source::Provider, format!("turn:stopped:{class}"))
        }
        "tool.failed" => (
            Source::Tool,
            format!("tool:{}:{}", s("tool"), norm(s("error"))),
        ),
        "tool.rejected" => (
            Source::Tool,
            format!("tool_args:{}:{}", s("tool"), norm(s("error"))),
        ),
        "claims.countered" | "model.false_claim" => {
            (Source::Verifier, format!("verifier:{}", e.kind))
        }
        "model.repeated"
        | "model.refused"
        | "model.empty_reply"
        | "model.asked_in_prose"
        | "model.printed_script"
        | "model.file_refusal" => (Source::Loop, format!("loop:{}", e.kind)),
        "tool.skipped" => {
            let reason = s("reason");
            if reason.is_empty() || reason == "next_batch" {
                return None;
            }
            (Source::Loop, format!("loop:{}:{reason}", s("tool")))
        }
        "turn.started" => {
            let kind = correction(s("message"))?;
            (Source::Correction, format!("correction:{kind}"))
        }
        _ => return None,
    };
    Some(Signal {
        source,
        signature,
        session: e.session_id.to_string(),
        at: e.ts.unix_timestamp(),
    })
}

fn first_of<'a>(p: &'a Value, keys: &[&str]) -> &'a str {
    keys.iter()
        .find_map(|k| p.get(*k).and_then(Value::as_str).filter(|v| !v.is_empty()))
        .unwrap_or("unknown")
}

/// An error reduced to its shape: lower case, digits as `#`, addresses
/// and quoted text out, at most 60 characters.
pub fn norm(text: &str) -> String {
    static URL: LazyLock<Regex> =
        LazyLock::new(|| Regex::new(r"https?://\S+").expect("static regex"));
    static QUOTED: LazyLock<Regex> =
        LazyLock::new(|| Regex::new(r#"`[^`]*`|"[^"]*""#).expect("static regex"));
    static DIGITS: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"\d+").expect("static regex"));
    let t = text.to_lowercase();
    let t = URL.replace_all(&t, "<url>");
    let t = QUOTED.replace_all(&t, "<q>");
    let t = DIGITS.replace_all(&t, "#");
    let t: String = t.split_whitespace().collect::<Vec<_>>().join(" ");
    let t: String = t.chars().take(60).collect();
    if t.is_empty() { "unknown".into() } else { t }
}

/// The kind of correction a message opens with, if it is one: `wrong`
/// ("no, that's wrong"), `asked` ("I said …", "not what I asked"),
/// `missed` ("you forgot …").
pub fn correction(message: &str) -> Option<&'static str> {
    static WRONG: LazyLock<Regex> = LazyLock::new(|| {
        Regex::new(r"^(no\b|nope\b|wrong\b|that'?s (not right|wrong|incorrect)|that is (not right|wrong|incorrect)|incorrect\b)")
            .expect("static regex")
    });
    static ASKED: LazyLock<Regex> = LazyLock::new(|| {
        Regex::new(r"^(i said\b|i asked\b|not what i (asked|meant|wanted)|that'?s not what i)")
            .expect("static regex")
    });
    static MISSED: LazyLock<Regex> = LazyLock::new(|| {
        Regex::new(r"^you (forgot|missed|ignored|didn'?t|did not)\b").expect("static regex")
    });
    let (own, _) = crate::attach::split(message);
    let (_, own) = crate::reply::split(own);
    let t = own.trim().to_lowercase();
    if ASKED.is_match(&t) {
        Some("asked")
    } else if MISSED.is_match(&t) {
        Some("missed")
    } else if WRONG.is_match(&t) {
        Some("wrong")
    } else {
        None
    }
}

/// Signatures counted across chats, kept in `signals/incidents.json`
/// under the data folder (or in memory).
#[derive(Debug, Default)]
pub struct IncidentStore {
    path: Option<PathBuf>,
    map: Mutex<BTreeMap<String, Incident>>,
}

impl IncidentStore {
    /// A store that keeps nothing on disk.
    pub fn memory() -> Self {
        Self::default()
    }

    /// The store in the app's data folder, with what it already holds.
    pub fn in_dir(data_dir: &Path) -> Self {
        let path = data_dir.join("signals").join("incidents.json");
        let map = std::fs::read_to_string(&path)
            .ok()
            .and_then(|t| serde_json::from_str::<Vec<Incident>>(&t).ok())
            .unwrap_or_default()
            .into_iter()
            .map(|i| (i.signature.clone(), i))
            .collect();
        Self {
            path: Some(path),
            map: Mutex::new(map),
        }
    }

    /// Taps `event` and counts its signal. Returns the incident's count,
    /// or `None` when the event gives no signal.
    pub fn observe(&self, event: &EventEnvelope) -> Option<u32> {
        tap(event).map(|s| self.record(s))
    }

    /// Counts `signal`; returns how often its signature has now been seen.
    pub fn record(&self, signal: Signal) -> u32 {
        let mut map = self.map.lock().unwrap_or_else(|e| e.into_inner());
        let i = map
            .entry(signal.signature.clone())
            .or_insert_with(|| Incident {
                source: signal.source,
                signature: signal.signature.clone(),
                count: 0,
                first: signal.at,
                last: signal.at,
                sessions: Vec::new(),
                harness: HARNESS_VERSION.to_owned(),
            });
        i.count += 1;
        i.last = i.last.max(signal.at);
        i.harness = HARNESS_VERSION.to_owned();
        i.sessions.retain(|s| *s != signal.session);
        i.sessions.push(signal.session);
        if i.sessions.len() > SESSIONS_KEPT {
            let extra = i.sessions.len() - SESSIONS_KEPT;
            i.sessions.drain(..extra);
        }
        let count = i.count;
        if let Some(path) = &self.path {
            let all: Vec<&Incident> = map.values().collect();
            if let Some(dir) = path.parent() {
                let _ = std::fs::create_dir_all(dir);
            }
            if let Ok(text) = serde_json::to_string_pretty(&all) {
                let _ = std::fs::write(path, text);
            }
        }
        count
    }

    /// Every incident, most frequent first.
    pub fn incidents(&self) -> Vec<Incident> {
        let map = self.map.lock().unwrap_or_else(|e| e.into_inner());
        let mut v: Vec<Incident> = map.values().cloned().collect();
        v.sort_by(|a, b| b.count.cmp(&a.count).then(a.signature.cmp(&b.signature)));
        v
    }

    /// Incidents seen at least `min` times (what the memory inbox offers).
    pub fn repeated(&self, min: u32) -> Vec<Incident> {
        self.incidents()
            .into_iter()
            .filter(|i| i.count >= min)
            .collect()
    }
}

/// One thing the harness learned from incidents, for the model's context.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct LearnedEntry {
    /// The incident it came from.
    pub signature: String,
    /// What the model is told.
    pub text: String,
    /// The harness version it was made for.
    pub harness: String,
}

/// Whether learned context is on: `TERN_NO_LEARNED=1` switches it off.
pub fn learned_enabled() -> bool {
    !std::env::var_os("TERN_NO_LEARNED").is_some_and(|v| v != "0" && !v.is_empty())
}

/// The `<learned_context>` block for `entries`: only entries made for
/// this harness version pass the boundary, each one line and at most 300
/// characters, nothing when `enabled` is false or nothing passes.
pub fn learned_context(entries: &[LearnedEntry], enabled: bool) -> String {
    if !enabled {
        return String::new();
    }
    let lines: Vec<String> = entries
        .iter()
        .filter(|e| e.harness == HARNESS_VERSION)
        .map(|e| {
            let one: String = e.text.split_whitespace().collect::<Vec<_>>().join(" ");
            format!("- {}", one.chars().take(300).collect::<String>())
        })
        .filter(|l| l.len() > 2)
        .collect();
    if lines.is_empty() {
        return String::new();
    }
    format!(
        "<learned_context>\n{}\n</learned_context>",
        lines.join("\n")
    )
}

/// Saves the learned entries for this harness version as
/// `signals/learned-<version>.json`, so a later version can be compared
/// with, or reverted to, this one.
pub fn snapshot(data_dir: &Path, entries: &[LearnedEntry]) -> std::io::Result<PathBuf> {
    let dir = data_dir.join("signals");
    std::fs::create_dir_all(&dir)?;
    let path = dir.join(format!("learned-{HARNESS_VERSION}.json"));
    let text = serde_json::to_string_pretty(entries).map_err(std::io::Error::other)?;
    std::fs::write(&path, text)?;
    Ok(path)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use tern_core::{EffectClass, SessionId};

    fn ev(kind: &str, payload: Value) -> EventEnvelope {
        EventEnvelope::new(SessionId::new(), kind, EffectClass::Analysis, "x", payload)
    }

    #[test]
    fn events_give_deterministic_signatures() {
        let a = tap(&ev(
            "tool.failed",
            json!({"tool": "read_url", "error": "HTTP 403 at https://a.org/x"}),
        ))
        .unwrap();
        let b = tap(&ev(
            "tool.failed",
            json!({"tool": "read_url", "error": "HTTP 404 at https://b.org/y"}),
        ))
        .unwrap();
        assert_eq!(a.source, Source::Tool);
        assert_eq!(a.signature, "tool:read_url:http # at <url>");
        assert_eq!(a.signature, b.signature);
        let r = tap(&ev(
            "provider.retry",
            json!({"provider": "mistral", "reason": "rate limited (429)"}),
        ))
        .unwrap();
        assert_eq!(r.signature, "provider:mistral:rate limited (#)");
        assert_eq!(
            tap(&ev("model.repeated", json!({}))).unwrap().signature,
            "loop:model.repeated"
        );
        assert_eq!(
            tap(&ev(
                "tool.skipped",
                json!({"tool": "paper_read", "reason": "already_read"})
            ))
            .unwrap()
            .signature,
            "loop:paper_read:already_read"
        );
        assert!(
            tap(&ev(
                "tool.skipped",
                json!({"tool": "paper_read", "reason": "next_batch"})
            ))
            .is_none()
        );
        assert!(tap(&ev("tool.executed", json!({"tool": "read_url"}))).is_none());
    }

    #[test]
    fn corrections() {
        assert_eq!(correction("No, that's wrong: it was 1889"), Some("wrong"));
        assert_eq!(correction("I said Harvard style, not APA"), Some("asked"));
        assert_eq!(correction("you forgot the second paper"), Some("missed"));
        assert_eq!(correction("Nobel prizes in 2020?"), None);
        let s = tap(&ev("turn.started", json!({"message": "not what I asked"}))).unwrap();
        assert_eq!(
            (s.source, s.signature.as_str()),
            (Source::Correction, "correction:asked")
        );
    }

    #[test]
    fn the_store_counts_across_chats_and_keeps_them_on_disk() {
        let dir = tempfile::tempdir().unwrap();
        let store = IncidentStore::in_dir(dir.path());
        for _ in 0..3 {
            store.observe(&ev(
                "tool.failed",
                json!({"tool": "read_url", "error": "HTTP 403"}),
            ));
        }
        store.observe(&ev("model.refused", json!({})));
        let again = IncidentStore::in_dir(dir.path());
        let all = again.incidents();
        assert_eq!(all[0].signature, "tool:read_url:http #");
        assert_eq!(all[0].count, 3);
        assert_eq!(all[0].sessions.len(), 3, "three chats");
        assert_eq!(again.repeated(2).len(), 1);
        assert_eq!(all[0].harness, HARNESS_VERSION);
    }

    #[test]
    fn learned_context_keeps_this_version_and_has_a_kill_switch() {
        let entries = vec![
            LearnedEntry {
                signature: "tool:read_url:http #".into(),
                text: "Pages on a.org refuse readers;\nuse another source.".into(),
                harness: HARNESS_VERSION.into(),
            },
            LearnedEntry {
                signature: "old".into(),
                text: "from another version".into(),
                harness: "84.0".into(),
            },
        ];
        let c = learned_context(&entries, true);
        assert!(c.contains("refuse readers; use another source"), "{c}");
        assert!(!c.contains("another version"));
        assert_eq!(learned_context(&entries, false), "");
        assert_eq!(learned_context(&[], true), "", "empty at first");
        let dir = tempfile::tempdir().unwrap();
        let p = snapshot(dir.path(), &entries[..1]).unwrap();
        assert!(p.ends_with(format!("learned-{HARNESS_VERSION}.json")));
    }
}
