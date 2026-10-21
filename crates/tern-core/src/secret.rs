//! A string that never prints itself.

use std::fmt;

/// A secret value, such as an API key. `Debug` and `Display` never show it.
#[derive(Clone, PartialEq, Eq)]
pub struct SecretString(String);

impl SecretString {
    pub fn new(value: impl Into<String>) -> Self {
        Self(value.into())
    }

    /// The value itself. Call only where it is handed to its consumer.
    pub fn expose(&self) -> &str {
        &self.0
    }

    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }
}

impl fmt::Debug for SecretString {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("SecretString([redacted])")
    }
}

impl fmt::Display for SecretString {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("[redacted]")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn never_prints() {
        let s = SecretString::new("sk-live-secret");
        assert!(!format!("{s:?} {s}").contains("sk-live"));
        assert_eq!(s.expose(), "sk-live-secret");
    }
}
