//! The decision benchmark: a small bundled fixture set with known answers,
//! scored for accuracy, calibration, latency, and cost.

use std::collections::BTreeMap;
use std::time::Instant;

use serde::{Deserialize, Serialize};

use crate::calibrate::{argmax, brier, ece, log_loss};
use crate::model::{Choice, DecisionKind, DecisionModel, DecisionRequest};

/// One benchmark item.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Fixture {
    pub id: String,
    pub kind: DecisionKind,
    pub question: String,
    #[serde(default)]
    pub context: String,
    pub choices: Vec<Choice>,
    /// Index of the right choice.
    pub gold: usize,
}

impl Fixture {
    pub fn request(&self) -> DecisionRequest {
        DecisionRequest::new(self.kind, self.question.clone(), self.choices.clone())
            .with_context(self.context.clone())
    }
}

const BUNDLED: &str = include_str!("../fixtures/bench.json");

/// The fixture set bundled with Tern.
pub fn fixtures() -> Vec<Fixture> {
    serde_json::from_str(BUNDLED).expect("the bundled fixtures parse")
}

/// One model's scores on one kind (or all kinds).
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Score {
    pub items: usize,
    pub correct: usize,
    pub errors: usize,
    pub brier: f32,
    pub log_loss: f32,
    pub ece: f32,
    pub latency_ms_total: u64,
    pub latency_ms_p50: u64,
    pub input_tokens: u64,
    pub usd: f64,
}

impl Score {
    pub fn accuracy(&self) -> f32 {
        if self.items == 0 {
            0.0
        } else {
            self.correct as f32 / self.items as f32
        }
    }
}

/// One model's results.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct ModelResult {
    pub name: String,
    pub overall: Score,
    pub by_kind: BTreeMap<String, Score>,
    /// Items it got wrong, by fixture id.
    pub misses: Vec<String>,
}

/// The benchmark run.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct BenchReport {
    pub fixtures: usize,
    pub results: Vec<ModelResult>,
}

#[derive(Default)]
struct Acc {
    items: usize,
    correct: usize,
    errors: usize,
    brier: f32,
    log_loss: f32,
    calib: Vec<(f32, bool)>,
    latencies: Vec<u64>,
    tokens: u64,
    usd: f64,
}

impl Acc {
    fn score(&self) -> Score {
        let n = self.items.max(1) as f32;
        let mut lat = self.latencies.clone();
        lat.sort_unstable();
        Score {
            items: self.items,
            correct: self.correct,
            errors: self.errors,
            brier: self.brier / n,
            log_loss: self.log_loss / n,
            ece: ece(&self.calib, 5),
            latency_ms_total: lat.iter().sum(),
            latency_ms_p50: lat.get(lat.len() / 2).copied().unwrap_or(0),
            input_tokens: self.tokens,
            usd: self.usd,
        }
    }
}

/// Runs `model` over `fixtures`. A failed call counts as wrong, with a
/// uniform distribution for the calibration scores.
pub async fn run_model(name: &str, model: &dyn DecisionModel, fixtures: &[Fixture]) -> ModelResult {
    let mut all = Acc::default();
    let mut kinds: BTreeMap<String, Acc> = BTreeMap::new();
    let mut misses = Vec::new();
    for f in fixtures {
        let req = f.request();
        let started = Instant::now();
        let res = model.score(&req).await;
        let took = started.elapsed().as_millis() as u64;
        let (probs, tokens, usd, failed) = match res {
            Ok(d) => (d.probabilities, d.cost.input_tokens, d.cost.usd, false),
            Err(_) => (
                vec![1.0 / f.choices.len() as f32; f.choices.len()],
                0,
                0.0,
                true,
            ),
        };
        let top = argmax(&probs);
        let ok = !failed && top == f.gold;
        if !ok {
            misses.push(f.id.clone());
        }
        for acc in [
            &mut all,
            kinds.entry(kind_key(f.kind).to_owned()).or_default(),
        ] {
            acc.items += 1;
            acc.correct += ok as usize;
            acc.errors += failed as usize;
            acc.brier += brier(&probs, f.gold);
            acc.log_loss += log_loss(&probs, f.gold);
            acc.calib.push((probs.get(top).copied().unwrap_or(0.0), ok));
            acc.latencies.push(took);
            acc.tokens += tokens;
            acc.usd += usd;
        }
    }
    ModelResult {
        name: name.to_owned(),
        overall: all.score(),
        by_kind: kinds.into_iter().map(|(k, a)| (k, a.score())).collect(),
        misses,
    }
}

pub fn kind_key(k: DecisionKind) -> &'static str {
    match k {
        DecisionKind::SourceRank => "source_rank",
        DecisionKind::NextTool => "next_tool",
        DecisionKind::ClaimCheck => "claim_check",
        DecisionKind::Triage => "triage",
        DecisionKind::General => "general",
    }
}

impl BenchReport {
    /// A plain table for the terminal and the docs.
    pub fn table(&self) -> String {
        let mut kinds: Vec<&str> = Vec::new();
        for r in &self.results {
            for k in r.by_kind.keys() {
                if !kinds.contains(&k.as_str()) {
                    kinds.push(k);
                }
            }
        }
        let mut out = format!(
            "{} fixtures\n\n| Model | Accuracy | {} | Brier | ECE | p50 latency | Tokens | Cost |\n| --- | --- |{} --- | --- | --- | --- | --- |\n",
            self.fixtures,
            kinds.join(" | "),
            " --- |".repeat(kinds.len())
        );
        for r in &self.results {
            let per: Vec<String> = kinds
                .iter()
                .map(|k| {
                    r.by_kind
                        .get(*k)
                        .map(|s| format!("{}/{}", s.correct, s.items))
                        .unwrap_or_else(|| "–".into())
                })
                .collect();
            out.push_str(&format!(
                "| {} | {:.0}% ({}/{}) | {} | {:.3} | {:.3} | {} ms | {} | ${:.6} |\n",
                r.name,
                r.overall.accuracy() * 100.0,
                r.overall.correct,
                r.overall.items,
                per.join(" | "),
                r.overall.brier,
                r.overall.ece,
                r.overall.latency_ms_p50,
                r.overall.input_tokens,
                r.overall.usd
            ));
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::local::LocalModel;

    #[test]
    fn bundled_fixtures_are_well_formed() {
        let f = fixtures();
        assert!(f.len() >= 40, "{} fixtures", f.len());
        let mut ids = std::collections::BTreeSet::new();
        for x in &f {
            assert!(ids.insert(x.id.clone()), "duplicate id {}", x.id);
            assert!(x.gold < x.choices.len(), "{} gold out of range", x.id);
            assert!(x.request().validate().is_ok(), "{}", x.id);
        }
        for k in [
            DecisionKind::SourceRank,
            DecisionKind::NextTool,
            DecisionKind::ClaimCheck,
            DecisionKind::Triage,
        ] {
            assert!(f.iter().filter(|x| x.kind == k).count() >= 8, "{k:?}");
        }
    }

    #[tokio::test]
    async fn local_model_runs_the_bench() {
        let f = fixtures();
        let r = run_model("Local", &LocalModel, &f).await;
        assert_eq!(r.overall.items, f.len());
        assert_eq!(r.overall.errors, 0);
        assert_eq!(r.overall.usd, 0.0);
        let report = BenchReport {
            fixtures: f.len(),
            results: vec![r],
        };
        assert!(report.table().contains("| Local |"));
    }
}
