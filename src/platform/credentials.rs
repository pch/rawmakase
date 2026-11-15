//! The system's credential store, where extensions' secret settings are kept
//! ([`rawmakase_extensions::secrets`]): the login keychain on macOS, the
//! Credential Manager on Windows and the Secret Service (GNOME Keyring,
//! KWallet) over D-Bus elsewhere, through `keyring-core`'s stores. Every call
//! may wait (a locked keyring's prompt, a D-Bus round trip), so the app makes
//! them on its credentials thread, never the UI thread.
//!
//! Errors are built from what failed, never from a value: `keyring-core`'s
//! `BadEncoding` and `BadDataFormat` carry the stored bytes, which are not
//! formatted here.
use keyring_core::{CredentialStore, Entry, Error};
use rawmakase_extensions::secrets::{SERVICE, Secret, SecretError, SecretStore};
use std::sync::Arc;

/// Deletes the keys extension `id` left in the system's credential store
/// when it was uninstalled ([`Store::forgotten_keys`]), for removing it from
/// the command line, where waiting is fine. The store is opened (`open`) only
/// when there is a key to delete, so an extension without keys is removed
/// whether or not one is available. Each key deleted leaves the list; the
/// first that can't be stops it, the rest kept for the app's next start.
///
/// [`Store::forgotten_keys`]: rawmakase_extensions::store::Store::forgotten_keys
pub(crate) fn delete_forgotten_keys(
    open: impl FnOnce() -> Result<Arc<dyn SecretStore>, SecretError>,
    extensions: &rawmakase_extensions::store::Store,
    id: &str,
) -> anyhow::Result<()> {
    let keys: Vec<String> = extensions
        .forgotten_keys()
        .map_err(|e| anyhow::anyhow!("{e}"))?
        .into_iter()
        .filter(|(of, _)| of == id)
        .map(|(_, key)| key)
        .collect();
    if keys.is_empty() {
        return Ok(());
    }
    let store = open().map_err(|e| anyhow::anyhow!("{e}"))?;
    for setting in &keys {
        store
            .delete(&rawmakase_extensions::secrets::account(id, setting))
            .map_err(|e| anyhow::anyhow!("its key {setting}: {e}"))?;
        extensions
            .key_deleted(id, setting)
            .map_err(|e| anyhow::anyhow!("{e}"))?;
    }
    Ok(())
}

/// The system's store, or why there is none (Linux without a Secret
/// Service, say). Opening it may wait (the Secret Service's D-Bus session):
/// the credentials thread opens it, and tries again at its next call when
/// it fails.
pub(crate) fn system_store() -> Result<Arc<dyn SecretStore>, SecretError> {
    match platform_store() {
        Ok(store) => Ok(Arc::new(System(store))),
        Err(e) => Err(failure(e)),
    }
}

#[cfg(target_os = "macos")]
fn platform_store() -> keyring_core::Result<Arc<CredentialStore>> {
    Ok(apple_native_keyring_store::keychain::Store::new()?)
}
#[cfg(target_os = "windows")]
fn platform_store() -> keyring_core::Result<Arc<CredentialStore>> {
    Ok(windows_native_keyring_store::Store::new()?)
}
#[cfg(target_os = "linux")]
fn platform_store() -> keyring_core::Result<Arc<CredentialStore>> {
    Ok(zbus_secret_service_keyring_store::Store::new()?)
}
#[cfg(not(any(target_os = "macos", target_os = "windows", target_os = "linux")))]
fn platform_store() -> keyring_core::Result<Arc<CredentialStore>> {
    Err(Error::NoDefaultStore)
}

struct System(Arc<CredentialStore>);
impl System {
    fn entry(&self, account: &str) -> Result<Entry, SecretError> {
        self.0.build(SERVICE, account, None).map_err(failure)
    }
}
impl SecretStore for System {
    fn get(&self, account: &str) -> Result<Option<Secret>, SecretError> {
        match self.entry(account)?.get_password() {
            Ok(value) => Ok(Some(Secret::new(value))),
            Err(Error::NoEntry) => Ok(None),
            Err(e) => Err(failure(e)),
        }
    }
    fn set(&self, account: &str, value: &Secret) -> Result<(), SecretError> {
        self.entry(account)?
            .set_password(value.expose())
            .map_err(failure)
    }
    fn delete(&self, account: &str) -> Result<(), SecretError> {
        match self.entry(account)?.delete_credential() {
            Ok(()) | Err(Error::NoEntry) => Ok(()),
            Err(e) => Err(failure(e)),
        }
    }
}

