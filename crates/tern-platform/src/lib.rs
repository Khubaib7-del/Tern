//! OS integration behind small traits, so ports stay small.
//!
//! Windows is implemented first (Credential Manager for keys). Linux and macOS
//! keychains are planned for roadmap phase 12; until then the default secret
//! store reports itself as unsupported there instead of falling back to
//! anything less safe.

#![forbid(unsafe_code)]

pub mod disk;
pub mod paths;
pub mod secrets;

pub use disk::DiskSpace;
pub use paths::AppPaths;
pub use secrets::{MemorySecretStore, SecretStore, SecretString, default_secret_store};

use thiserror::Error;

#[derive(Debug, Error)]
pub enum PlatformError {
    #[error("no home directory found for this user")]
    NoHome,
    #[error("the OS keychain is not supported on {0} yet (planned for roadmap phase 12)")]
    KeychainUnsupported(&'static str),
    #[error("keychain error: {0}")]
    Keychain(String),
    #[error("invalid secret name `{0}`: use lowercase letters, digits, `.`, `-`, `_`")]
    InvalidName(String),
    #[error(transparent)]
    Io(#[from] std::io::Error),
}

pub type PlatformResult<T> = Result<T, PlatformError>;

/// The OS family this build targets, in plain words.
pub fn os_name() -> &'static str {
    match std::env::consts::OS {
        "windows" => "Windows",
        "linux" => "Linux",
        "macos" => "macOS",
        other => other,
    }
}
