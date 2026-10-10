//! The replicated event log: signed, hash-chained, sequenced entries (F-SYS-04, R-OPS-02).
//!
//! Every station event the primary records is appended here as one entry. An entry carries its
//! sequence number, the fence of the primary that produced it, the event kind, a TAI timestamp,
//! an opaque payload (the core's serialised record), the hash of the previous entry and an
//! Ed25519 signature by the producing node over all of these. The entry hash covers the signed
//! fields and the signature, so changing, removing, reordering or inserting any entry breaks the
//! chain, and an entry from an unknown node or with a bad signature is refused.

use std::collections::BTreeMap;
use std::fmt;

use caelum_security::keys::InstanceKey;
use ed25519_dalek::{Signature, Verifier, VerifyingKey};
use sha2::{Digest, Sha256};

use crate::fence::{Fence, NodeId};

/// Domain separation for entry signatures, so no other CAELUM signature can pass as an entry.
const DOMAIN: &[u8] = b"CAELUM-REPL-1\0";

/// Largest payload of one entry (4 MiB).
pub const MAX_PAYLOAD: usize = 4 * 1024 * 1024;

/// Longest event kind.
pub const MAX_KIND_LEN: usize = 64;

/// A SHA-256 hash.
pub type Hash = [u8; 32];

/// The `prev_hash` of the first entry.
pub const GENESIS_HASH: Hash = [0; 32];

/// The kind of a replicated event, e.g. `observation` or `alarm.action`: lower-case ASCII letters,
/// digits, `.` and `_`, 1 to 64 characters. The core defines the kinds (`docs/integration/M11.md`).
#[derive(
    Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, serde::Serialize, serde::Deserialize,
)]
#[serde(try_from = "String", into = "String")]
pub struct EventKind(String);

impl EventKind {
    /// Validates and wraps a kind.
    pub fn new(kind: &str) -> Result<Self, LogError> {
        let ok = !kind.is_empty()
            && kind.len() <= MAX_KIND_LEN
            && kind
                .bytes()
                .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || matches!(b, b'.' | b'_'));
        if ok {
            Ok(Self(kind.to_owned()))
        } else {
            Err(LogError::InvalidKind)
        }
    }

    /// The kind text.
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl TryFrom<String> for EventKind {
    type Error = LogError;

    fn try_from(value: String) -> Result<Self, Self::Error> {
        EventKind::new(&value)
    }
}

impl From<EventKind> for String {
    fn from(value: EventKind) -> Self {
        value.0
    }
}

impl fmt::Display for EventKind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

/// What the core appends: one station event.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EntryBody {
    /// Event kind.
    pub kind: EventKind,
    /// When the event happened (TAI ns).
    pub at_tai_ns: i64,
    /// The core's serialised record.
    pub payload: Vec<u8>,
}

/// One signed, chained log entry.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LogEntry {
    /// Position in the log, from 1, without gaps.
    pub seq: u64,
    /// The primary that produced the entry.
    pub fence: Fence,
    /// Event kind.
    pub kind: EventKind,
    /// When the event happened (TAI ns).
    pub at_tai_ns: i64,
    /// The core's serialised record.
    pub payload: Vec<u8>,
    /// Hash of the previous entry ([`GENESIS_HASH`] for the first).
    pub prev_hash: Hash,
    /// Ed25519 signature of the producing node over [`Self::signed_bytes`].
    pub signature: [u8; 64],
}

/// The last entry of a log, or the empty log.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ChainHead {
    /// Sequence number of the last entry (0 when empty).
    pub seq: u64,
    /// Hash of the last entry ([`GENESIS_HASH`] when empty).
    pub hash: Hash,
    /// Fence of the last entry (`None` when empty).
    pub fence: Option<Fence>,
}

impl ChainHead {
    /// The empty log.
    pub fn genesis() -> Self {
        Self {
            seq: 0,
            hash: GENESIS_HASH,
            fence: None,
        }
    }

    /// The head after `entry`.
    pub fn of(entry: &LogEntry) -> Self {
        Self {
            seq: entry.seq,
            hash: entry.hash(),
            fence: Some(entry.fence.clone()),
        }
    }
}

/// Signs entries with this node's key.
pub trait EntrySigner: Send + Sync {
    /// The node whose key this is.
    fn node(&self) -> &NodeId;
    /// Ed25519 signature over `message`.
    fn sign(&self, message: &[u8]) -> [u8; 64];
}

/// The instance key (`caelum-security`) as the entry signer of its node.
pub struct InstanceSigner {
    node: NodeId,
    key: InstanceKey,
}

impl InstanceSigner {
    /// Signs as `node` with `key`.
    pub fn new(node: NodeId, key: InstanceKey) -> Self {
        Self { node, key }
    }

    /// The public key bytes, for the peer's [`TrustedSigners`].
    pub fn public_key(&self) -> [u8; 32] {
        self.key.verifying_key().to_bytes()
    }
}

impl EntrySigner for InstanceSigner {
    fn node(&self) -> &NodeId {
        &self.node
    }

    fn sign(&self, message: &[u8]) -> [u8; 64] {
        self.key.sign(message)
    }
}

