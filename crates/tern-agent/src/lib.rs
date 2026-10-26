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
//! Extensibility: MCP servers with a trust preview and health checks
//! ([`mcp`]), their output kept in an untrusted data channel
//! ([`untrusted`]); the ACP server for editors ([`acp`]); skills loaded on
//! demand ([`skills`]); `TERN.md` project rules and the effective
//! instructions ([`rules`]); and project folders ([`projects`]).

// Denied, not forbidden: `local_models::gpu` asks ggml for its devices
// and holds the Windows delay-load hook (the only unsafe code).
#![deny(unsafe_code)]

pub mod acp;
pub mod ask;
pub mod attach;
pub mod branch;
pub mod browse;
pub mod cards;
pub mod chat;
pub mod checkpoint;
pub mod compact;
pub mod crawlers;
pub mod decide;
pub mod deep;
pub mod diag;
pub mod doctor;
pub mod engine;
pub mod eval;
pub mod explain;
pub mod git;
pub mod handoff;
pub mod highlight;
pub mod ledger;
pub mod library;
pub mod local_models;
pub mod mcp;
pub mod media;
pub mod mention;
pub mod paths;
pub mod pictures;
pub mod policy;
pub mod prefs;
pub mod projects;
pub mod prompts;
pub mod protocol;
pub mod providers;
pub mod range;
pub mod refs;
pub mod relevance;
pub mod reply;
pub mod report;
pub mod rules;
pub mod run;
pub mod search;
pub mod side;
pub mod skill_author;
pub mod skills;
pub mod sources;
pub mod steer;
pub mod study;
pub mod sub;
pub mod tools;
pub mod transcript;
pub mod trash;
pub mod untrusted;
pub mod vault;
pub mod verify;
pub mod watch;
pub mod web;
pub mod webintent;
pub mod workspace;

pub use ask::{AskOptions, AskReport, Source};
pub use attach::{Attachment, Chip, Role};
pub use chat::{
    ChatOptions, ChatSession, ContextParts, ContextUse, ReasoningSink, Usage, archive_session,
    chat_to_reopen, clean_title, pick_reopen, recent_sessions, remember_chat, rename_session,
    stored_turns, title_from,
};
pub use checkpoint::{
    CheckpointDir, CheckpointSink, OfferKind, RunCheckpoint, RunMode, RunOffer, Stage, WorkerCounts,
};
pub use compact::{Compacted, Compaction};
pub use decide::{Decider, DecisionKind, DecisionPrefs};
pub use deep::{DeepControl, DeepOptions, PlanAnswer, PlanApprover, ResearchPlan, RunAsDrafted};
pub use engine::{
    ApprovalRequest, Approver, Budget, CONTINUE_PROMPT, CancelFlag, DenyAll, EventSink, MemorySink,
    Model, OnText, StoreSink, TurnLoop, TurnOutcome, is_length_stop,
};
pub use policy::{ApprovalAnswer, Decision, Mode, PolicyConfig, PolicyGate};
pub use prefs::Preferences;
pub use report::{ReportFiles, RunReport};
pub use run::{
    DeepRun, PlanState, RawFilter, Run, RunEnd, RunStep, RunSummary, StepKind, WorkerRow,
};
pub use sources::PageRef;
pub use steer::Steering;
pub use tern_providers::{
    Delta, Effort, ImageInput, Thinking, known_context_window, thinking_support,
};
pub use tern_research::deep::WorkerStatus;
pub use tern_store::{Harness, ProjectSummary, SessionSummary, Store};
pub use workspace::{TurnMode, Workspace};

/// OS details the front ends need, from `tern-platform` (front ends depend
/// on `tern-agent` and `tern-core` only).
pub mod platform {
    /// Where Tern keeps data, settings, and caches (Settings › Privacy).
    pub use tern_platform::AppPaths;
    /// Speech WAV helpers for the mic.
    pub use tern_platform::audio;
    /// The microphone (the `mic` feature).
    #[cfg(feature = "mic")]
    pub use tern_platform::mic;
    pub use tern_platform::motion::os_reduces_motion;
    pub use tern_platform::open::{file_url, open};
    pub use tern_platform::os_name;
}

/// Opens the user's database, creating the data folder if needed.
pub fn open_store() -> Result<Store, String> {
    let paths = tern_platform::AppPaths::discover().map_err(|e| e.to_string())?;
    paths.ensure().map_err(|e| e.to_string())?;
    Store::open(&paths.database()).map_err(|e| e.to_string())
}
pub use tools::{Tool, ToolError, ToolOutput, ToolRegistry};
pub use transcript::{PastTurn, SubReport, live_turns, past_turns};
pub use verify::{CheckClaims, ClaimCheck, Tally, Verdict};
