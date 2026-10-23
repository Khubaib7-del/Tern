//! The unit rung: `cargo test --workspace` in a source checkout, read
//! back as a run.

use crate::report::{Better, EvalRun, Failure, Metric, REPORT_FORMAT, SuiteResult};
use crate::suite::{Level, SuiteKind};

/// Passed and failed counts, and failing test names, from `cargo test`'s
/// output (every `test result:` line summed).
pub fn parse_cargo_test(output: &str) -> (u64, u64, Vec<String>) {
    let mut passed = 0;
    let mut failed = 0;
    let mut names = Vec::new();
    for line in output.lines() {
        let l = line.trim();
        if let Some(rest) = l.strip_prefix("test result:") {
            for part in rest.split(';') {
                let words: Vec<&str> = part.split_whitespace().collect();
                for w in words.windows(2) {
                    if let Ok(n) = w[0].parse::<u64>() {
                        match w[1] {
                            "passed" => passed += n,
                            "failed" => failed += n,
                            _ => {}
                        }
                    }
                }
            }
        } else if let Some(name) = l
            .strip_prefix("test ")
            .and_then(|r| r.strip_suffix(" ... FAILED"))
        {
            names.push(name.to_owned());
        }
    }
    (passed, failed, names)
}

/// The run for one `cargo test` pass.
pub fn unit_run(id: String, started: String, seconds: f64, output: &str) -> EvalRun {
    let (passed, failed, names) = parse_cargo_test(output);
    let total = passed + failed;
    EvalRun {
        format: REPORT_FORMAT,
        id,
        started,
        seconds,
        level: Level::Unit,
        mode: "offline".into(),
        subject: "cargo test --workspace".into(),
        provider: None,
        trials: 1,
        tern_version: env!("CARGO_PKG_VERSION").into(),
        suites: vec![SuiteResult {
            id: "unit".into(),
            version: 1,
            title: "Unit and integration tests".into(),
            kind: SuiteKind::Tests,
            level: Level::Unit,
            origin: "the workspace's own tests".into(),
            split: None,
            cases: total as usize,
            trials: 1,
            passed: passed as usize,
            attempted: total as usize,
            skipped: 0,
            skip_reason: String::new(),
            headline: "tests_passed".into(),
            metrics: Metric::ratio(
                "tests_passed",
                "Tests passed",
                Better::Higher,
                &[(passed, total)],
            )
            .into_iter()
            .collect(),
            failures: names
                .into_iter()
                .map(|n| Failure {
                    case: n,
                    trial: 1,
                    expected: "ok".into(),
                    got: "FAILED".into(),
                    why: String::new(),
                })
                .collect(),
            input_tokens: 0,
            output_tokens: 0,
            latency_ms: 0,
        }],
        notes: Vec::new(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sums_every_result_line() {
        let out = "running 3 tests\ntest a ... ok\ntest b::c ... FAILED\ntest result: FAILED. 2 passed; 1 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.01s\n\nrunning 4 tests\ntest result: ok. 4 passed; 0 failed; 1 ignored\n";
        let (p, f, names) = parse_cargo_test(out);
        assert_eq!((p, f), (6, 1));
        assert_eq!(names, vec!["b::c".to_owned()]);
        let run = unit_run("x".into(), "t".into(), 1.0, out);
        assert_eq!(run.suites[0].metric("tests_passed").unwrap().k, Some(6));
    }
}
