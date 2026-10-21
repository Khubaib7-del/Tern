//! The versioned event envelope (`docs/03-architecture.md`, section 4.1).

use serde::{Deserialize, Serialize};
use time::OffsetDateTime;

use crate::{EffectClass, EventId, Redactor, SessionId, TurnId};

/// Current envelope schema version.
pub const EVENT_VERSION: u16 = 1;

/// Whether a usage figure was reported by the provider or estimated by Tern.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Measure {
    Measured,
    Estimated,
}

/// Tokens and cost added by one event.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct UsageDelta {
    pub input_tokens: u64,
    pub output_tokens: u64,
    /// Cost in micro-units of `currency` (1 USD = 1_000_000).
    pub cost_micros: u64,
    pub currency: String,
    pub measure: Measure,
}

/// One state change. The log is append-only and strictly ordered per session.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct EventEnvelope {
    pub version: u16,
    pub event_id: EventId,
    pub session_id: SessionId,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub turn_id: Option<TurnId>,
    /// Monotonic per session, starting at 1. Assigned by the store on append.
    pub seq: u64,
    #[serde(with = "time::serde::rfc3339")]
    pub ts: OffsetDateTime,
    /// Dotted event type, for example `plan.created` or `claim.verified`.
    #[serde(rename = "type")]
    pub kind: String,
    /// One plain-language line for the Summary view.
    pub user_summary: String,
    pub effect_class: EffectClass,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub evidence_refs: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub usage_delta: Option<UsageDelta>,
    #[serde(default)]
    pub payload: serde_json::Value,
}

impl EventEnvelope {
    /// A new event for `session`. `seq` is 0 until the store assigns it.
    pub fn new(
        session_id: SessionId,
        kind: impl Into<String>,
        effect_class: EffectClass,
        user_summary: impl Into<String>,
        payload: serde_json::Value,
    ) -> Self {
        Self {
            version: EVENT_VERSION,
            event_id: EventId::new(),
            session_id,
            turn_id: None,
            seq: 0,
            ts: OffsetDateTime::now_utc(),
            kind: kind.into(),
            user_summary: user_summary.into(),
            effect_class,
            evidence_refs: Vec::new(),
            usage_delta: None,
            payload,
        }
    }

    pub fn with_turn(mut self, turn: TurnId) -> Self {
        self.turn_id = Some(turn);
        self
    }

    pub fn with_evidence(mut self, refs: impl IntoIterator<Item = impl Into<String>>) -> Self {
        self.evidence_refs = refs.into_iter().map(Into::into).collect();
        self
    }

    pub fn with_usage(mut self, usage: UsageDelta) -> Self {
        self.usage_delta = Some(usage);
        self
    }

    /// A copy with every free-text field passed through the redactor.
    /// The store calls this before anything is persisted.
    pub fn redacted(&self, redactor: &Redactor) -> Self {
        let mut e = self.clone();
        e.user_summary = redactor.redact(&e.user_summary).into_owned();
        e.evidence_refs = e
            .evidence_refs
            .iter()
            .map(|r| redactor.redact(r).into_owned())
            .collect();
        redactor.redact_json(&mut e.payload);
        e
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn envelope_round_trips_through_json() {
        let e = EventEnvelope::new(
            SessionId::new(),
            "source.fetched",
            EffectClass::Network,
            "Fetched the arXiv abstract",
            json!({"url": "https://arxiv.org/abs/1706.03762"}),
        )
        .with_turn(TurnId::new())
        .with_evidence(["S3"]);
        let text = serde_json::to_string(&e).unwrap();
        assert!(text.contains("\"type\":\"source.fetched\""));
        let back: EventEnvelope = serde_json::from_str(&text).unwrap();
        assert_eq!(back, e);
    }

    #[test]
    fn redacted_copy_hides_keys_everywhere() {
        let key = "sk-proj-abcdefghijklmnopqrstuvwxyz0123456789";
        let e = EventEnvelope::new(
            SessionId::new(),
            "provider.tested",
            EffectClass::Credentials,
            format!("Tested key {key}"),
            json!({"headers": {"authorization": format!("Bearer {key}")}, "list": [key]}),
        );
        let r = e.redacted(&Redactor::default());
        let text = serde_json::to_string(&r).unwrap();
        assert!(!text.contains(key), "key leaked: {text}");
        assert!(r.user_summary.contains("[redacted"));
    }
}
