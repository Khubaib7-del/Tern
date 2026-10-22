//! The decision contract: `score(context, question, choices) -> probabilities`.

use std::future::Future;
use std::pin::Pin;

use serde::{Deserialize, Serialize};

/// A boxed future, so decision models can be trait objects.
pub type BoxFuture<'a, T> = Pin<Box<dyn Future<Output = T> + Send + 'a>>;

/// Which decision point asked. Local scoring and the log use it; remote
/// models see only the question and the choices.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DecisionKind {
    /// Which search result to read first.
    SourceRank,
    /// Which tool fits the next step.
    NextTool,
    /// Whether a passage supports a claim.
    ClaimCheck,
    /// What to do now and what later.
    Triage,
    /// Anything else, such as `tern decide`.
    #[default]
    General,
}

impl DecisionKind {
    pub const ALL: [DecisionKind; 5] = [
        Self::SourceRank,
        Self::NextTool,
        Self::ClaimCheck,
        Self::Triage,
        Self::General,
    ];

    pub fn label(self) -> &'static str {
        match self {
            Self::SourceRank => "Source ranking",
            Self::NextTool => "Next tool",
            Self::ClaimCheck => "Claim check",
            Self::Triage => "Now or later",
            Self::General => "Question",
        }
    }

    pub fn parse(s: &str) -> Option<Self> {
        Some(match s.trim().to_lowercase().replace(['-', ' '], "_").as_str() {
            "source_rank" | "sources" | "source" => Self::SourceRank,
            "next_tool" | "tool" => Self::NextTool,
            "claim_check" | "claim" | "claims" => Self::ClaimCheck,
            "triage" | "now_or_later" => Self::Triage,
            "general" | "question" => Self::General,
            _ => return None,
        })
    }
}

/// One fixed answer.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Choice {
    /// A stable id the caller reads the answer by (`web_search`, a URL).
    pub id: String,
    /// What the scorer reads: a label, or a title and snippet.
    pub text: String,
    /// A prior the caller already has, 0 to 1 (a search rank, how overdue
    /// a watch is). Only the local scorer uses it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub prior: Option<f32>,
}

impl Choice {
    pub fn new(id: impl Into<String>, text: impl Into<String>) -> Self {
        Self {
            id: id.into(),
            text: text.into(),
            prior: None,
        }
    }

    pub fn with_prior(mut self, prior: f32) -> Self {
        self.prior = Some(prior.clamp(0.0, 1.0));
        self
    }

    /// A choice whose id is its text.
    pub fn plain(text: impl Into<String>) -> Self {
        let t = text.into();
        Self::new(t.clone(), t)
    }
}

/// One question for a decision model.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct DecisionRequest {
    pub kind: DecisionKind,
    /// What the question is about: a passage, a run's state. May be empty.
    pub context: String,
    pub question: String,
    pub choices: Vec<Choice>,
}

impl DecisionRequest {
    pub fn new(kind: DecisionKind, question: impl Into<String>, choices: Vec<Choice>) -> Self {
        Self {
            kind,
            context: String::new(),
            question: question.into(),
            choices,
        }
    }

    pub fn with_context(mut self, context: impl Into<String>) -> Self {
        self.context = context.into();
        self
    }

    /// The request is answerable: a question and at least two choices.
    pub fn validate(&self) -> Result<(), DecisionError> {
        if self.question.trim().is_empty() {
            return Err(DecisionError::Invalid("the question is empty".into()));
        }
        if self.choices.len() < 2 {
            return Err(DecisionError::Invalid(
                "a decision needs at least two choices".into(),
            ));
        }
        Ok(())
    }
}

/// What a decision cost. Local decisions cost nothing.
#[derive(Clone, Copy, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Cost {
    pub input_tokens: u64,
    /// US dollars, from the provider's published price per million input
    /// tokens. Output is free on every provider Tern supports.
    pub usd: f64,
}

/// A hard check that changed the score.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Override {
    /// The choice the check acted on.
    pub choice: usize,
    /// `forced` (it must be this) or `ruled_out`.
    pub action: String,
    pub reason: String,
}

/// A scored decision, as logged.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Decision {
    pub kind: DecisionKind,
    /// The question, cut to a line for the log.
    pub question: String,
    /// The choices' ids, in request order.
    pub choices: Vec<String>,
    /// One probability per choice, in request order, summing to 1.
    pub probabilities: Vec<f32>,
    /// The most probable choice after hard checks.
    pub chosen: usize,
    /// 0 to 1: high when the probability sits on one choice.
    pub confidence: f32,
    /// Why, in a few words.
    pub rationale: String,
    /// Which model answered (`local-lexical-1`, `jev-1.13.0`).
    pub model: String,
    /// The provider id (`local`, `typesafe`, `subq`, `voyage`).
    pub provider: String,
    pub latency_ms: u64,
    pub cost: Cost,
    /// Why the remote model was not used, when it fell back to local.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub fallback: Option<String>,
    /// Hard checks that overrode the score.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub overrides: Vec<Override>,
    /// Served from the cache: no call was made.
    #[serde(default)]
    pub cached: bool,
}

