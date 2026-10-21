use thiserror::Error;

/// Errors raised by the core contracts.
#[derive(Debug, Error, PartialEq, Eq)]
pub enum CoreError {
    #[error("invalid message reference `{0}`: expected Q<n> or Q<n>.<m>")]
    InvalidMessageRef(String),
    #[error("invalid {kind} id `{value}`")]
    InvalidId { kind: &'static str, value: String },
    #[error("turn cannot move from {from:?} to {to:?}")]
    InvalidTransition {
        from: crate::TurnState,
        to: crate::TurnState,
    },
    #[error("turn is already in the terminal state {0:?}")]
    Terminal(crate::TurnState),
}

pub type CoreResult<T> = Result<T, CoreError>;
