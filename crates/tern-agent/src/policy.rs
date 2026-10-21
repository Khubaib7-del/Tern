//! The policy gate: decides whether one exact tool call runs, asks, or is refused.
//!
//! The model never holds permissions. It proposes a call; the gate looks at
//! the tool's effect class, its target (a domain, a path, a provider), and the
//! grants the user has given, and answers [`Decision::Allow`],
//! [`Decision::Ask`], or [`Decision::Deny`]. See `docs/05-security-model.md`.
//!
//! | Grant | Covers | Ends |
//! | --- | --- | --- |
//! | Once | These exact arguments, one time | When the call runs |
//! | Session | This tool on this target | At its expiry, or when the session ends |
//!
//! A new target always asks again, and a deny rule beats any grant.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};
use serde_json::Value;
use tern_core::EffectClass;
use time::{Duration, OffsetDateTime};

/// What the user's settings say about one effect class.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Mode {
    Allow,
    Ask,
    Deny,
}

/// The gate's answer for one call.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(tag = "decision", rename_all = "snake_case")]
pub enum Decision {
    Allow { because: String },
    Ask,
    Deny { reason: String },
}

/// One call the model proposed, as the gate sees it.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct CallView<'a> {
    pub tool: &'a str,
    pub effect: EffectClass,
    /// What the call touches: a domain, a vault path, a provider id.
    pub target: Option<&'a str>,
    pub args: &'a Value,
}

/// The user's reply to an approval request.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ApprovalAnswer {
    /// Run these exact arguments once.
    AllowOnce,
    /// Allow this tool on this target for the rest of the session, until it expires.
    AllowForSession,
    Deny,
}

#[derive(Clone, Debug, PartialEq)]
enum GrantScope {
    /// The canonical JSON of the arguments.
    Exact(String),
    Target(Option<String>),
}

#[derive(Clone, Debug, PartialEq)]
struct Grant {
    tool: String,
    scope: GrantScope,
    expires: Option<OffsetDateTime>,
}

/// Per-class modes. Defaults follow [`EffectClass::asks_by_default`].
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct PolicyConfig {
    pub modes: BTreeMap<EffectClass, Mode>,
    /// How long a session grant lasts.
    pub session_grant_minutes: i64,
}

impl Default for PolicyConfig {
    fn default() -> Self {
        let modes = EffectClass::ALL
            .into_iter()
            .map(|c| {
                (
                    c,
                    if c.asks_by_default() {
                        Mode::Ask
                    } else {
                        Mode::Allow
                    },
                )
            })
            .collect();
        Self {
            modes,
            session_grant_minutes: 30,
        }
    }
}

impl PolicyConfig {
    pub fn mode(&self, class: EffectClass) -> Mode {
        self.modes.get(&class).copied().unwrap_or(Mode::Ask)
    }

    pub fn with_mode(mut self, class: EffectClass, mode: Mode) -> Self {
        self.modes.insert(class, mode);
        self
    }
}

type Clock = fn() -> OffsetDateTime;

/// The gate for one session. Grants live as long as the gate.
#[derive(Clone, Debug)]
pub struct PolicyGate {
    config: PolicyConfig,
    grants: Vec<Grant>,
    clock: Clock,
}

impl Default for PolicyGate {
    fn default() -> Self {
        Self::new(PolicyConfig::default())
    }
}

impl PolicyGate {
    pub fn new(config: PolicyConfig) -> Self {
        Self {
            config,
            grants: Vec::new(),
            clock: OffsetDateTime::now_utc,
        }
    }

    /// A gate with a fixed clock, for tests of expiry.
    pub fn with_clock(mut self, clock: Clock) -> Self {
        self.clock = clock;
        self
    }

    pub fn config(&self) -> &PolicyConfig {
        &self.config
    }

    /// Decides one call. Pure apart from dropping expired grants.
    pub fn check(&mut self, call: &CallView<'_>) -> Decision {
        let now = (self.clock)();
        self.grants.retain(|g| g.expires.is_none_or(|t| t > now));
        match self.config.mode(call.effect) {
            Mode::Deny => Decision::Deny {
                reason: format!(
                    "Your settings refuse {} steps, so `{}` did not run.",
                    call.effect.label(),
                    call.tool
                ),
            },
            Mode::Allow => Decision::Allow {
                because: format!("{} steps are allowed", call.effect.label()),
            },
            Mode::Ask => {
                let exact = canonical(call.args);
                let target = call.target.map(str::to_owned);
                match self.grants.iter().find(|g| {
                    g.tool == call.tool
                        && match &g.scope {
                            GrantScope::Exact(a) => *a == exact,
                            GrantScope::Target(t) => *t == target,
                        }
                }) {
                    Some(g) => Decision::Allow {
                        because: match g.scope {
                            GrantScope::Exact(_) => "approved once".into(),
                            GrantScope::Target(_) => "approved for this session".into(),
                        },
                    },
                    None => Decision::Ask,
                }
            }
        }
    }

    /// Records the user's answer for `call`. Returns whether it may run.
    pub fn answer(&mut self, call: &CallView<'_>, answer: ApprovalAnswer) -> bool {
        match answer {
            ApprovalAnswer::Deny => false,
            ApprovalAnswer::AllowOnce => {
                self.grants.push(Grant {
                    tool: call.tool.to_owned(),
                    scope: GrantScope::Exact(canonical(call.args)),
                    expires: None,
                });
                true
            }
            ApprovalAnswer::AllowForSession => {
                self.grants.push(Grant {
                    tool: call.tool.to_owned(),
                    scope: GrantScope::Target(call.target.map(str::to_owned)),
                    expires: Some(
                        (self.clock)() + Duration::minutes(self.config.session_grant_minutes),
                    ),
                });
                true
            }
        }
    }

