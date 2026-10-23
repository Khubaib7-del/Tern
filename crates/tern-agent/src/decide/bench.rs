//! `tern decide bench`: the bundled fixtures, scored for the baselines and
//! the decision models.
//!
//! | Row | What it is |
//! | --- | --- |
//! | Baseline: fixed order | What Tern does with the decision model off: the search service's order, the plan's order, the first tool, and the word-overlap claim check from `tern-research` |
//! | Reasoning model | The chat model asked each fixture as a numbered multiple-choice question (needs a provider key) |
//! | Local decision model | [`LocalModel`], offline |
//! | Remote decision model | The chosen remote model (needs its key) |

use std::time::Instant;

use tern_decision::bench::{BenchReport, Fixture, fixtures, run_model};
use tern_decision::calibrate::softmax;
use tern_decision::{
    BoxFuture, Cost, Decision, DecisionError, DecisionKind, DecisionModel, DecisionRequest,
};
use tern_providers::{ChatMessage, ChatRequest};
use tern_research::verdict::{SourceText, lexical_verdict, passages};

use crate::engine::Model;

/// Tern without a decision model.
#[derive(Clone, Copy, Debug, Default)]
pub struct FixedOrder;

fn one_hot(n: usize, at: usize, p: f32) -> Vec<f32> {
    let rest = if n > 1 {
        (1.0 - p) / (n - 1) as f32
    } else {
        0.0
    };
    (0..n).map(|i| if i == at { p } else { rest }).collect()
}

impl FixedOrder {
    fn probabilities(req: &DecisionRequest) -> Vec<f32> {
        let n = req.choices.len();
        match req.kind {
            DecisionKind::ClaimCheck => {
                let src = SourceText {
                    n: 1,
                    text: req.context.clone(),
                    ..SourceText::default()
                };
                let verdict = passages(&req.question, &[&src])
                    .into_iter()
                    .next()
                    .map(|p| lexical_verdict(&p))
                    .unwrap_or(tern_research::verdict::Verdict::NotSupported);
                let want = match verdict {
                    tern_research::verdict::Verdict::Supported => "supported",
                    tern_research::verdict::Verdict::Partly => "partly",
                    _ => "not_supported",
                };
                let at = req.choices.iter().position(|c| c.id == want).unwrap_or(0);
                one_hot(n, at, 0.9)
            }
            DecisionKind::NextTool => {
                let at = req
                    .choices
                    .iter()
                    .position(|c| c.id == "web_search")
                    .unwrap_or(0);
                one_hot(n, at, 0.7)
            }
            // The search service's order, or the plan's.
            _ => {
                let logits: Vec<f32> = (0..n).map(|i| -(i as f32)).collect();
                softmax(&logits, 0.7)
            }
        }
    }
}

impl DecisionModel for FixedOrder {
    fn provider(&self) -> &str {
        "baseline"
    }
    fn model(&self) -> &str {
        "fixed-order"
    }
    fn score<'a>(
        &'a self,
        req: &'a DecisionRequest,
    ) -> BoxFuture<'a, Result<Decision, DecisionError>> {
        Box::pin(async move {
            let probabilities = Self::probabilities(req);
            let chosen = tern_decision::calibrate::argmax(&probabilities);
            Ok(Decision {
                kind: req.kind,
                choices: req.choices.iter().map(|c| c.id.clone()).collect(),
                probabilities,
                chosen,
                model: "fixed-order".into(),
                provider: "baseline".into(),
                ..Decision::default()
            })
        })
    }
}

/// The reasoning model answering each fixture as a multiple-choice question.
#[allow(missing_debug_implementations)]
pub struct ReasoningModel<'a> {
    pub model: &'a dyn Model,
    pub model_name: &'a str,
}

