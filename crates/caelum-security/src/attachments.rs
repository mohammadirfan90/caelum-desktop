//! Encrypted attachment store (docs/05 §2, F-CLN-04, CR-019 §6): one file per attachment,
//! AES-256-GCM with a random 96-bit nonce and the attachment ID as associated data. The key is
//! derived from the database key (SHA-256 with a domain tag), so it cannot drift from the records.
//! Reading verifies the tag and the plaintext hash; a mismatch is an error, never partial data.

use std::path::{Path, PathBuf};

use aes_gcm::aead::{Aead, KeyInit, Payload};
use aes_gcm::{Aes256Gcm, Nonce};
use sha2::{Digest, Sha256};
use zeroize::Zeroizing;

use crate::keys::{hex, random_bytes, DB_KEY_ENTRY};
use crate::keystore::KeyStore;
use crate::SecurityError;

/// Domain tag of the attachment key derivation.
const KEY_TAG: &[u8] = b"CAELUM-ATTACHMENT-KEY-1";
/// File format version byte.
const VERSION: u8 = 1;

/// Attachment store errors.
#[derive(Debug, thiserror::Error)]
pub enum AttachmentError {
    /// Key or randomness failure.
    #[error(transparent)]
    Security(#[from] SecurityError),
    /// File system failure.
    #[error("attachment file: {0}")]
    Io(#[from] std::io::Error),
    /// The file was changed, truncated or belongs to another attachment.
    #[error("attachment {0} failed its integrity check")]
    Integrity(String),
    /// The ID is not a plain identifier (it names a file).
    #[error("invalid attachment id")]
    InvalidId,
}

/// The attachment key, derived from the database key (which must exist).
pub fn attachment_key(store: &dyn KeyStore) -> Result<Zeroizing<[u8; 32]>, SecurityError> {
    let db = store
        .get(DB_KEY_ENTRY)?
        .ok_or(SecurityError::KeyMissing(DB_KEY_ENTRY))?;
    if db.len() != 32 {
        return Err(SecurityError::KeyMalformed(DB_KEY_ENTRY));
    }
    let mut h = Sha256::new();
    h.update(KEY_TAG);
    h.update(db.as_slice());
    Ok(Zeroizing::new(h.finalize().into()))
}

/// A stored attachment's identity.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Stored {
    /// SHA-256 of the plaintext, hex.
    pub sha256: String,
    /// Plaintext length.
    pub len: usize,
}

/// Encrypted files in one directory.
pub struct AttachmentStore {
    dir: PathBuf,
    key: Zeroizing<[u8; 32]>,
}

impl AttachmentStore {
    /// A store in `dir` (created if missing) with `key`.
    pub fn open(dir: &Path, key: Zeroizing<[u8; 32]>) -> Result<Self, AttachmentError> {
        std::fs::create_dir_all(dir)?;
        Ok(Self {
            dir: dir.to_path_buf(),
            key,
        })
    }

    fn path(&self, id: &str) -> Result<PathBuf, AttachmentError> {
        let plain = !id.is_empty()
            && id.len() <= 64
            && id.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-');
        if !plain {
            return Err(AttachmentError::InvalidId);
        }
        Ok(self.dir.join(format!("{id}.bin")))
    }

    fn cipher(&self) -> Aes256Gcm {
        Aes256Gcm::new(self.key.as_slice().into())
    }

    /// Encrypts and writes `plaintext` as attachment `id`.
    pub fn put(&self, id: &str, plaintext: &[u8]) -> Result<Stored, AttachmentError> {
        let path = self.path(id)?;
        let nonce_bytes = random_bytes::<12>()?;
        let ciphertext = self
            .cipher()
            .encrypt(
                Nonce::from_slice(&nonce_bytes),
                Payload {
                    msg: plaintext,
                    aad: id.as_bytes(),
                },
            )
            .map_err(|_| AttachmentError::Integrity(id.to_owned()))?;
        let mut file = Vec::with_capacity(1 + 12 + ciphertext.len());
        file.push(VERSION);
        file.extend_from_slice(&nonce_bytes);
        file.extend_from_slice(&ciphertext);
        let tmp = path.with_extension("tmp");
        std::fs::write(&tmp, &file)?;
        std::fs::rename(&tmp, &path)?;
        Ok(Stored {
            sha256: hex(&Sha256::digest(plaintext)),
            len: plaintext.len(),
        })
    }

