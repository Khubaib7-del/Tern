//! Shared types and contracts for Tern.
//!
//! `tern-core` depends on no other Tern crate and on no network, UI, provider,
//! or database library (see `docs/03-architecture.md`, section 3).

#![forbid(unsafe_code)]

pub mod browser_ipc;
pub mod effect;
pub mod error;
pub mod event;
pub mod ids;
pub mod redact;
pub mod secret;
pub mod turn;

pub use effect::EffectClass;
pub use error::{CoreError, CoreResult};
pub use event::{EVENT_VERSION, EventEnvelope, Measure, UsageDelta};
pub use ids::{EventId, MessageRef, ProjectId, SessionId, TurnId};
pub use redact::Redactor;
pub use secret::SecretString;
pub use turn::{TurnState, TurnStateMachine};
