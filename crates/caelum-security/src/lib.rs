//! Security: users, roles, sessions, argon2 credentials, OS keystore, Ed25519 signing and the signed,
//! hash-chained audit log.
//!
//! Implemented in M1: F-SYS-01, F-SYS-02 (keys), F-SYS-03, R-SEC-01, R-SEC-02, R-SEC-03.

pub mod audit;
pub mod clock;
pub mod credential;
pub mod keys;
pub mod keystore;
pub mod role;
pub mod session;
pub mod users;

/// Security errors.
#[derive(Debug, thiserror::Error)]
pub enum SecurityError {
    /// Storage failure.
    #[error(transparent)]
    Store(#[from] caelum_store::StoreError),
    /// Keystore failure.
    #[error(transparent)]
    KeyStore(#[from] keystore::KeyStoreError),
    /// Clock failure.
    #[error(transparent)]
    Clock(#[from] clock::ClockError),
    /// The operating system could not supply random bytes.
    #[error("random source: {0}")]
    Random(String),
    /// Hashing failure.
    #[error("credential: {0}")]
    Credential(String),
    /// A new account or secret breaks the policy.
    #[error("{0}")]
    Policy(String),
    /// A key needed by an existing installation is missing from the keystore.
    #[error("keystore entry {0} is missing; restore it (records cannot be opened without it)")]
    KeyMissing(&'static str),
    /// A keystore entry has the wrong size.
    #[error("keystore entry {0} is malformed")]
    KeyMalformed(&'static str),
    /// A key written to the keystore could not be read back.
    #[error("keystore entry {0} was not stored")]
    KeyNotPersisted(&'static str),
    /// Audit details could not be serialised.
    #[error("audit: {0}")]
    Audit(String),
    /// A stored record is malformed.
    #[error("malformed record: {0}")]
    Malformed(String),
}
