//! The Tern engine.
//!
//! The bounded turn loop ([`engine`]), the policy gate ([`policy`]), and the
//! typed tool registry ([`tools`]), plus the health checks behind
//! `tern doctor` and provider setup. Front ends reach providers, the
//! platform, and the store only through this crate. Workers and live
//! steering follow in roadmap phase 5.

#![forbid(unsafe_code)]

pub mod doctor;
pub mod engine;
pub mod policy;
pub mod protocol;
pub mod providers;
pub mod tools;

pub use engine::{
    ApprovalRequest, Approver, Budget, CancelFlag, DenyAll, EventSink, MemorySink, Model,
    StoreSink, TurnLoop, TurnOutcome,
};
pub use policy::{ApprovalAnswer, Decision, Mode, PolicyConfig, PolicyGate};
pub use tools::{Tool, ToolError, ToolOutput, ToolRegistry};
