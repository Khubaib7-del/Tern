//! OS integration behind small traits, so ports stay small.
//!
//! | Piece | Windows | Linux | macOS |
//! | --- | --- | --- | --- |
//! | Keys ([`secrets`]) | Credential Manager | Secret Service; kernel keyring with `TERN_KEYRING=keyutils` | Keychain |
//! | Folders ([`paths`]) | `%LOCALAPPDATA%\Tern\data` | XDG data, config, and cache | Application Support and Caches |
//! | Reduced motion ([`motion`]) | Not read yet | GNOME and KDE settings | Accessibility setting |
//! | Opener ([`open`]) | `explorer` | `xdg-open` | `open` |
//!
//! Where an OS has no keychain Tern can reach, the default secret store
//! reports an error with what to do instead of falling back to anything less
//! safe: keys are never written to a file.

#![forbid(unsafe_code)]

pub mod audio;
pub mod disk;
#[cfg(feature = "mic")]
pub mod mic;
pub mod motion;
pub mod open;
pub mod paths;
pub mod secrets;
/// The speakers, for read aloud (#69; cpal, same feature as the mic).
#[cfg(feature = "mic")]
pub mod speaker;

pub use disk::DiskSpace;
pub use paths::AppPaths;
#[cfg(any(windows, target_os = "linux", target_os = "macos"))]
pub use secrets::KeyringStore;
pub use secrets::{MemorySecretStore, SecretStore, SecretString, default_secret_store};

use thiserror::Error;

#[derive(Debug, Error)]
pub enum PlatformError {
    #[error("no home directory found for this user")]
    NoHome,
    #[error("the OS keychain is not supported on {0}")]
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
