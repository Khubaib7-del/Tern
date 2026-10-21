//! Stable identifiers.
//!
//! Internal IDs are ULIDs with a short type prefix (`ses_`, `trn_`, `evt_`,
//! `prj_`) so a log line always says what it refers to. Message references
//! (`Q12`, `Q12.3`) are the human-facing IDs shown in the transcript.

use std::fmt;
use std::str::FromStr;

use serde::{Deserialize, Deserializer, Serialize, Serializer};
use ulid::Ulid;

use crate::{CoreError, CoreResult};

macro_rules! prefixed_id {
    ($(#[$doc:meta])* $name:ident, $prefix:literal, $kind:literal) => {
        $(#[$doc])*
        #[derive(Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
        pub struct $name(Ulid);

        impl $name {
            pub const PREFIX: &'static str = $prefix;

            /// A new, time-ordered ID.
            pub fn new() -> Self {
                Self(Ulid::new())
            }

            pub fn as_ulid(&self) -> Ulid {
                self.0
            }
        }

        impl Default for $name {
            fn default() -> Self {
                Self::new()
            }
        }

        impl fmt::Display for $name {
            fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                write!(f, "{}{}", $prefix, self.0)
            }
        }

        impl fmt::Debug for $name {
            fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                fmt::Display::fmt(self, f)
            }
        }

        impl FromStr for $name {
            type Err = CoreError;
            fn from_str(s: &str) -> CoreResult<Self> {
                let err = || CoreError::InvalidId { kind: $kind, value: s.to_owned() };
                let raw = s.strip_prefix($prefix).ok_or_else(err)?;
                Ulid::from_string(raw).map(Self).map_err(|_| err())
            }
        }

        impl Serialize for $name {
            fn serialize<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
                s.collect_str(self)
            }
        }

        impl<'de> Deserialize<'de> for $name {
            fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
                let s = String::deserialize(d)?;
                s.parse().map_err(serde::de::Error::custom)
            }
        }
    };
}

prefixed_id!(
    /// A project: one vault, its library, and its sessions.
    ProjectId, "prj_", "project"
);
prefixed_id!(
    /// A session (one chat thread, Research or Study).
    SessionId, "ses_", "session"
);
prefixed_id!(
    /// One turn: a user message and everything the agent did for it.
    TurnId, "trn_", "turn"
);
prefixed_id!(
    /// One event in the append-only log.
    EventId, "evt_", "event"
);

/// The human-facing ID of a message in a session: `Q12` for a top-level
/// message, `Q12.3` for the third message of a sub-session branched from it.
#[derive(Clone, PartialEq, Eq, Hash, PartialOrd, Ord, Debug)]
pub struct MessageRef {
    pub number: u32,
    pub sub: Option<u32>,
}

impl MessageRef {
    pub fn top(number: u32) -> Self {
        Self { number, sub: None }
    }

    pub fn sub(number: u32, sub: u32) -> Self {
        Self {
            number,
            sub: Some(sub),
        }
    }

    /// The top-level message this reference belongs to.
    pub fn root(&self) -> Self {
        Self::top(self.number)
    }
}

impl fmt::Display for MessageRef {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self.sub {
            Some(sub) => write!(f, "Q{}.{}", self.number, sub),
            None => write!(f, "Q{}", self.number),
        }
    }
}

impl FromStr for MessageRef {
    type Err = CoreError;
    fn from_str(s: &str) -> CoreResult<Self> {
        let err = || CoreError::InvalidMessageRef(s.to_owned());
        let body = s
            .strip_prefix('Q')
            .or_else(|| s.strip_prefix('q'))
            .ok_or_else(err)?;
        let parse = |p: &str| -> CoreResult<u32> {
            if p.is_empty() || !p.bytes().all(|b| b.is_ascii_digit()) || p.starts_with('0') {
                return Err(err());
            }
            p.parse().map_err(|_| err())
        };
        match body.split_once('.') {
            Some((n, m)) => Ok(Self::sub(parse(n)?, parse(m)?)),
            None => Ok(Self::top(parse(body)?)),
        }
    }
}

impl Serialize for MessageRef {
    fn serialize<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        s.collect_str(self)
    }
}

impl<'de> Deserialize<'de> for MessageRef {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        let s = String::deserialize(d)?;
        s.parse().map_err(serde::de::Error::custom)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn prefixed_ids_round_trip() {
        let id = SessionId::new();
        let text = id.to_string();
        assert!(text.starts_with("ses_"));
        assert_eq!(text.parse::<SessionId>().unwrap(), id);
        let json = serde_json::to_string(&id).unwrap();
        assert_eq!(serde_json::from_str::<SessionId>(&json).unwrap(), id);
    }

    #[test]
    fn ids_reject_wrong_prefix() {
        let turn = TurnId::new().to_string();
        assert!(turn.parse::<SessionId>().is_err());
        assert!("ses_not-a-ulid".parse::<SessionId>().is_err());
    }

    #[test]
    fn ids_are_time_ordered() {
        let a = EventId::new();
        std::thread::sleep(std::time::Duration::from_millis(2));
        let b = EventId::new();
        assert!(a < b);
    }

    #[test]
    fn message_refs_parse_and_print() {
        assert_eq!("Q12".parse::<MessageRef>().unwrap(), MessageRef::top(12));
        assert_eq!(
            "Q12.3".parse::<MessageRef>().unwrap(),
            MessageRef::sub(12, 3)
        );
        assert_eq!(MessageRef::sub(12, 3).to_string(), "Q12.3");
        assert_eq!(MessageRef::sub(12, 3).root(), MessageRef::top(12));
        for bad in [
            "", "Q", "12", "Q0", "Q01", "Q1.", "Q1.0", "Q1.2.3", "Q-1", "Qx",
        ] {
            assert!(
                bad.parse::<MessageRef>().is_err(),
                "{bad} should be rejected"
            );
        }
    }

    #[test]
    fn message_refs_sort_by_thread() {
        let mut v: Vec<MessageRef> = ["Q2", "Q1.2", "Q1", "Q10", "Q1.10"]
            .iter()
            .map(|s| s.parse().unwrap())
            .collect();
        v.sort();
        let out: Vec<String> = v.iter().map(ToString::to_string).collect();
        assert_eq!(out, ["Q1", "Q1.2", "Q1.10", "Q2", "Q10"]);
    }
}