    /// Reads and decrypts attachment `id`, checking it against the recorded `sha256`.
    pub fn get(&self, id: &str, sha256: &str) -> Result<Vec<u8>, AttachmentError> {
        let file = std::fs::read(self.path(id)?)?;
        let integrity = || AttachmentError::Integrity(id.to_owned());
        if file.len() < 1 + 12 + 16 || file.first() != Some(&VERSION) {
            return Err(integrity());
        }
        let (nonce, ciphertext) = file[1..].split_at(12);
        let plaintext = self
            .cipher()
            .decrypt(
                Nonce::from_slice(nonce),
                Payload {
                    msg: ciphertext,
                    aad: id.as_bytes(),
                },
            )
            .map_err(|_| integrity())?;
        if hex(&Sha256::digest(&plaintext)) != sha256 {
            return Err(integrity());
        }
        Ok(plaintext)
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used)]
    use super::*;

    fn store() -> (AttachmentStore, PathBuf) {
        let dir = std::env::temp_dir().join(format!("caelum-att-{}", uuid::Uuid::new_v4()));
        (
            AttachmentStore::open(&dir, Zeroizing::new([7u8; 32])).unwrap(),
            dir,
        )
    }

    #[test]
    fn cr_019_attachments_round_trip_encrypted() {
        let (s, dir) = store();
        let stored = s.put("a1", b"DICM sample bytes").unwrap();
        let raw = std::fs::read(dir.join("a1.bin")).unwrap();
        assert!(!raw.windows(4).any(|w| w == b"DICM"), "stored encrypted");
        assert_eq!(s.get("a1", &stored.sha256).unwrap(), b"DICM sample bytes");
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn cr_019_tampering_swapping_or_wrong_keys_are_refused() {
        let (s, dir) = store();
        let a = s.put("a1", b"first").unwrap();
        let b = s.put("a2", b"second").unwrap();
        // Flip one ciphertext byte.
        let mut raw = std::fs::read(dir.join("a1.bin")).unwrap();
        let last = raw.len() - 1;
        raw[last] ^= 1;
        std::fs::write(dir.join("a1.bin"), &raw).unwrap();
        assert!(matches!(
            s.get("a1", &a.sha256),
            Err(AttachmentError::Integrity(_))
        ));
        // Another attachment's file under this ID (associated data differs).
        std::fs::copy(dir.join("a2.bin"), dir.join("a3.bin")).unwrap();
        assert!(matches!(
            s.get("a3", &b.sha256),
            Err(AttachmentError::Integrity(_))
        ));
        // Wrong recorded hash.
        assert!(matches!(
            s.get("a2", &a.sha256),
            Err(AttachmentError::Integrity(_))
        ));
        // Wrong key.
        let other = AttachmentStore::open(&dir, Zeroizing::new([8u8; 32])).unwrap();
        assert!(matches!(
            other.get("a2", &b.sha256),
            Err(AttachmentError::Integrity(_))
        ));
        // Truncated and path-like IDs.
        std::fs::write(dir.join("a4.bin"), [1u8, 2, 3]).unwrap();
        assert!(matches!(
            s.get("a4", &b.sha256),
            Err(AttachmentError::Integrity(_))
        ));
        assert!(matches!(
            s.put("../x", b"x"),
            Err(AttachmentError::InvalidId)
        ));
        let _ = std::fs::remove_dir_all(dir);
    }
}
