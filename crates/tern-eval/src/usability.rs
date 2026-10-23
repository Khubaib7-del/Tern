//! The human rung: a person does scripted tasks in Tern, then answers the
//! System Usability Scale (Brooke, 1996). Each session is saved as a run.

use serde::{Deserialize, Serialize};

use crate::report::{Better, EvalRun, Failure, Metric, REPORT_FORMAT, SuiteResult, Unit};
use crate::suite::{Level, SuiteKind};

/// The tasks, in order. Each is something a student does in a first week.
pub const TASKS: &[(&str, &str)] = &[
    ("ask", "Ask a research question and open one of the cited sources."),
    ("verify", "Find which claims of that answer are supported, and why."),
    ("quiz", "Make a five-question quiz from a study material and finish it."),
    ("cards", "Write flashcards from the same material and export them for Anki."),
    ("compare", "Run the offline evaluation and find what changed since the last run."),
];

/// The ten SUS statements. Odd ones are positive, even ones negative.
pub const SUS: [&str; 10] = [
    "I think that I would like to use this system frequently.",
    "I found the system unnecessarily complex.",
    "I thought the system was easy to use.",
    "I think that I would need the support of a technical person to be able to use this system.",
    "I found the various functions in this system were well integrated.",
    "I thought there was too much inconsistency in this system.",
    "I would imagine that most people would learn to use this system very quickly.",
    "I found the system very cumbersome to use.",
    "I felt very confident using the system.",
    "I needed to learn a lot of things before I could get going with this system.",
];

/// The SUS score, 0 to 100, from ten answers of 1 (strongly disagree) to 5.
pub fn sus_score(answers: &[u8; 10]) -> Result<f64, String> {
    if let Some(a) = answers.iter().find(|a| !(1..=5).contains(*a)) {
        return Err(format!("answers are 1 to 5, not {a}"));
    }
    let sum: u32 = answers
        .iter()
        .enumerate()
        .map(|(i, a)| {
            let a = u32::from(*a);
            if i % 2 == 0 { a - 1 } else { 5 - a }
        })
        .sum();
    Ok(f64::from(sum) * 2.5)
}

/// One task as the person did it.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct TaskResult {
    pub id: String,
    pub done: bool,
    pub seconds: f64,
    #[serde(default)]
    pub note: String,
}

/// A usability session as a run on the human rung.
pub fn session_run(
    id: String,
    started: String,
    participant: &str,
    tasks: &[TaskResult],
    sus: &[u8; 10],
) -> Result<EvalRun, String> {
    let score = sus_score(sus)?;
    let done = tasks.iter().filter(|t| t.done).count();
    let secs: Vec<f64> = tasks.iter().filter(|t| t.done).map(|t| t.seconds).collect();
    let failures = tasks
        .iter()
        .filter(|t| !t.done)
        .map(|t| Failure {
            case: t.id.clone(),
            trial: 1,
            expected: "finished".into(),
            got: "not finished".into(),
            why: t.note.clone(),
        })
        .collect();
    let metrics = [
        Metric::ratio(
            "task_success",
            "Tasks finished",
            Better::Higher,
            &[(done as u64, tasks.len() as u64)],
        ),
        Metric::values(
            "task_seconds",
            "Seconds per finished task",
            Unit::Number,
            Better::Lower,
            if secs.is_empty() { Vec::new() } else { vec![crate::stats::mean(&secs)] },
        ),
        Metric::values("sus", "SUS score", Unit::Number, Better::Higher, vec![score])
            .map(|m| m.with_note("0 to 100; 68 is the usual average")),
    ]
    .into_iter()
    .flatten()
    .collect();
    Ok(EvalRun {
        format: REPORT_FORMAT,
        id,
        started,
        seconds: tasks.iter().map(|t| t.seconds).sum(),
        level: Level::Human,
        mode: "human".into(),
        subject: format!("participant {participant}"),
        provider: None,
        trials: 1,
        tern_version: env!("CARGO_PKG_VERSION").into(),
        suites: vec![SuiteResult {
            id: "usability".into(),
            version: 1,
            title: "Usability session".into(),
            kind: SuiteKind::Usability,
            level: Level::Human,
            origin: "a person doing Tern's scripted tasks, then the System Usability Scale (Brooke, 1996)".into(),
            split: None,
            cases: tasks.len(),
            trials: 1,
            passed: done,
            attempted: tasks.len(),
            skipped: 0,
            skip_reason: String::new(),
            headline: "sus".into(),
            metrics,
            failures,
            input_tokens: 0,
            output_tokens: 0,
            latency_ms: 0,
        }],
        notes: vec!["One participant. Pool sessions from several people before reading much into a SUS score.".into()],
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sus_scores_like_the_scale() {
        assert_eq!(sus_score(&[5, 1, 5, 1, 5, 1, 5, 1, 5, 1]).unwrap(), 100.0);
        assert_eq!(sus_score(&[1, 5, 1, 5, 1, 5, 1, 5, 1, 5]).unwrap(), 0.0);
        assert_eq!(sus_score(&[3; 10]).unwrap(), 50.0);
        assert!(sus_score(&[0; 10]).is_err());
    }

    #[test]
    fn a_session_is_a_human_run() {
        let tasks = vec![
            TaskResult { id: "ask".into(), done: true, seconds: 40.0, note: String::new() },
            TaskResult { id: "quiz".into(), done: false, seconds: 90.0, note: "could not find Quiz".into() },
        ];
        let run = session_run("u1".into(), "2026-10-23T00:00:00Z".into(), "P1", &tasks, &[4, 2, 4, 2, 4, 2, 4, 2, 4, 2]).unwrap();
        let s = &run.suites[0];
        assert_eq!(s.metric("sus").unwrap().mean, 75.0);
        assert_eq!(s.metric("task_success").unwrap().mean, 0.5);
        assert_eq!(s.failures.len(), 1);
        assert_eq!(run.level, Level::Human);
    }
}
