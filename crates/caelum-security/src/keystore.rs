//! The key store: where the database key and the instance signing key live (F-SYS-02, R-SEC-01).
//!
//! Keys are kept only in the operating system's credential store (Windows Credential Manager,
//! macOS Keychain, Linux Secret Service), never in a file. [`KeyStore`] is CAELUM's own interface,
//! so the `keyring-core` crates (CR-006) or a hardware token can be swapped without touching
//! callers.

use std::collections::HashMap;
use std::sync::Arc;

use keyring_core::CredentialStore;
use zeroize::Zeroizing;

/// Keystore service name: the bundle identifier (CR-003, CR-006 §2).
pub const SERVICE: &str = "org.chronostrider.caelum";

/// A secret store.
pub trait KeyStore: Send + Sync {
    /// The secret stored under `name`, or `None` if there is none.
    fn get(&self, name: &str) -> Result<Option<Zeroizing<Vec<u8>>>, KeyStoreError>;
    /// Stores `secret` under `name`, replacing any previous value.
    fn set(&self, name: &str, secret: &[u8]) -> Result<(), KeyStoreError>;
    /// Removes the secret under `name` (decommissioning and tests); absent is not an error.
    fn delete(&self, name: &str) -> Result<(), KeyStoreError>;
}

/// Keystore errors.
#[derive(Debug, thiserror::Error)]
pub enum KeyStoreError {
    /// The operating-system store refused or failed.
    #[error("keystore: {0}")]
    Platform(String),
}

impl From<keyring_core::Error> for KeyStoreError {
    fn from(e: keyring_core::Error) -> Self {
        KeyStoreError::Platform(e.to_string())
    }
}

/// The operating-system credential store.
pub struct OsKeyStore {
    store: Arc<CredentialStore>,
    service: String,
}

impl OsKeyStore {
    /// Opens the platform store under the CAELUM service name.
    pub fn open() -> Result<Self, KeyStoreError> {
        Self::open_with_service(SERVICE)
    }

    /// Opens the platform store under another service name (separate installations, tests).
    pub fn open_with_service(service: &str) -> Result<Self, KeyStoreError> {
        Ok(Self {
            store: platform_store()?,
            service: service.to_owned(),
        })
    }

    fn entry(&self, name: &str) -> Result<keyring_core::Entry, KeyStoreError> {
        let modifiers = platform_modifiers();
        Ok(self.store.build(&self.service, name, modifiers.as_ref())?)
    }
}

#[cfg(windows)]
fn platform_store() -> Result<Arc<CredentialStore>, KeyStoreError> {
    Ok(windows_native_keyring_store::Store::new()?)
}

#[cfg(target_os = "macos")]
fn platform_store() -> Result<Arc<CredentialStore>, KeyStoreError> {
    Ok(apple_native_keyring_store::keychain::Store::new()?)
}

#[cfg(target_os = "linux")]
fn platform_store() -> Result<Arc<CredentialStore>, KeyStoreError> {
    Ok(dbus_secret_service_keyring_store::Store::new()?)
}

/// Windows: keep keys on this machine (`Local`), never in a roaming enterprise profile.
#[cfg(windows)]
fn platform_modifiers() -> Option<HashMap<&'static str, &'static str>> {
    Some(HashMap::from([("persistence", "Local")]))
}

#[cfg(not(windows))]
fn platform_modifiers() -> Option<HashMap<&'static str, &'static str>> {
    None
}

impl KeyStore for OsKeyStore {
    fn get(&self, name: &str) -> Result<Option<Zeroizing<Vec<u8>>>, KeyStoreError> {
        match self.entry(name)?.get_secret() {
            Ok(secret) => Ok(Some(Zeroizing::new(secret))),
            Err(keyring_core::Error::NoEntry) => Ok(None),
            Err(e) => Err(e.into()),
        }
    }

    fn set(&self, name: &str, secret: &[u8]) -> Result<(), KeyStoreError> {
        Ok(self.entry(name)?.set_secret(secret)?)
    }

    fn delete(&self, name: &str) -> Result<(), KeyStoreError> {
        match self.entry(name)?.delete_credential() {
            Ok(()) | Err(keyring_core::Error::NoEntry) => Ok(()),
            Err(e) => Err(e.into()),
        }
    }
}
