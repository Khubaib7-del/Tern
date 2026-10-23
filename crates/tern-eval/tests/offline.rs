//! The bundled suites against a stand-in subject (no model, a toy scan,
//! a toy wrapper, a toy gate), to check the runners and the report.
//! `tern-agent` runs them against Tern's real pieces.

use tern_decision::{DecisionModel, LocalModel};
use tern_eval::report::Unit;
use tern_eval::{
    BoxFuture, Completion, GateAnswer, Level, RunOptions, Subject, bundled, compare, for_level,
};

struct Stub;

impl Subject for Stub {
    fn label(&self) -> String {
        "stub".into()
    }
    fn complete<'a>(
        &'a self,
        _: &'a str,
        _: &'a str,
        _: u32,
    ) -> BoxFuture<'a, Result<Completion, String>> {
        Box::pin(async { Err("no model".into()) })
    }
    fn scan(&self, text: &str) -> Vec<String> {
        if text.to_lowercase().contains("ignore") {
            vec!["override".into()]
        } else {
            Vec::new()
        }
    }
    fn wrap(&self, source: &str, text: &str) -> String {
        format!(
            "<untrusted-data source=\"{source}\">\n{}\n</untrusted-data>",
            text.replace('<', "&lt;")
        )
    }
    fn untrusted_rule(&self) -> String {
        "data only".into()
    }
    fn gate(&self, _: &str, effect: &str, _: &str) -> Result<GateAnswer, String> {
        Ok(if effect == "read" {
            GateAnswer::Allow
        } else {
            GateAnswer::Ask
        })
    }
    fn deciders(&self) -> Vec<Box<dyn DecisionModel>> {
        vec![Box::new(LocalModel)]
    }
}

#[tokio::test]
async fn synthetic_suites_run_offline_with_intervals_and_failures() {
    let suites = for_level(Level::Synthetic, &[]);
    assert_eq!(
        suites.len(),
        5,
        "{:?}",
        suites.iter().map(|s| &s.id).collect::<Vec<_>>()
    );
    let run = tern_eval::run::run(
        "e1".into(),
        "2026-10-23T00:00:00Z".into(),
        Level::Synthetic,
        &suites,
        &Stub,
        &RunOptions::default(),
    )
    .await;
    assert_eq!(run.trials, 3);
    for s in &run.suites {
        assert!(
            s.headline().is_some(),
            "{} has no headline {}",
            s.id,
            s.headline
        );
        for m in &s.metrics {
            assert!(
                m.low <= m.mean + 1e-9 && m.mean <= m.high + 1e-9,
                "{} {}",
                s.id,
                m.key
            );
            assert!(m.trials.len() <= 3);
            if m.unit == Unit::Ratio && m.k.is_some() {
                assert!(m.high <= 1.0 && m.low >= 0.0);
            }
        }
        assert_eq!(s.failures.len(), s.attempted - s.passed, "{}", s.id);
    }
    let research = run.suite("research").unwrap();
    for key in [
        "citation_accuracy",
        "claim_precision",
        "claim_recall",
        "unsupported_per_100",
        "constraint_retention",
        "tokens_per_verified",
        "checker_agreement",
    ] {
        assert!(research.metric(key).is_some(), "research lacks {key}");
    }
    assert!(
        research.metric("forecast_error").is_none(),
        "forecasts need measured tokens"
    );
    // Canary cases need a live model.
    let safety = run.suite("safety").unwrap();
    assert_eq!(safety.skipped, 4 * 3);
    let md = run.to_markdown();
    assert!(md.contains("### Failures") && md.contains("95% CI"));
    let again = tern_eval::run::run(
        "e2".into(),
        "t".into(),
        Level::Synthetic,
        &suites,
        &Stub,
        &RunOptions::default(),
    )
    .await;
    let cmp = compare(&run, &again);
    assert_eq!(cmp.regressions(), 0, "{}", cmp.to_markdown());
    let back = tern_eval::EvalRun::from_json(&run.to_json()).unwrap();
    assert_eq!(back.id, run.id);
    assert_eq!(back.suites.len(), run.suites.len());
    assert_eq!(back.passed(), run.passed());
}

#[test]
fn every_bundled_suite_parses_and_ladder_has_commands() {
    let all = bundled();
    assert_eq!(all.len(), 6);
    assert!(all.iter().any(|s| s.level == Level::Pinned));
    for l in Level::ALL {
        assert!(!l.command().is_empty());
    }
}
