//! The Tern engine.
//!
//! The bounded turn loop ([`engine`]), the policy gate ([`policy`]), and the
//! typed tool registry ([`tools`]), plus the health checks behind
//! `tern doctor` and provider setup. Front ends reach providers, the
//! platform, and the store only through this crate. Deep research
//! ([`deep`]) runs an approved plan's workers in parallel. Research runs
//! save checkpoints ([`checkpoint`]) and can be resumed, take steering notes
//! while they run ([`steer`]), keep source and budget ledgers ([`ledger`]),
//! and saved questions can be watched ([`watch`]). The Study harness
//! ([`study`]) keeps the user's materials, topics, and mastery, and runs
//! the tutor, quizzes, and viva practice. The decision model ([`decide`])
//! ranks sources, hints the next tool, pre-scores claims, and orders work.
//! Continuity: message IDs and sub-IDs ([`refs`]), cleaning a chat by ID
//! range ([`range`]), sub-sessions that report back ([`sub`]), handoff
//! packets ([`handoff`]), the Library of everything ([`library`]), the
//! throwaway side chat ([`side`]), and highlight actions ([`highlight`]).

#![forbid(unsafe_code)]

pub mod ask;
pub mod attach;
pub mod branch;
pub mod browse;
pub mod cards;
pub mod chat;
pub mod checkpoint;
pub mod decide;
pub mod deep;
pub mod doctor;
pub mod engine;
pub mod explain;
pub mod git;
pub mod handoff;
pub mod highlight;
pub mod ledger;
pub mod library;
pub mod mcp;
pub mod mention;
pub mod policy;
pub mod prefs;
pub mod protocol;
pub mod providers;
pub mod range;
pub mod refs;
pub mod reply;
pub mod report;
pub mod run;
pub mod search;
pub mod side;
pub mod sources;
pub mod steer;
pub mod study;
pub mod sub;
pub mod tools;
pub mod transcript;
pub mod untrusted;
pub mod vault;
pub mod verify;
pub mod watch;
pub mod web;

pub use ask::{AskOptions, AskReport, Source};
pub use attach::{Attachment, Chip, Role};
pub use chat::{
    ChatOptions, ChatSession, ContextUse, Usage, archive_session, clean_title, recent_sessions,
    rename_session, title_from,
};
pub use checkpoint::{CheckpointDir, CheckpointSink, RunCheckpoint, RunMode, Stage};
pub use decide::{Decider, DecisionKind, DecisionPrefs};
pub use deep::{DeepControl, DeepOptions, PlanAnswer, PlanApprover, ResearchPlan, RunAsDrafted};
pub use engine::{
    ApprovalRequest, Approver, Budget, CancelFlag, DenyAll, EventSink, MemorySink, Model, OnText,
    StoreSink, TurnLoop, TurnOutcome,
};
pub use policy::{ApprovalAnswer, Decision, Mode, PolicyConfig, PolicyGate};
pub use prefs::Preferences;
pub use report::{ReportFiles, RunReport};
pub use run::{
    DeepRun, PlanState, RawFilter, Run, RunEnd, RunStep, RunSummary, StepKind, WorkerRow,
};
pub use sources::PageRef;
pub use steer::Steering;
pub use tern_providers::ImageInput;
pub use tern_research::deep::WorkerStatus;
pub use tern_store::{Harness, SessionSummary, Store};

/// Opens the user's database, creating the data folder if needed.
pub fn open_store() -> Result<Store, String> {
    let paths = tern_platform::AppPaths::discover().map_err(|e| e.to_string())?;
    paths.ensure().map_err(|e| e.to_string())?;
    Store::open(&paths.database()).map_err(|e| e.to_string())
}
pub use tools::{Tool, ToolError, ToolOutput, ToolRegistry};
pub use transcript::{PastTurn, SubReport, live_turns, past_turns};
pub use verify::{CheckClaims, ClaimCheck, Tally, Verdict};
