//! Secret settings (`control = "secret"`, such as an API key): their values
//! are kept in a credential store, never in `extensions.json`, which records
//! only which of an extension's secrets are set ([`crate::state::Entry`]'s
//! `secrets`).
//!
//! The host defines what a store does and names each value; the app passes
//! the system's store (the macOS keychain, the Windows Credential Manager,
//! the Secret Service on Linux) and runs every operation off its UI thread,
//! since a store may wait for the user to unlock it. [`MemoryStore`] stands
//! in for it in tests and headless runs.
//!
//! A value never appears in an error, a `Debug` or a `Display`: [`Secret`]
//! redacts it, and [`SecretError`] holds only the store's reason.
use std::collections::HashMap;
use std::sync::Mutex;

/// The service every value is kept under.
pub const SERVICE: &str = "rawmakase";

/// The account a setting's value is kept under: `<extension>/<setting>`.
pub fn account(extension: &str, setting: &str) -> String {
    format!("{extension}/{setting}")
}

/// A secret's value. Its `Debug` doesn't show it.
#[derive(Clone, PartialEq, Eq, Default)]
pub struct Secret(String);
impl Secret {
    /// Wraps a value.
    pub fn new(value: impl Into<String>) -> Self {
        Self(value.into())
    }
    /// The value, to send to the extension's program.
    pub fn expose(&self) -> &str {
        &self.0
    }
    /// Whether none is set.
    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }
}
impl std::fmt::Debug for Secret {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(if self.0.is_empty() {
            "Secret(none)"
        } else {
            "Secret(…)"
        })
    }
}

/// Why a store couldn't do what it was asked. Never holds a value.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum SecretError {
    /// There is no store here (Linux without a Secret Service), or it
    /// failed; the reason, in the store's words.
    Unavailable(String),
    /// The store is locked, or the user refused access.
    Locked(String),
}
impl std::fmt::Display for SecretError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Unavailable(why) => {
                write!(f, "the system's credential store isn't available: {why}")
            }
            Self::Locked(why) => write!(f, "the system's credential store is locked: {why}"),
        }
    }
}
impl std::error::Error for SecretError {}

/// Where secret values are kept. Every call may wait (for a locked store's
/// prompt, a D-Bus round trip), so callers make them off the UI thread.
pub trait SecretStore: Send + Sync {
    /// The value kept for `account`, or `None` when there is none.
    fn get(&self, account: &str) -> Result<Option<Secret>, SecretError>;
    /// Keeps `value` for `account`, replacing what was kept.
    fn set(&self, account: &str, value: &Secret) -> Result<(), SecretError>;
    /// Forgets the value of `account`; nothing kept is no error.
    fn delete(&self, account: &str) -> Result<(), SecretError>;
}

/// A store in memory, for tests and headless runs: nothing outlives the
/// process. [`MemoryStore::unavailable`] makes it fail as a missing system
/// store does.
#[derive(Default)]
pub struct MemoryStore {
    values: Mutex<HashMap<String, Secret>>,
    failing: Mutex<Option<SecretError>>,
}
impl MemoryStore {
    /// A store that fails every call with `error` until [`MemoryStore::heal`].
    pub fn unavailable(&self, error: SecretError) {
        *self.failing.lock().unwrap_or_else(|e| e.into_inner()) = Some(error);
    }
    /// Lets the store work again.
    pub fn heal(&self) {
        *self.failing.lock().unwrap_or_else(|e| e.into_inner()) = None;
    }
    fn check(&self) -> Result<(), SecretError> {
        match &*self.failing.lock().unwrap_or_else(|e| e.into_inner()) {
            Some(error) => Err(error.clone()),
            None => Ok(()),
        }
    }
    fn values(&self) -> std::sync::MutexGuard<'_, HashMap<String, Secret>> {
        self.values.lock().unwrap_or_else(|e| e.into_inner())
    }
}
impl SecretStore for MemoryStore {
    fn get(&self, account: &str) -> Result<Option<Secret>, SecretError> {
        self.check()?;
        Ok(self.values().get(account).cloned())
    }
    fn set(&self, account: &str, value: &Secret) -> Result<(), SecretError> {
        self.check()?;
        self.values().insert(account.to_string(), value.clone());
        Ok(())
    }
    fn delete(&self, account: &str) -> Result<(), SecretError> {
        self.check()?;
        self.values().remove(account);
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_secret_never_shows_in_debug_or_errors() {
        let key = Secret::new("sk-sentinel-0123456789");
        assert_eq!(format!("{key:?}"), "Secret(…)");
        assert_eq!(format!("{:?}", Secret::default()), "Secret(none)");
        let store = MemoryStore::default();
        let account = account("dev.pch.ai-remove", "fal-key");
        assert_eq!(account, "dev.pch.ai-remove/fal-key");
        assert_eq!(store.get(&account), Ok(None));
        store.set(&account, &key).unwrap();
        assert_eq!(store.get(&account).unwrap().unwrap().expose(), key.expose());
        store.unavailable(SecretError::Locked("the keyring is locked".into()));
        let error = store.set(&account, &key).unwrap_err();
        assert!(!error.to_string().contains("sentinel"), "{error}");
        store.heal();
        store.delete(&account).unwrap();
        store.delete(&account).unwrap();
        assert_eq!(store.get(&account), Ok(None));
    }
}