fn failure(e: Error) -> SecretError {
    match e {
        Error::NoStorageAccess(_) => SecretError::Locked(reason(&e)),
        Error::PlatformFailure(ref p) if refused(p.as_ref()) => SecretError::Locked(reason(&e)),
        other => SecretError::Unavailable(reason(&other)),
    }
}

/// Whether a platform failure is the store refusing access (the user
/// denied it, or it can't ask), which `keyring-core`'s macOS store reports
/// as a failure rather than as no access.
#[cfg(target_os = "macos")]
fn refused(e: &(dyn std::error::Error + Send + Sync + 'static)) -> bool {
    // errSecUserCanceled, errSecAuthFailed, errSecInteractionNotAllowed.
    e.downcast_ref::<security_framework::base::Error>()
        .is_some_and(|e| matches!(e.code(), -128 | -25293 | -25308))
}
#[cfg(not(target_os = "macos"))]
fn refused(_: &(dyn std::error::Error + Send + Sync + 'static)) -> bool {
    false
}

/// What failed, in words that hold no stored value.
fn reason(e: &Error) -> String {
    match e {
        Error::PlatformFailure(p) | Error::NoStorageAccess(p) => p.to_string(),
        Error::NoEntry => "nothing is stored".into(),
        Error::BadEncoding(_) => "what it holds isn't text".into(),
        Error::BadDataFormat(..) => "what it holds is damaged".into(),
        Error::BadStoreFormat(why) => why.clone(),
        Error::TooLong(name, limit) => format!("the {name} is longer than {limit}"),
        Error::Invalid(name, why) => format!("the {name} {why}"),
        Error::Ambiguous(_) => "it holds more than one entry for this setting".into(),
        Error::NoDefaultStore => "there is none on this system".into(),
        Error::NotSupportedByStore(why) => why.clone(),
        _ => "it failed".into(),
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn a_stored_value_never_reaches_an_error() {
        let sentinel = b"sk-SENTINEL-0123456789".to_vec();
        for e in [
            keyring_core::Error::BadEncoding(sentinel.clone()),
            keyring_core::Error::BadDataFormat(sentinel, "x".into()),
        ] {
            let error = super::failure(e).to_string();
            assert!(!error.contains("SENTINEL"), "{error}");
        }
    }
}

#[cfg(test)]
mod uninstall_tests {
    use super::*;
    use rawmakase_extensions::secrets::{MemoryStore, account};
    use rawmakase_extensions::state::State;
    use rawmakase_extensions::store::Store;

    #[test]
    fn removing_from_the_command_line_deletes_the_keys_it_left() {
        let dir = tempfile::tempdir().unwrap();
        let extensions = Store::new(dir.path());
        State::update(dir.path(), |state| {
            state.forget_keys("dev.example.keys", ["key".to_string()]);
            Ok(())
        })
        .unwrap();
        let store: Arc<dyn SecretStore> = Arc::new(MemoryStore::default());
        let kept = account("dev.example.keys", "key");
        let other = account("dev.example.other", "key");
        store.set(&kept, &Secret::new("sk-1")).unwrap();
        store.set(&other, &Secret::new("sk-2")).unwrap();
        delete_forgotten_keys(|| Ok(store.clone()), &extensions, "dev.example.keys").unwrap();
        assert!(store.get(&kept).unwrap().is_none());
        assert!(store.get(&other).unwrap().is_some(), "another's stays");
        assert!(extensions.forgotten_keys().unwrap().is_empty());
        // Without keys left, no store is needed, even one that can't open.
        let unavailable = || Err(SecretError::Unavailable("no Secret Service".into()));
        delete_forgotten_keys(unavailable, &extensions, "dev.example.none").unwrap();
        // With one, a store that can't open keeps it for the next start.
        State::update(dir.path(), |state| {
            state.forget_keys("dev.example.keys", ["key".to_string()]);
            Ok(())
        })
        .unwrap();
        assert!(delete_forgotten_keys(unavailable, &extensions, "dev.example.keys").is_err());
        assert_eq!(extensions.forgotten_keys().unwrap().len(), 1);
    }
}
