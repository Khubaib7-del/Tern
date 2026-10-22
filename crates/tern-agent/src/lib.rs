//! The Tern engine.
//!
//! The bounded turn loop ([`engine`]), the policy gate ([`policy`]), and the
//! typed tool registry ([`tools`]), plus the health checks behind
//! `tern doctor` and provider setup. Front ends reach providers, the
//! platform, and the store only through this crate. Workers and live
//! steering follow in roadmap phase 5.

#![forbid(unsafe_code)]

pub mod ask;
pub mod attach;
pub mod branch;
pub mod browse;
pub mod chat;
pub mod doctor;
pub mod engine;
pub mod git;
pub mod mention;
pub mod policy;
pub mod prefs;
pub mod protocol;
pub mod providers;
pub mod reply;
pub mod run;
pub mod search;
pub mod sources;
pub mod tools;
pub mod transcript;
pub mod vault;
pub mod web;

pub use ask::{AskOptions, AskReport, Source};
pub use attach::{Attachment, Chip, Role};
pub use chat::{
    ChatOptions, ChatSession, ContextUse, Usage, archive_session, clean_title, recent_sessions,
    rename_session, title_from,
};
pub use engine::{
    ApprovalRequest, Approver, Budget, CancelFlag, DenyAll, EventSink, MemorySink, Model, OnText,
    StoreSink, TurnLoop, TurnOutcome,
};
pub use policy::{ApprovalAnswer, Decision, Mode, PolicyConfig, PolicyGate};
pub use prefs::Preferences;
pub use run::{RawFilter, Run, RunEnd, RunStep, RunSummary, StepKind};
pub use sources::PageRef;
pub use tern_providers::ImageInput;
pub use tern_store::{Harness, SessionSummary};
pub use tools::{Tool, ToolError, ToolOutput, ToolRegistry};
pub use transcript::{PastTurn, past_turns};
