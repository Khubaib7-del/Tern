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

/// A store backed by the OS keychain through the keyring-core ecosystem.
///
/// | OS | Backend | Where an entry shows |
/// | --- | --- | --- |
/// | Windows | Credential Manager | Generic credential `<name>.tern` |
/// | Linux | Secret Service (GNOME Keyring, KWallet, KeePassXC) over D-Bus | Login collection, label `keyring:<name>@tern` |
/// | Linux, `TERN_KEYRING=keyutils` | The kernel's per-user keyring | Gone after a reboot |
/// | macOS | Keychain | Login keychain, service `tern`, account `<name>` |
#[cfg(any(windows, target_os = "linux", target_os = "macos"))]
#[derive(Debug)]
pub struct KeyringStore {
    backend: &'static str,
}

#[cfg(any(windows, target_os = "linux", target_os = "macos"))]
impl KeyringStore {
    fn with(backend: &'static str, store: std::sync::Arc<keyring_core::CredentialStore>) -> Self {
        keyring_core::set_default_store(store);
        Self { backend }
    }

    fn entry(name: &str) -> PlatformResult<keyring_core::Entry> {
        check_name(name)?;
        keyring_core::Entry::new(SERVICE, name).map_err(|e| PlatformError::Keychain(e.to_string()))
    }
}

#[cfg(any(windows, target_os = "linux", target_os = "macos"))]
impl SecretStore for KeyringStore {
    fn backend(&self) -> &'static str {
        self.backend
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

/// Which Linux backend `TERN_KEYRING` asks for.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LinuxKeyring {
    /// Secret Service over D-Bus (the default).
    SecretService,
    /// The kernel keyring: no daemon needed, but keys last until reboot.
    Keyutils,
}

impl LinuxKeyring {
    /// Reads `TERN_KEYRING` (`secret-service`, the default, or `keyutils`).
    pub fn from_env(value: Option<&str>) -> Self {
        match value.map(|v| v.trim().to_ascii_lowercase()).as_deref() {
            Some("keyutils" | "kernel") => Self::Keyutils,
            _ => Self::SecretService,
        }
    }
}

/// What to do when Linux has no Secret Service.
pub const LINUX_FALLBACK_HINT: &str = "Start a Secret Service provider (GNOME Keyring, KWallet, or KeePassXC with its Secret Service integration) and unlock it, or set TERN_KEYRING=keyutils to keep keys in the kernel keyring until the next reboot. Tern never writes keys to a file.";

#[cfg(target_os = "linux")]
fn linux_store(choice: LinuxKeyring) -> PlatformResult<KeyringStore> {
    match choice {
        LinuxKeyring::SecretService => {
            let store = zbus_secret_service_keyring_store::Store::new().map_err(|e| {
                PlatformError::Keychain(format!(
                    "the Secret Service is not available ({e}). {LINUX_FALLBACK_HINT}"
                ))
            })?;
            Ok(KeyringStore::with("Secret Service (D-Bus)", store))
        }
        LinuxKeyring::Keyutils => {
            let store = linux_keyutils_keyring_store::Store::new()
                .map_err(|e| PlatformError::Keychain(format!("the kernel keyring: {e}")))?;
            Ok(KeyringStore::with(
                "Linux kernel keyring (keys last until reboot)",
                store,
            ))
        }
    }
}

/// The OS keychain for this platform. `TERN_SECRETS=memory` swaps in an
/// empty in-memory store instead (the desktop's UI tour sets it, so a
/// recording never reads or writes a real key).
pub fn default_secret_store() -> PlatformResult<Box<dyn SecretStore>> {
    if std::env::var("TERN_SECRETS").is_ok_and(|v| v == "memory") {
        return Ok(Box::new(MemorySecretStore::default()));
    }
    #[cfg(windows)]
    {
        let store = windows_native_keyring_store::Store::new()
            .map_err(|e| PlatformError::Keychain(e.to_string()))?;
        Ok(Box::new(KeyringStore::with(
            "Windows Credential Manager",
            store,
        )))
    }
    #[cfg(target_os = "linux")]
    {
        let choice = LinuxKeyring::from_env(std::env::var("TERN_KEYRING").ok().as_deref());
        Ok(Box::new(linux_store(choice)?))
    }
    #[cfg(target_os = "macos")]
    {
        let store = apple_native_keyring_store::keychain::Store::new()
            .map_err(|e| PlatformError::Keychain(e.to_string()))?;
        Ok(Box::new(KeyringStore::with("macOS Keychain", store)))
    }
    #[cfg(not(any(windows, target_os = "linux", target_os = "macos")))]
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

    /// The macOS Keychain, when the run allows it (CI sets
    /// `TERN_KEYCHAIN_TEST=1`; a locked keychain on a desk would prompt).
    #[cfg(target_os = "macos")]
    #[test]
    fn os_keychain_round_trip() {
        if std::env::var("TERN_KEYCHAIN_TEST").as_deref() != Ok("1") {
            eprintln!("skipped: set TERN_KEYCHAIN_TEST=1 to use the login keychain");
            return;
        }
        round_trip(default_secret_store().unwrap().as_ref());
    }

    /// Linux: the Secret Service when a session runs one, else the kernel
    /// keyring; a machine with neither (a bare container) skips.
    #[cfg(target_os = "linux")]
    #[test]
    fn linux_keyring_round_trip() {
        for choice in [LinuxKeyring::SecretService, LinuxKeyring::Keyutils] {
            match linux_store(choice) {
                Ok(store) => {
                    // Opening may succeed while the collection is locked.
                    let name = "test.probe";
                    if store.get(name).is_ok() {
                        round_trip(&store);
                        return;
                    }
                }
                Err(e) => eprintln!("{choice:?} unavailable: {e}"),
            }
        }
        eprintln!("skipped: no Secret Service and no kernel keyring here");
    }

    #[test]
    fn linux_backend_choice_reads_the_variable() {
        assert_eq!(LinuxKeyring::from_env(None), LinuxKeyring::SecretService);
        assert_eq!(
            LinuxKeyring::from_env(Some("keyutils")),
            LinuxKeyring::Keyutils
        );
        assert_eq!(
            LinuxKeyring::from_env(Some("anything")),
            LinuxKeyring::SecretService
        );
        assert!(LINUX_FALLBACK_HINT.contains("TERN_KEYRING=keyutils"));
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
