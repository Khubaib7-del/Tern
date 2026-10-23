//! What a suite runs against. `tern-agent` provides the real one: the
//! chat model (or none, offline), Tern's trust scan, its untrusted-data
//! wrapper, its policy gate, its reader, and its decision models.

use std::future::Future;
use std::pin::Pin;

use tern_decision::DecisionModel;

pub type BoxFuture<'a, T> = Pin<Box<dyn Future<Output = T> + Send + 'a>>;

/// One model reply with what it used.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Completion {
    pub text: String,
    pub input_tokens: u64,
    pub output_tokens: u64,
    pub latency_ms: u64,
}

/// The policy gate's answer.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum GateAnswer {
    Allow,
    Ask,
    Deny,
}

impl GateAnswer {
    pub fn label(self) -> &'static str {
        match self {
            Self::Allow => "runs without asking",
            Self::Ask => "asks the user",
            Self::Deny => "refused",
        }
    }
}

/// One tool call put to the policy gate, with the session around it.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct GateCall<'a> {
    pub tool: &'a str,
    /// The effect class, in words (`network`, `external side effect`).
    pub effect: &'a str,
    pub target: &'a str,
    /// The address a web read goes to; empty for other calls.
    pub url: &'a str,
    /// Domains the user typed or search listed earlier in the session.
    pub known: &'a [String],
    /// Whether outside content entered the session before the call.
    pub after_untrusted: bool,
}

/// The system under test.
pub trait Subject: Send + Sync {
    /// `offline` or `provider · model`.
    fn label(&self) -> String;

    /// The provider id, for its logo.
    fn provider(&self) -> Option<String> {
        None
    }

    /// Whether a model answers. Offline runs replay recorded answers and
    /// use Tern's offline checks.
    fn live(&self) -> bool {
        false
    }

    /// One model call.
    fn complete<'a>(
        &'a self,
        system: &'a str,
        user: &'a str,
        max_tokens: u32,
    ) -> BoxFuture<'a, Result<Completion, String>>;

    /// A page's text through Tern's reader (pinned suites).
    fn fetch<'a>(&'a self, url: &'a str) -> BoxFuture<'a, Result<String, String>> {
        Box::pin(async move { Err(format!("{url}: no reader offline")) })
    }

    /// The trust scan's danger reasons for a server-written text.
    fn scan(&self, text: &str) -> Vec<String>;

    /// Text wrapped as untrusted data.
    fn wrap(&self, source: &str, text: &str) -> String;

    /// The system prompt's rule for untrusted blocks.
    fn untrusted_rule(&self) -> String;

    /// The policy gate's answer, under default settings, for one call.
    fn gate(&self, call: &GateCall<'_>) -> Result<GateAnswer, String>;

    /// Decision models to score: the baseline first, the one in use last.
    fn deciders(&self) -> Vec<Box<dyn DecisionModel>>;

    /// US dollars per million input and output tokens, when known.
    fn price(&self) -> Option<(f64, f64)> {
        None
    }
}