/// Why an entry or a chain was refused.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum LogError {
    /// The kind is malformed.
    #[error("event kind must be 1 to 64 characters of a-z, 0-9, '.' or '_'")]
    InvalidKind,
    /// The payload exceeds [`MAX_PAYLOAD`].
    #[error("payload of {0} bytes exceeds the limit")]
    PayloadTooLarge(usize),
    /// A gap or repeat in the sequence.
    #[error("expected entry {expected}, got {got}")]
    BadSequence {
        /// Expected sequence number.
        expected: u64,
        /// Received sequence number.
        got: u64,
    },
    /// `prev_hash` does not match the previous entry.
    #[error("entry {0} does not chain to the previous entry")]
    BrokenChain(u64),
    /// Signed by a node that is not a trusted cluster member.
    #[error("entry {seq} is signed by unknown node {node}")]
    UnknownSigner {
        /// Sequence number.
        seq: u64,
        /// Claimed node.
        node: NodeId,
    },
    /// The signature does not verify.
    #[error("entry {0} has an invalid signature")]
    BadSignature(u64),
    /// The fence went backwards along the chain.
    #[error("entry {0} has an older fence than the entry before it")]
    FenceRegression(u64),
    /// The entry claims a fence newer than the primary sending it.
    #[error("entry {0} has a newer fence than its sender")]
    FenceAhead(u64),
    /// A trusted key is malformed.
    #[error("public key of {0} is malformed")]
    BadKey(NodeId),
}

impl LogEntry {
    /// Builds and signs the entry that follows `head`.
    pub fn create(
        head: &ChainHead,
        fence: Fence,
        body: EntryBody,
        signer: &dyn EntrySigner,
    ) -> Result<Self, LogError> {
        if body.payload.len() > MAX_PAYLOAD {
            return Err(LogError::PayloadTooLarge(body.payload.len()));
        }
        let mut entry = Self {
            seq: head.seq.saturating_add(1),
            fence,
            kind: body.kind,
            at_tai_ns: body.at_tai_ns,
            payload: body.payload,
            prev_hash: head.hash,
            signature: [0; 64],
        };
        entry.signature = signer.sign(&entry.signed_bytes());
        Ok(entry)
    }

    /// The canonical bytes the signature covers.
    pub fn signed_bytes(&self) -> Vec<u8> {
        let node = self.fence.node.as_str().as_bytes();
        let kind = self.kind.as_str().as_bytes();
        let mut out =
            Vec::with_capacity(DOMAIN.len() + 64 + node.len() + kind.len() + self.payload.len());
        out.extend_from_slice(DOMAIN);
        out.extend_from_slice(&self.seq.to_be_bytes());
        out.extend_from_slice(&self.fence.epoch.to_be_bytes());
        // Lengths fit: node ≤ 64, kind ≤ 64, payload ≤ 4 MiB (checked on creation and decoding).
        out.extend_from_slice(&(node.len() as u32).to_be_bytes());
        out.extend_from_slice(node);
        out.extend_from_slice(&(kind.len() as u32).to_be_bytes());
        out.extend_from_slice(kind);
        out.extend_from_slice(&self.at_tai_ns.to_be_bytes());
        out.extend_from_slice(&(self.payload.len() as u64).to_be_bytes());
        out.extend_from_slice(&self.payload);
        out.extend_from_slice(&self.prev_hash);
        out
    }

    /// The entry hash: SHA-256 over the signed bytes and the signature.
    pub fn hash(&self) -> Hash {
        let mut h = Sha256::new();
        h.update(self.signed_bytes());
        h.update(self.signature);
        h.finalize().into()
    }
}

/// The public keys of the cluster members, by node.
#[derive(Debug, Clone, Default)]
pub struct TrustedSigners(BTreeMap<NodeId, VerifyingKey>);

impl TrustedSigners {
    /// No trusted nodes.
    pub fn new() -> Self {
        Self::default()
    }

    /// Trusts `node`'s public key.
    pub fn insert(&mut self, node: NodeId, public_key: &[u8; 32]) -> Result<(), LogError> {
        let key =
            VerifyingKey::from_bytes(public_key).map_err(|_| LogError::BadKey(node.clone()))?;
        self.0.insert(node, key);
        Ok(())
    }

    /// Checks the producer's signature on `entry`.
    pub fn verify(&self, entry: &LogEntry) -> Result<(), LogError> {
        let key = self
            .0
            .get(&entry.fence.node)
            .ok_or_else(|| LogError::UnknownSigner {
                seq: entry.seq,
                node: entry.fence.node.clone(),
            })?;
        key.verify(
            &entry.signed_bytes(),
            &Signature::from_bytes(&entry.signature),
        )
        .map_err(|_| LogError::BadSignature(entry.seq))
    }
}

/// Checks that `entry` may follow `head`: sequence, chain, fence order and signature.
pub fn verify_next(
    head: &ChainHead,
    entry: &LogEntry,
    trust: &TrustedSigners,
) -> Result<(), LogError> {
    let expected = head.seq.saturating_add(1);
    if entry.seq != expected {
        return Err(LogError::BadSequence {
            expected,
            got: entry.seq,
        });
    }
    if entry.prev_hash != head.hash {
        return Err(LogError::BrokenChain(entry.seq));
    }
    if head.fence.as_ref().is_some_and(|f| &entry.fence < f) {
        return Err(LogError::FenceRegression(entry.seq));
    }
    if entry.payload.len() > MAX_PAYLOAD {
        return Err(LogError::PayloadTooLarge(entry.payload.len()));
    }
    trust.verify(entry)
}

/// Verifies a whole log segment that follows `start` (audit and export check).
pub fn verify_chain(
    start: &ChainHead,
    entries: &[LogEntry],
    trust: &TrustedSigners,
) -> Result<ChainHead, LogError> {
    let mut head = start.clone();
    for entry in entries {
        verify_next(&head, entry, trust)?;
        head = ChainHead::of(entry);
    }
    Ok(head)
}
