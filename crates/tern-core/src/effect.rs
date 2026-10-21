//! Effect classes: what a tool call or step can touch.
//!
//! The policy gate in `tern-agent` decides per class whether a step runs,
//! asks, or is refused. The order here is the order of risk.

use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EffectClass {
    /// Reads local project data.
    Read,
    /// Pure computation over data already in context.
    Analysis,
    /// Writes inside the project vault or app data.
    Write,
    /// Reaches the network (fetch, search, model call).
    Network,
    /// Touches stored credentials.
    Credentials,
    /// Acts outside Tern (posts, sends, pushes, purchases).
    ExternalSideEffect,
    /// Deletes or overwrites data that cannot be recovered.
    Destructive,
}

impl EffectClass {
    pub const ALL: [EffectClass; 7] = [
        Self::Read,
        Self::Analysis,
        Self::Write,
        Self::Network,
        Self::Credentials,
        Self::ExternalSideEffect,
        Self::Destructive,
    ];

    /// Whether the default policy asks the user before a step of this class runs.
    /// Writes inside the vault are shown as diffs and need no prompt; everything
    /// that leaves the machine's data or cannot be undone asks.
    pub fn asks_by_default(self) -> bool {
        matches!(
            self,
            Self::Credentials | Self::ExternalSideEffect | Self::Destructive
        )
    }

    pub fn label(self) -> &'static str {
        match self {
            Self::Read => "read",
            Self::Analysis => "analysis",
            Self::Write => "write",
            Self::Network => "network",
            Self::Credentials => "credentials",
            Self::ExternalSideEffect => "external side effect",
            Self::Destructive => "destructive",
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn risk_order_is_stable() {
        let mut sorted = EffectClass::ALL;
        sorted.sort();
        assert_eq!(sorted, EffectClass::ALL);
        assert!(EffectClass::Read < EffectClass::Destructive);
    }

    #[test]
    fn only_risky_classes_ask_by_default() {
        let asking: Vec<_> = EffectClass::ALL
            .into_iter()
            .filter(|c| c.asks_by_default())
            .collect();
        assert_eq!(
            asking,
            [
                EffectClass::Credentials,
                EffectClass::ExternalSideEffect,
                EffectClass::Destructive
            ]
        );
    }

    #[test]
    fn serialises_as_snake_case() {
        let s = serde_json::to_string(&EffectClass::ExternalSideEffect).unwrap();
        assert_eq!(s, "\"external_side_effect\"");
    }
}