    /// Spends a one-time grant once its call has run.
    pub fn consume(&mut self, call: &CallView<'_>) {
        let exact = GrantScope::Exact(canonical(call.args));
        if let Some(i) = self
            .grants
            .iter()
            .position(|g| g.tool == call.tool && g.scope == exact)
        {
            self.grants.remove(i);
        }
    }
}

/// JSON with object keys sorted, so equal arguments compare equal.
fn canonical(v: &Value) -> String {
    fn sort(v: &Value) -> Value {
        match v {
            Value::Object(m) => {
                let sorted: BTreeMap<_, _> = m.iter().map(|(k, v)| (k.clone(), sort(v))).collect();
                Value::Object(sorted.into_iter().collect())
            }
            Value::Array(a) => Value::Array(a.iter().map(sort).collect()),
            other => other.clone(),
        }
    }
    sort(v).to_string()
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn call<'a>(
        tool: &'a str,
        effect: EffectClass,
        target: Option<&'a str>,
        args: &'a Value,
    ) -> CallView<'a> {
        CallView {
            tool,
            effect,
            target,
            args,
        }
    }

    #[test]
    fn defaults_allow_safe_classes_and_ask_for_risky_ones() {
        let mut g = PolicyGate::default();
        let a = json!({});
        assert!(matches!(
            g.check(&call("read_note", EffectClass::Read, None, &a)),
            Decision::Allow { .. }
        ));
        assert!(matches!(
            g.check(&call("web_search", EffectClass::Network, None, &a)),
            Decision::Allow { .. }
        ));
        assert_eq!(
            g.check(&call("delete_note", EffectClass::Destructive, None, &a)),
            Decision::Ask
        );
        assert_eq!(
            g.check(&call("post", EffectClass::ExternalSideEffect, None, &a)),
            Decision::Ask
        );
    }

    #[test]
    fn deny_rule_beats_a_grant() {
        let mut g = PolicyGate::new(
            PolicyConfig::default().with_mode(EffectClass::Destructive, Mode::Deny),
        );
        let a = json!({"path": "notes/a.md"});
        let c = call(
            "delete_note",
            EffectClass::Destructive,
            Some("notes/a.md"),
            &a,
        );
        g.answer(&c, ApprovalAnswer::AllowForSession);
        assert!(matches!(g.check(&c), Decision::Deny { .. }));
    }

    #[test]
    fn once_covers_exact_arguments_only_and_is_spent() {
        let mut g = PolicyGate::default();
        let a = json!({"path": "a.md", "force": true});
        let same_reordered: Value =
            serde_json::from_str(r#"{"force":true,"path":"a.md"}"#).unwrap();
        let other = json!({"path": "b.md", "force": true});
        let c = call("delete_note", EffectClass::Destructive, Some("a.md"), &a);
        assert!(g.answer(&c, ApprovalAnswer::AllowOnce));
        assert!(matches!(
            g.check(&call(
                "delete_note",
                EffectClass::Destructive,
                Some("a.md"),
                &same_reordered
            )),
            Decision::Allow { .. }
        ));
        assert_eq!(
            g.check(&call(
                "delete_note",
                EffectClass::Destructive,
                Some("b.md"),
                &other
            )),
            Decision::Ask
        );
        g.consume(&c);
        assert_eq!(g.check(&c), Decision::Ask);
    }

    #[test]
    fn session_grant_is_scoped_to_its_target() {
        let mut g = PolicyGate::default();
        let a = json!({"to": "github.com"});
        let c = call(
            "push",
            EffectClass::ExternalSideEffect,
            Some("github.com"),
            &a,
        );
        g.answer(&c, ApprovalAnswer::AllowForSession);
        let a2 = json!({"to": "github.com", "branch": "x"});
        assert!(matches!(
            g.check(&call(
                "push",
                EffectClass::ExternalSideEffect,
                Some("github.com"),
                &a2
            )),
            Decision::Allow { .. }
        ));
        let a3 = json!({"to": "gitlab.com"});
        assert_eq!(
            g.check(&call(
                "push",
                EffectClass::ExternalSideEffect,
                Some("gitlab.com"),
                &a3
            )),
            Decision::Ask,
            "a new target must ask again"
        );
        assert_eq!(
            g.check(&call(
                "post",
                EffectClass::ExternalSideEffect,
                Some("github.com"),
                &a
            )),
            Decision::Ask,
            "a grant covers one tool"
        );
    }

    fn t0() -> OffsetDateTime {
        OffsetDateTime::UNIX_EPOCH
    }
    fn t31() -> OffsetDateTime {
        OffsetDateTime::UNIX_EPOCH + Duration::minutes(31)
    }

    #[test]
    fn session_grants_expire() {
        let mut g = PolicyGate::default().with_clock(t0);
        let a = json!({});
        let c = call(
            "push",
            EffectClass::ExternalSideEffect,
            Some("github.com"),
            &a,
        );
        g.answer(&c, ApprovalAnswer::AllowForSession);
        assert!(matches!(g.check(&c), Decision::Allow { .. }));
        let mut later = g.with_clock(t31);
        assert_eq!(later.check(&c), Decision::Ask);
    }

    #[test]
    fn a_denied_answer_grants_nothing() {
        let mut g = PolicyGate::default();
        let a = json!({});
        let c = call("post", EffectClass::ExternalSideEffect, None, &a);
        assert!(!g.answer(&c, ApprovalAnswer::Deny));
        assert_eq!(g.check(&c), Decision::Ask);
    }
}
