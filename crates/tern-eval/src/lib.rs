//! Tern's evaluation harness (`docs/evaluation.md`).
//!
//! Versioned task suites ([`suite`]) run against a [`Subject`] for several
//! trials ([`run`]) into an [`EvalRun`] with metrics, 95% intervals, and
//! every failure ([`report`]); runs compare against each other and against
//! the bundled baseline. The test ladder's rungs are [`Level`]s: unit
//! ([`unit`]), synthetic and pinned suites, a public benchmark
//! ([`public`]), and human sessions ([`usability`]).
//!
//! The crate does no I/O and holds no clock: `tern-agent` provides the
//! subject (Tern's model, trust scan, untrusted wrapper, policy gate,
//! reader, and decision models), reads and writes `<data>/evals/`, and
//! names the runs.

pub mod public;
pub mod report;
pub mod run;
pub mod stats;
pub mod subject;
pub mod suite;
pub mod unit;
pub mod usability;

pub use report::{
    Baseline, Better, Change, Comparison, Delta, EvalRun, Failure, Metric, RunSummary,
    SuiteResult, Unit, baseline, check_baseline, compare,
};
pub use run::{MIN_TRIALS, RunOptions, run_suite};
pub use subject::{BoxFuture, Completion, GateAnswer, Subject};
pub use suite::{Level, Suite, SuiteKind, bundled, for_level};