impl Decision {
    /// The chosen choice's id.
    pub fn chosen_id(&self) -> &str {
        self.choices
            .get(self.chosen)
            .map(String::as_str)
            .unwrap_or("")
    }

    /// Indices of the choices, most probable first (ties keep request order).
    pub fn ranking(&self) -> Vec<usize> {
        let mut idx: Vec<usize> = (0..self.probabilities.len()).collect();
        idx.sort_by(|&a, &b| {
            self.probabilities[b]
                .partial_cmp(&self.probabilities[a])
                .unwrap_or(std::cmp::Ordering::Equal)
                .then(a.cmp(&b))
        });
        idx
    }

    /// The probability of the choice with this id.
    pub fn probability_of(&self, id: &str) -> Option<f32> {
        self.choices
            .iter()
            .position(|c| c == id)
            .map(|i| self.probabilities[i])
    }

    /// One line for logs and the terminal.
    pub fn line(&self) -> String {
        format!(
            "{}: {} ({:.0}%, {}{})",
            self.kind.label(),
            clip(self.chosen_id(), 60),
            self.probabilities.get(self.chosen).copied().unwrap_or(0.0) * 100.0,
            self.model,
            if self.fallback.is_some() {
                ", fell back"
            } else {
                ""
            }
        )
    }
}

/// Errors a decision model can return. The [`crate::Decider`] turns every
/// one of them into a local fallback.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum DecisionError {
    Invalid(String),
    MissingKey(&'static str),
    /// The model cannot answer this kind (a reranker asked a yes/no).
    Unsupported(String),
    Network(String),
    Http {
        status: u16,
        body: String,
    },
    BadResponse(String),
    TimedOut(u64),
}

impl std::fmt::Display for DecisionError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Invalid(e) => write!(f, "The decision could not be asked: {e}."),
            Self::MissingKey(name) => write!(f, "{name} needs a key. Add one in Decision model settings or with `tern decide key add`."),
            Self::Unsupported(e) => write!(f, "{e}"),
            Self::Network(e) => write!(f, "The decision model could not be reached: {e}."),
            Self::Http { status, body } => {
                let hint = match status {
                    401 | 403 => " The key was refused.",
                    402 => " The account is out of credit.",
                    429 => " Too many requests; try again shortly.",
                    _ => "",
                };
                write!(f, "The decision model answered {status}.{hint} {body}")
            }
            Self::BadResponse(e) => write!(f, "The decision model's reply could not be read: {e}."),
            Self::TimedOut(ms) => write!(f, "The decision model took longer than {ms} ms."),
        }
    }
}

impl std::error::Error for DecisionError {}

/// A model that scores fixed choices.
pub trait DecisionModel: Send + Sync {
    /// Provider id: `local`, `typesafe`, `subq`, `voyage`.
    fn provider(&self) -> &str;
    /// Model id sent or used.
    fn model(&self) -> &str;
    /// Whether it can answer this kind of question at all.
    fn supports(&self, _kind: DecisionKind) -> bool {
        true
    }
    /// Scores the choices. The decision's probabilities are in request
    /// order; `chosen`, `confidence`, and the cost are filled in.
    fn score<'a>(&'a self, req: &'a DecisionRequest)
    -> BoxFuture<'a, Result<Decision, DecisionError>>;
}

/// Cuts `s` to `n` characters with an ellipsis.
pub fn clip(s: &str, n: usize) -> String {
    let s = s.split_whitespace().collect::<Vec<_>>().join(" ");
    if s.chars().count() <= n {
        s
    } else {
        let mut out: String = s.chars().take(n.saturating_sub(1)).collect();
        out.push('…');
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ranking_and_lookup() {
        let d = Decision {
            choices: vec!["a".into(), "b".into(), "c".into()],
            probabilities: vec![0.2, 0.5, 0.3],
            chosen: 1,
            ..Decision::default()
        };
        assert_eq!(d.ranking(), [1, 2, 0]);
        assert_eq!(d.chosen_id(), "b");
        assert_eq!(d.probability_of("c"), Some(0.3));
        assert!(d.line().contains("50%"));
    }

    #[test]
    fn requests_need_a_question_and_two_choices() {
        let r = DecisionRequest::new(DecisionKind::General, "q", vec![Choice::plain("a")]);
        assert!(r.validate().is_err());
        let r = DecisionRequest::new(
            DecisionKind::General,
            " ",
            vec![Choice::plain("a"), Choice::plain("b")],
        );
        assert!(r.validate().is_err());
        assert_eq!(DecisionKind::parse("now-or-later"), Some(DecisionKind::Triage));
        assert_eq!(clip("a  b c", 3), "a …");
    }
}
