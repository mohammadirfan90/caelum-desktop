//! Key material: the database key and the instance Ed25519 signing key (F-SYS-02, F-SYS-03).
//!
//! Both are 32 random bytes from the operating system, created once per installation and kept
//! only in the [`KeyStore`]. A key is created only for a new installation: if the database already
//! exists and its key is missing, CAELUM stops rather than creating a key that would lock the
//! records away (or start a second, unverifiable audit chain).

use caelum_store::DbKey;
use ed25519_dalek::{Signature, Signer, SigningKey, Verifier, VerifyingKey};
use sha2::{Digest, Sha256};
use zeroize::Zeroizing;

use crate::keystore::KeyStore;
use crate::SecurityError;

/// Keystore entry of the database key.
pub const DB_KEY_ENTRY: &str = "database-key";
/// Keystore entry of the instance signing key.
pub const INSTANCE_KEY_ENTRY: &str = "instance-signing-key";

/// Random bytes from the operating system.
pub fn random_bytes<const N: usize>() -> Result<[u8; N], SecurityError> {
    let mut bytes = [0u8; N];
    getrandom::fill(&mut bytes).map_err(|e| SecurityError::Random(e.to_string()))?;
    Ok(bytes)
}

/// Whether a key may be created when the keystore has none.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum IfMissing {
    /// New installation: create and store a fresh key.
    Create,
    /// Existing installation: a missing key is an error.
    Fail,
}

/// Loads a 32-byte key, creating it if allowed. Returns the key and whether it was created.
fn load_or_create(
    store: &dyn KeyStore,
    name: &'static str,
    if_missing: IfMissing,
) -> Result<(Zeroizing<[u8; 32]>, bool), SecurityError> {
    if let Some(secret) = store.get(name)? {
        let bytes: [u8; 32] = secret
            .as_slice()
            .try_into()
            .map_err(|_| SecurityError::KeyMalformed(name))?;
        return Ok((Zeroizing::new(bytes), false));
    }
    match if_missing {
        IfMissing::Fail => Err(SecurityError::KeyMissing(name)),
        IfMissing::Create => {
            let bytes = Zeroizing::new(random_bytes::<32>()?);
            store.set(name, bytes.as_slice())?;
            // Read back, so a store that silently drops writes is caught now, not at next start.
            match store.get(name)? {
                Some(read) if read.as_slice() == bytes.as_slice() => Ok((bytes, true)),
                _ => Err(SecurityError::KeyNotPersisted(name)),
            }
        }
    }
}

/// The database key. Returns the key and whether it was created.
pub fn database_key(
    store: &dyn KeyStore,
    if_missing: IfMissing,
) -> Result<(DbKey, bool), SecurityError> {
    let (bytes, created) = load_or_create(store, DB_KEY_ENTRY, if_missing)?;
    Ok((DbKey::from_bytes(*bytes), created))
}

/// The instance signing key. Returns the key and whether it was created.
pub fn instance_key(
    store: &dyn KeyStore,
    if_missing: IfMissing,
) -> Result<(InstanceKey, bool), SecurityError> {
    let (bytes, created) = load_or_create(store, INSTANCE_KEY_ENTRY, if_missing)?;
    Ok((InstanceKey::from_seed(&bytes), created))
}

/// This installation's Ed25519 key; signs audit entries. Wiped from memory when dropped.
pub struct InstanceKey(SigningKey);

impl InstanceKey {
    /// Builds the key from its 32-byte seed.
    pub fn from_seed(seed: &[u8; 32]) -> Self {
        Self(SigningKey::from_bytes(seed))
    }

    /// A fresh random key (not stored).
    pub fn generate() -> Result<Self, SecurityError> {
        Ok(Self::from_seed(&Zeroizing::new(random_bytes::<32>()?)))
    }

    /// The public half.
    pub fn verifying_key(&self) -> InstanceVerifier {
        InstanceVerifier(self.0.verifying_key())
    }

    /// Signs `message`.
    pub fn sign(&self, message: &[u8]) -> [u8; 64] {
        self.0.sign(message).to_bytes()
    }

    /// The Ed25519 public key (for signed downlink items, CR-016); the secret never leaves.
    pub fn public_key(&self) -> VerifyingKey {
        self.0.verifying_key()
    }
}

impl std::fmt::Debug for InstanceKey {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "InstanceKey({})", self.verifying_key().key_id())
    }
}

/// The public instance key, for verifying audit signatures.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct InstanceVerifier(VerifyingKey);

impl InstanceVerifier {
    /// Checks a signature over `message`.
    pub fn verify(&self, message: &[u8], signature: &[u8; 64]) -> bool {
        self.0
            .verify(message, &Signature::from_bytes(signature))
            .is_ok()
    }

    /// The 32 public key bytes.
    pub fn to_bytes(&self) -> [u8; 32] {
        self.0.to_bytes()
    }

    /// Short identifier: first 8 bytes of SHA-256 of the public key, hex.
    pub fn key_id(&self) -> String {
        hex(&Sha256::digest(self.0.to_bytes())[..8])
    }
}

/// Lower-case hex.
pub fn hex(bytes: &[u8]) -> String {
    const DIGITS: &[u8; 16] = b"0123456789abcdef";
    let mut s = String::with_capacity(bytes.len() * 2);
    for b in bytes {
        s.push(char::from(DIGITS[usize::from(b >> 4)]));
        s.push(char::from(DIGITS[usize::from(b & 0x0f)]));
    }
    s
}

/// Parses lower-case hex of exactly `N` bytes.
pub(crate) fn unhex<const N: usize>(text: &str) -> Option<[u8; N]> {
    fn digit(c: u8) -> Option<u8> {
        match c {
            b'0'..=b'9' => Some(c - b'0'),
            b'a'..=b'f' => Some(c - b'a' + 10),
            _ => None,
        }
    }
    let raw = text.as_bytes();
    if raw.len() != N * 2 {
        return None;
    }
    let mut out = [0u8; N];
    for (slot, [hi, lo]) in out.iter_mut().zip(raw.as_chunks::<2>().0) {
        *slot = (digit(*hi)? << 4) | digit(*lo)?;
    }
    Some(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hex_round_trips_and_rejects_bad_input() {
        let bytes = [0x00, 0x7f, 0xab, 0xff];
        assert_eq!(hex(&bytes), "007fabff");
        assert_eq!(unhex::<4>("007fabff"), Some(bytes));
        assert_eq!(unhex::<4>("007FABFF"), None, "upper case is not canonical");
        assert_eq!(unhex::<4>("007fab"), None);
        assert_eq!(unhex::<4>("007fabfg"), None);
    }

    #[test]
    fn signatures_verify_only_for_the_signed_message_and_key() {
        let key = InstanceKey::generate().expect("key");
        let other = InstanceKey::generate().expect("key");
        let sig = key.sign(b"entry");
        assert!(key.verifying_key().verify(b"entry", &sig));
        assert!(!key.verifying_key().verify(b"entrz", &sig));
        assert!(!other.verifying_key().verify(b"entry", &sig));
    }

    #[test]
    fn debug_output_shows_only_the_key_id() {
        let key = InstanceKey::from_seed(&[7; 32]);
        let text = format!("{key:?}");
        assert_eq!(
            text,
            format!("InstanceKey({})", key.verifying_key().key_id())
        );
        assert_eq!(key.verifying_key().key_id().len(), 16);
    }
}
