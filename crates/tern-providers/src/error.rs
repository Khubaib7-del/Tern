use thiserror::Error;

/// Provider failures, worded for the user. Response bodies are redacted
/// before they are kept here.
#[derive(Debug, Error, PartialEq, Eq)]
pub enum ProviderError {
    #[error("{provider} needs an API key. Add one with `tern provider add {id}`.")]
    MissingKey {
        provider: &'static str,
        id: &'static str,
    },
    #[error(
        "{provider} refused the key (HTTP {status}). Check it is current and has access to this model."
    )]
    Auth { provider: &'static str, status: u16 },
    #[error("{provider} is rate limiting this key{}. Wait and try again.", retry_after.map(|s| format!(" (retry after {s} s)")).unwrap_or_default())]
    RateLimited {
        provider: &'static str,
        retry_after: Option<u64>,
    },
    #[error("{provider} does not know {what}.")]
    NotFound {
        provider: &'static str,
        what: String,
    },
    #[error("{provider} returned HTTP {status}: {body}")]
    Http {
        provider: &'static str,
        status: u16,
        body: String,
    },
    #[error("Could not reach {provider}: {reason}")]
    Network {
        provider: &'static str,
        reason: String,
    },
    #[error("{provider} sent a reply Tern could not read: {reason}")]
    Decode {
        provider: &'static str,
        reason: String,
    },
    #[error("Invalid base URL `{0}`. Use an http or https address.")]
    BadUrl(String),
}

pub type ProviderResult<T> = Result<T, ProviderError>;
