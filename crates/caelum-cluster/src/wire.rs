//! Replication messages between the two cluster nodes (JSON text frames over a mutual-TLS
//! WebSocket; CR-017 §2). Integrity does not depend on this encoding: every entry is verified by
//! its signature and chain after decoding.

use serde::{Deserialize, Serialize};

use crate::fence::Fence;
use crate::log::{EventKind, LogEntry, MAX_PAYLOAD};
use crate::replication::Checkpoint;

/// Largest WebSocket message accepted on the replication channel (hex doubles payload size).
pub const MAX_FRAME: usize = 3 * MAX_PAYLOAD + 64 * 1024;

/// Entries per `Entries` message.
pub const BATCH: usize = 256;

/// A replication message.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum PeerMessage {
    /// Leader → follower: opens a session.
    Hello {
        /// The leader's fence.
        fence: Fence,
        /// The leader's last sequence number.
        head_seq: u64,
    },
    /// Follower → leader: the leader is stale; this is the current fence.
    Fenced {
        /// The current fence.
        fence: Fence,
    },
    /// Follower → leader: hashes of the follower's entries.
    Checkpoints {
        /// Newest first.
        points: Vec<WireCheckpoint>,
    },
    /// Leader → follower: list the checkpoints strictly between `after` and `before`.
    Probe {
        /// Highest seq known to match.
        after: u64,
        /// Lowest seq known to differ.
        before: u64,
    },
    /// Leader → follower: the logs agree up to `after`.
    Resume {
        /// Last common sequence number.
        after: u64,
    },
    /// Leader → follower: entries in order.
    Entries {
        /// The entries.
        entries: Vec<WireEntry>,
    },
    /// Leader → follower: the leader is alive.
    Heartbeat {
        /// The leader's fence.
        fence: Fence,
        /// The leader's last sequence number.
        head_seq: u64,
    },
    /// Follower → leader: persisted everything up to `seq`.
    Ack {
        /// The leader's fence, as the follower knows it.
        fence: Fence,
        /// Highest persisted sequence number.
        seq: u64,
    },
}

/// A checkpoint on the wire.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct WireCheckpoint {
    /// Sequence number.
    pub seq: u64,
    /// Entry hash, hex.
    pub hash: String,
}

/// A log entry on the wire (binary fields in hex).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct WireEntry {
    /// Sequence number.
    pub seq: u64,
    /// Producer's fence.
    pub fence: Fence,
    /// Event kind.
    pub kind: EventKind,
    /// TAI ns.
    pub at_tai_ns: i64,
    /// Payload, hex.
    pub payload: String,
    /// Previous hash, hex.
    pub prev_hash: String,
    /// Signature, hex.
    pub signature: String,
}

/// A malformed message.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("malformed replication message: {0}")]
pub struct WireError(pub &'static str);

impl From<&LogEntry> for WireEntry {
    fn from(e: &LogEntry) -> Self {
        Self {
            seq: e.seq,
            fence: e.fence.clone(),
            kind: e.kind.clone(),
            at_tai_ns: e.at_tai_ns,
            payload: hex(&e.payload),
            prev_hash: hex(&e.prev_hash),
            signature: hex(&e.signature),
        }
    }
}

impl TryFrom<&WireEntry> for LogEntry {
    type Error = WireError;

    fn try_from(w: &WireEntry) -> Result<Self, Self::Error> {
        if w.payload.len() > 2 * MAX_PAYLOAD {
            return Err(WireError("payload too large"));
        }
        Ok(Self {
            seq: w.seq,
            fence: w.fence.clone(),
            kind: w.kind.clone(),
            at_tai_ns: w.at_tai_ns,
            payload: unhex(&w.payload).ok_or(WireError("payload"))?,
            prev_hash: unhex_array(&w.prev_hash).ok_or(WireError("prev_hash"))?,
            signature: unhex_array(&w.signature).ok_or(WireError("signature"))?,
        })
    }
}

impl From<&Checkpoint> for WireCheckpoint {
    fn from(c: &Checkpoint) -> Self {
        Self {
            seq: c.seq,
            hash: hex(&c.hash),
        }
    }
}

impl TryFrom<&WireCheckpoint> for Checkpoint {
    type Error = WireError;

    fn try_from(w: &WireCheckpoint) -> Result<Self, Self::Error> {
        Ok(Self {
            seq: w.seq,
            hash: unhex_array(&w.hash).ok_or(WireError("checkpoint hash"))?,
        })
    }
}

impl PeerMessage {
    /// Encodes as JSON text.
    pub fn encode(&self) -> Result<String, WireError> {
        serde_json::to_string(self).map_err(|_| WireError("encode"))
    }

    /// Decodes JSON text.
    pub fn decode(text: &str) -> Result<Self, WireError> {
        if text.len() > MAX_FRAME {
            return Err(WireError("frame too large"));
        }
        serde_json::from_str(text).map_err(|_| WireError("decode"))
    }
}

/// Lower-case hex.
pub fn hex(bytes: &[u8]) -> String {
    caelum_security::keys::hex(bytes)
}

fn nibble(c: u8) -> Option<u8> {
    match c {
        b'0'..=b'9' => Some(c - b'0'),
        b'a'..=b'f' => Some(c - b'a' + 10),
        _ => None,
    }
}

/// Decodes lower-case hex.
pub fn unhex(text: &str) -> Option<Vec<u8>> {
    let bytes = text.as_bytes();
    if !bytes.len().is_multiple_of(2) {
        return None;
    }
    bytes
        .as_chunks::<2>()
        .0
        .iter()
        .map(|p| Some((nibble(p[0])? << 4) | nibble(p[1])?))
        .collect()
}

fn unhex_array<const N: usize>(text: &str) -> Option<[u8; N]> {
    unhex(text)?.try_into().ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn f_sys_04_hex_round_trips_and_rejects_garbage() {
        assert_eq!(
            unhex(&hex(&[0, 1, 0xab, 0xff])),
            Some(vec![0, 1, 0xab, 0xff])
        );
        assert_eq!(unhex("abc"), None);
        assert_eq!(unhex("zz"), None);
        assert_eq!(unhex("AB"), None);
        assert_eq!(unhex_array::<2>("00"), None);
    }
}