/// The multiple-choice prompt the reasoning model gets.
pub fn prompt(req: &DecisionRequest) -> String {
    let mut s = String::new();
    if !req.context.trim().is_empty() {
        s.push_str(&format!("Context:\n{}\n\n", req.context));
    }
    s.push_str(&format!("Question: {}\n\nChoices:\n", req.question));
    for (i, c) in req.choices.iter().enumerate() {
        s.push_str(&format!("{}. {}\n", i + 1, c.text));
    }
    s.push_str("\nReply with only the number of the best choice.");
    s
}

/// The first number in `reply` that names a choice.
pub fn parse_choice(reply: &str, n: usize) -> Option<usize> {
    reply
        .split(|c: char| !c.is_ascii_digit())
        .filter_map(|w| w.parse::<usize>().ok())
        .find(|k| (1..=n).contains(k))
        .map(|k| k - 1)
}

impl DecisionModel for ReasoningModel<'_> {
    fn provider(&self) -> &str {
        self.model.provider_id()
    }
    fn model(&self) -> &str {
        self.model_name
    }
    fn score<'a>(
        &'a self,
        req: &'a DecisionRequest,
    ) -> BoxFuture<'a, Result<Decision, DecisionError>> {
        Box::pin(async move {
            let started = Instant::now();
            let mut r = ChatRequest::new(self.model_name, vec![ChatMessage::user(prompt(req))]);
            r.max_tokens = 16;
            r.temperature = Some(0.0);
            let resp = self
                .model
                .complete(&r)
                .await
                .map_err(|e| DecisionError::Network(e.to_string()))?;
            let at = parse_choice(&resp.text, req.choices.len())
                .ok_or_else(|| DecisionError::BadResponse(resp.text.clone()))?;
            let probabilities = one_hot(req.choices.len(), at, 0.97);
            Ok(Decision {
                kind: req.kind,
                choices: req.choices.iter().map(|c| c.id.clone()).collect(),
                probabilities,
                chosen: at,
                model: self.model_name.to_owned(),
                provider: self.model.provider_id().to_owned(),
                latency_ms: started.elapsed().as_millis() as u64,
                cost: Cost {
                    input_tokens: resp.usage.input_tokens + resp.usage.output_tokens,
                    usd: 0.0,
                },
                ..Decision::default()
            })
        })
    }
}

/// Runs the bench: the fixed-order baseline and the local model always;
/// the reasoning model and a remote decision model when given.
pub async fn run(
    reasoning: Option<ReasoningModel<'_>>,
    remote: Option<&dyn DecisionModel>,
) -> BenchReport {
    let f: Vec<Fixture> = fixtures();
    let mut results = vec![run_model("Baseline: fixed order", &FixedOrder, &f).await];
    if let Some(r) = &reasoning {
        results.push(run_model(&format!("Reasoning model ({})", r.model_name), r, &f).await);
    }
    results.push(run_model("Local decision model", &tern_decision::LocalModel, &f).await);
    if let Some(m) = remote {
        results.push(
            run_model(
                &format!("Remote decision model ({} {})", m.provider(), m.model()),
                m,
                &f,
            )
            .await,
        );
    }
    BenchReport {
        fixtures: f.len(),
        results,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn prompts_and_replies() {
        let req = DecisionRequest::new(
            DecisionKind::General,
            "Which?",
            vec![
                tern_decision::Choice::plain("a"),
                tern_decision::Choice::plain("b"),
            ],
        );
        assert!(prompt(&req).contains("2. b"));
        assert_eq!(parse_choice("The answer is 2.", 2), Some(1));
        assert_eq!(parse_choice("9 or 1", 2), Some(0));
        assert_eq!(parse_choice("none", 2), None);
    }

    #[tokio::test]
    async fn offline_bench_runs_both_rows() {
        let r = run(None, None).await;
        assert_eq!(r.results.len(), 2);
        assert!(r.results.iter().all(|m| m.overall.items == r.fixtures));
        let local = &r.results[1];
        let base = &r.results[0];
        assert!(
            local.overall.correct >= base.overall.correct,
            "local {} vs baseline {}",
            local.overall.correct,
            base.overall.correct
        );
    }
}
