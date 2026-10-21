//! Secret storage.
//!
//! Provider and search API keys live only in the OS keychain. Nothing else in
//! Tern stores a key: not the database, not the event log, not a config file.

use std::collections::HashMap;
use std::fmt;
use std::sync::Mutex;

use crate::{PlatformError, PlatformResult};

/// The keychain service name every Tern entry is filed under.
pub const SERVICE: &str = "tern";

pub use tern_core::SecretString;

/// A store for named secrets, such as `provider.openai`.
pub trait SecretStore: Send + Sync + fmt::Debug {
    /// Plain-language name of the backend, for `tern doctor`.
    fn backend(&self) -> &'static str;
    fn set(&self, name: &str, value: &SecretString) -> PlatformResult<()>;
    fn get(&self, name: &str) -> PlatformResult<Option<SecretString>>;
    /// Returns whether an entry existed.
    fn delete(&self, name: &str) -> PlatformResult<bool>;
}

pub(crate) fn check_name(name: &str) -> PlatformResult<()> {
    let ok = !name.is_empty()
        && name.len() <= 128
        && name
            .bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b"._-".contains(&b));
    if ok {
        Ok(())
    } else {
        Err(PlatformError::InvalidName(name.to_owned()))
    }
}

/// In-memory store for tests. Never used for real keys.
#[derive(Debug, Default)]
pub struct MemorySecretStore {
    map: Mutex<HashMap<String, SecretString>>,
}

impl SecretStore for MemorySecretStore {
    fn backend(&self) -> &'static str {
        "in-memory test store"
    }
    fn set(&self, name: &str, value: &SecretString) -> PlatformResult<()> {
        check_name(name)?;
        self.map
            .lock()
            .expect("lock")
            .insert(name.to_owned(), value.clone());
        Ok(())
    }
    fn get(&self, name: &str) -> PlatformResult<Option<SecretString>> {
        check_name(name)?;
        Ok(self.map.lock().expect("lock").get(name).cloned())
    }
    fn delete(&self, name: &str) -> PlatformResult<bool> {
        check_name(name)?;
        Ok(self.map.lock().expect("lock").remove(name).is_some())
    }
}

#[cfg(windows)]
mod windows_store {
    use super::*;

    /// Windows Credential Manager, through the keyring-core ecosystem.
    /// Entries appear as generic credentials named `<name>.tern`.
    #[derive(Debug)]
    pub struct CredentialManagerStore {
        _priv: (),
    }

    impl CredentialManagerStore {
        pub fn open() -> PlatformResult<Self> {
            let store = windows_native_keyring_store::Store::new()
                .map_err(|e| PlatformError::Keychain(e.to_string()))?;
            keyring_core::set_default_store(store);
            Ok(Self { _priv: () })
        }

        fn entry(name: &str) -> PlatformResult<keyring_core::Entry> {
            check_name(name)?;
            keyring_core::Entry::new(SERVICE, name)
                .map_err(|e| PlatformError::Keychain(e.to_string()))
        }
    }

    impl SecretStore for CredentialManagerStore {
        fn backend(&self) -> &'static str {
            "Windows Credential Manager"
        }
        fn set(&self, name: &str, value: &SecretString) -> PlatformResult<()> {
            Self::entry(name)?
                .set_password(value.expose())
                .map_err(|e| PlatformError::Keychain(e.to_string()))
        }
        fn get(&self, name: &str) -> PlatformResult<Option<SecretString>> {
            match Self::entry(name)?.get_password() {
                Ok(v) => Ok(Some(SecretString::new(v))),
                Err(keyring_core::Error::NoEntry) => Ok(None),
                Err(e) => Err(PlatformError::Keychain(e.to_string())),
            }
        }
        fn delete(&self, name: &str) -> PlatformResult<bool> {
            match Self::entry(name)?.delete_credential() {
                Ok(()) => Ok(true),
                Err(keyring_core::Error::NoEntry) => Ok(false),
                Err(e) => Err(PlatformError::Keychain(e.to_string())),
            }
        }
    }
}

#[cfg(windows)]
pub use windows_store::CredentialManagerStore;

/// The OS keychain for this platform.
pub fn default_secret_store() -> PlatformResult<Box<dyn SecretStore>> {
    #[cfg(windows)]
    {
        Ok(Box::new(CredentialManagerStore::open()?))
    }
    #[cfg(not(windows))]
    {
        Err(PlatformError::KeychainUnsupported(crate::os_name()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn round_trip(store: &dyn SecretStore) {
        let name = format!("test.round-trip.{}", std::process::id());
        let value = SecretString::new("sk-test-value-not-a-real-key");
        store.set(&name, &value).unwrap();
        assert_eq!(store.get(&name).unwrap(), Some(value));
        assert!(store.delete(&name).unwrap());
        assert_eq!(store.get(&name).unwrap(), None);
        assert!(!store.delete(&name).unwrap());
    }

    #[test]
    fn memory_store_round_trip() {
        round_trip(&MemorySecretStore::default());
    }

    /// Stores, reads, and deletes a test key in the real OS keychain.
    #[cfg(windows)]
    #[test]
    fn os_keychain_round_trip() {
        round_trip(default_secret_store().unwrap().as_ref());
    }

    #[cfg(not(windows))]
    #[test]
    fn non_windows_keychain_is_reported_unsupported() {
        assert!(matches!(
            default_secret_store(),
            Err(PlatformError::KeychainUnsupported(_))
        ));
    }

    #[test]
    fn names_are_validated() {
        let m = MemorySecretStore::default();
        for bad in ["", "Provider.OpenAI", "a b", "../x", "x/y"] {
            assert!(m.get(bad).is_err(), "{bad:?} accepted");
        }
        assert!(m.get("provider.openai").is_ok());
    }
}
