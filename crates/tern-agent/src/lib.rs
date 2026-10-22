//! The Tern engine.
//!
//! The bounded turn loop ([`engine`]), the policy gate ([`policy`]), and the
//! typed tool registry ([`tools`]), plus the health checks behind
//! `tern doctor` and provider setup. Front ends reach providers, the
//! platform, and the store only through this crate. Workers and live
//! steering follow in roadmap phase 5.

#![forbid(unsafe_code)]

pub mod ask;
pub mod chat;
pub mod doctor;
pub mod engine;
pub mod policy;
pub mod prefs;
pub mod protocol;
pub mod providers;
pub mod search;
pub mod tools;
pub mod web;

pub use ask::{AskOptions, AskReport, Source};
pub use chat::{ChatOptions, ChatSession, Usage, recent_sessions, title_from};
pub use engine::{
    ApprovalRequest, Approver, Budget, CancelFlag, DenyAll, EventSink, MemorySink, Model, OnText,
    StoreSink, TurnLoop, TurnOutcome,
};
pub use policy::{ApprovalAnswer, Decision, Mode, PolicyConfig, PolicyGate};
pub use prefs::Preferences;
pub use tern_store::{Harness, SessionSummary};
pub use tools::{Tool, ToolError, ToolOutput, ToolRegistry};
