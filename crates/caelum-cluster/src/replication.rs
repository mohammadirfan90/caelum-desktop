//! Leader/follower replication of the event log, sans I/O (F-SYS-04, R-OPS-02).
//!
//! Protocol (one session per connection, the primary is the leader and connects to the peer):
//! 1. Leader → `Hello { fence, head_seq }`. The follower checks the fence against the fail-over
//!    state machine; a stale leader gets `Fenced { fence }` and demotes itself.
//! 2. Follower → `Checkpoints`: hashes of its recent entries and of exponentially spaced older
//!    ones. Because entries are hash-chained, a matching hash at `s` means the logs are identical
//!    up to `s`, and a differing one that they differ from `s` on. While the highest match and
//!    the lowest difference are not adjacent, the leader sends `Probe { after, before }` and the
//!    follower lists the checkpoints between (each round narrows the range 256-fold).
//! 3. Leader → `Resume { after }` at the exact common point. The follower diverts any entries after that point (entries a
//!    fenced primary wrote that were never acknowledged) into quarantine, never deleting them.
//! 4. Leader → `Entries`; the follower verifies each one (sequence, chain, fence, signature),
//!    persists it, and only then answers `Ack { seq }`. Leader → `Heartbeat` every interval.
//!
//! **Acknowledged** means the follower has persisted the entry: an entry with
//! `seq ≤ acked` survives the loss of the leader, so a takeover loses no acknowledged data.

use crate::fence::Fence;
use crate::log::{verify_next, ChainHead, Hash, LogEntry, LogError, TrustedSigners};
use crate::StoreFailure;

/// Recent entries listed one by one in the checkpoints.
const DENSE_CHECKPOINTS: u64 = 64;

/// Default number of entries the leader may have in flight without acknowledgement.
pub const DEFAULT_WINDOW: u64 = 1024;

/// The event log of one node, implemented by the core on SQLite.
///
/// `append` and `divert_after` must be durable when they return: the follower acknowledges an
/// entry right after `append` succeeds.
pub trait LogStore: Send {
    /// The last entry, or [`ChainHead::genesis`].
    fn head(&self) -> Result<ChainHead, StoreFailure>;
    /// Appends `entry` (already verified) durably; in the same transaction the core applies it to
    /// its record.
    fn append(&mut self, entry: &LogEntry) -> Result<(), StoreFailure>;
    /// The entry at `seq`.
    fn get(&self, seq: u64) -> Result<Option<LogEntry>, StoreFailure>;
    /// Up to `limit` entries from `from` on, in order.
    fn range(&self, from: u64, limit: usize) -> Result<Vec<LogEntry>, StoreFailure>;
    /// Moves every entry after `seq` out of the log into a quarantine kept for review and export
    /// (never deleted, never re-applied), removes their effects from the live record, and returns
    /// them.
    fn divert_after(&mut self, seq: u64) -> Result<Vec<LogEntry>, StoreFailure>;
}

/// The highest fence a node has seen or taken, implemented by the core.
pub trait FenceStore: Send {
    /// The persisted fence.
    fn load(&self) -> Result<Option<Fence>, StoreFailure>;
    /// Persists `fence` durably.
    fn save(&mut self, fence: &Fence) -> Result<(), StoreFailure>;
}

/// A replication failure.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum ReplicationError {
    /// An entry was refused.
    #[error(transparent)]
    Log(#[from] LogError),
    /// Persistence failed.
    #[error(transparent)]
    Store(#[from] StoreFailure),
    /// The peer broke the protocol.
    #[error("protocol: {0}")]
    Protocol(&'static str),
}

/// The hash of one of the follower's entries.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Checkpoint {
    /// Sequence number.
    pub seq: u64,
    /// Entry hash.
    pub hash: Hash,
}

/// The follower's checkpoints, newest first: the last 64 entries, then every 2ⁿ-th further back,
/// then the first entry.
pub fn checkpoints(store: &dyn LogStore) -> Result<Vec<Checkpoint>, StoreFailure> {
    let head = store.head()?;
    let mut seqs = Vec::new();
    let mut seq = head.seq;
    let mut step = 1u64;
    let mut listed = 0u64;
    while seq > 0 {
        seqs.push(seq);
        listed += 1;
        if listed >= DENSE_CHECKPOINTS {
            step = step.saturating_mul(2);
        }
        seq = seq.saturating_sub(step);
    }
    if seqs.last().is_some_and(|&s| s != 1) {
        seqs.push(1);
    }
    let mut out = Vec::with_capacity(seqs.len());
    for seq in seqs {
        if let Some(entry) = store.get(seq)? {
            out.push(Checkpoint {
                seq,
                hash: entry.hash(),
            });
        }
    }
    Ok(out)
}

/// Most checkpoints the follower lists for one probe.
pub const PROBE_POINTS: u64 = 256;

/// The follower's checkpoints strictly between `after` and `before`: all of them if there are at
/// most [`PROBE_POINTS`], else that many evenly spaced (answer to a `Probe`).
pub fn checkpoints_between(
    store: &dyn LogStore,
    after: u64,
    before: u64,
) -> Result<Vec<Checkpoint>, StoreFailure> {
    let span = before.saturating_sub(after).saturating_sub(1);
    let step = span.div_ceil(PROBE_POINTS).max(1);
    let mut out = Vec::new();
    let mut seq = before.saturating_sub(1);
    while seq > after {
        if let Some(entry) = store.get(seq)? {
            out.push(Checkpoint {
                seq,
                hash: entry.hash(),
            });
        }
        seq = seq.saturating_sub(step);
    }
    Ok(out)
}

/// The highest checkpoint the leader holds with the same hash (0: nothing in common).
pub fn common_point(store: &dyn LogStore, points: &[Checkpoint]) -> Result<u64, StoreFailure> {
    Ok(bounds(store, points, 0, None)?.0)
}

/// Narrows (highest matching, lowest differing) seq using `points`. Because entries are
/// hash-chained, a match at `s` means the logs agree up to `s`, and a difference at `s` means
/// they differ from `s` on.
fn bounds(
    store: &dyn LogStore,
    points: &[Checkpoint],
    mut lo: u64,
    mut hi: Option<u64>,
) -> Result<(u64, Option<u64>), StoreFailure> {
    for point in points {
        let same = store
            .get(point.seq)?
            .is_some_and(|entry| entry.hash() == point.hash);
        if same {
            lo = lo.max(point.seq);
        } else {
            hi = Some(hi.map_or(point.seq, |h| h.min(point.seq)));
        }
    }
    Ok((lo, hi.filter(|&h| h > lo)))
}

/// What the leader sends after checkpoints.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SyncStep {
    /// The exact common point is known: resume after it.
    Resume(u64),
    /// The logs agree up to `after` and differ from `before`: list the checkpoints between.
    Probe {
        /// Highest seq known to match.
        after: u64,
        /// Lowest seq known to differ.
        before: u64,
    },
}

/// The leader's side of one session.
#[derive(Debug, Clone)]
pub struct LeaderSession {
    fence: Fence,
    window: u64,
    next_send: u64,
    acked: u64,
    synced: bool,
    probe: Option<(u64, u64)>,
}

impl LeaderSession {
    /// A session under `fence` with at most `window` unacknowledged entries in flight.
    pub fn new(fence: Fence, window: u64) -> Self {
        Self {
            fence,
            window: window.max(1),
            next_send: 1,
            acked: 0,
            synced: false,
            probe: None,
        }
    }

    /// The fence of this session.
    pub fn fence(&self) -> &Fence {
        &self.fence
    }

    /// Highest sequence number the follower has persisted.
    pub fn acked(&self) -> u64 {
        self.acked
    }

    /// Whether the resume point is agreed.
    pub fn synced(&self) -> bool {
        self.synced
    }

    /// Handles the follower's checkpoints (the first list, or the answer to a probe). Returns the
    /// exact resume point, or the range to probe next; each probe narrows the range by a factor
    /// of [`PROBE_POINTS`], so only entries the leader really lacks are ever diverted.
    pub fn on_checkpoints(
        &mut self,
        store: &dyn LogStore,
        points: &[Checkpoint],
    ) -> Result<SyncStep, ReplicationError> {
        if self.synced {
            return Err(ReplicationError::Protocol("checkpoints after resume"));
        }
        if let Some(probe) = self.probe {
            if points.iter().any(|p| p.seq <= probe.0 || p.seq >= probe.1) {
                return Err(ReplicationError::Protocol("checkpoint outside the probe"));
            }
        }
        let (lo, hi) = bounds(
            store,
            points,
            self.probe.map_or(0, |p| p.0),
            self.probe.map(|p| p.1),
        )?;
        match hi {
            Some(hi) if hi > lo.saturating_add(1) => {
                if self.probe == Some((lo, hi)) {
                    // The follower listed nothing new: it lacks entries it claimed.
                    return Err(ReplicationError::Protocol("probe made no progress"));
                }
                self.probe = Some((lo, hi));
                Ok(SyncStep::Probe {
                    after: lo,
                    before: hi,
                })
            }
            _ => {
                self.next_send = lo.saturating_add(1);
                self.acked = lo;
                self.synced = true;
                self.probe = None;
                Ok(SyncStep::Resume(lo))
            }
        }
    }

    /// The next entries to send, within the window.
    pub fn next_batch(
        &mut self,
        store: &dyn LogStore,
        max: usize,
    ) -> Result<Vec<LogEntry>, ReplicationError> {
        if !self.synced {
            return Ok(Vec::new());
        }
        let in_flight = self.next_send.saturating_sub(1).saturating_sub(self.acked);
        let budget = self.window.saturating_sub(in_flight);
        let limit = usize::try_from(budget).unwrap_or(usize::MAX).min(max);
        if limit == 0 {
            return Ok(Vec::new());
        }
        let batch = store.range(self.next_send, limit)?;
        if let Some(last) = batch.last() {
            self.next_send = last.seq.saturating_add(1);
        }
        Ok(batch)
    }

    /// The follower persisted everything up to `seq`. Returns the acknowledged sequence number.
    pub fn on_ack(&mut self, fence: &Fence, seq: u64) -> Result<u64, ReplicationError> {
        if fence != &self.fence {
            return Err(ReplicationError::Protocol(
                "acknowledgement for another fence",
            ));
        }
        if seq >= self.next_send {
            return Err(ReplicationError::Protocol(
                "acknowledgement of an entry never sent",
            ));
        }
        self.acked = self.acked.max(seq);
        Ok(self.acked)
    }
}

/// The follower's side of one session.
#[derive(Debug, Clone)]
pub struct FollowerSession {
    leader: Fence,
    head: ChainHead,
    resumed: bool,
}

impl FollowerSession {
    /// A session with the leader `leader`, starting from the follower's `head`.
    pub fn new(leader: Fence, head: ChainHead) -> Self {
        Self {
            leader,
            head,
            resumed: false,
        }
    }

    /// The leader's fence.
    pub fn leader(&self) -> &Fence {
        &self.leader
    }

    /// The follower's head.
    pub fn head(&self) -> &ChainHead {
        &self.head
    }

    /// Applies the resume point: diverts entries after it. Returns the diverted entries.
    pub fn on_resume(
        &mut self,
        store: &mut dyn LogStore,
        after: u64,
    ) -> Result<Vec<LogEntry>, ReplicationError> {
        if self.resumed {
            return Err(ReplicationError::Protocol("resume sent twice"));
        }
        if after > self.head.seq {
            return Err(ReplicationError::Protocol(
                "resume point beyond the follower's log",
            ));
        }
        let diverted = if after < self.head.seq {
            store.divert_after(after)?
        } else {
            Vec::new()
        };
        self.head = store.head()?;
        if self.head.seq != after {
            return Err(ReplicationError::Store(StoreFailure(
                "divert_after left the log at the wrong position".into(),
            )));
        }
        self.resumed = true;
        Ok(diverted)
    }

    /// Verifies and persists one entry. Only after this returns may it be acknowledged.
    pub fn accept(
        &mut self,
        store: &mut dyn LogStore,
        trust: &TrustedSigners,
        entry: &LogEntry,
    ) -> Result<(), ReplicationError> {
        if !self.resumed {
            return Err(ReplicationError::Protocol("entries before resume"));
        }
        if entry.fence > self.leader {
            return Err(LogError::FenceAhead(entry.seq).into());
        }
        verify_next(&self.head, entry, trust)?;
        store.append(entry)?;
        self.head = ChainHead::of(entry);
        Ok(())
    }

    /// The acknowledgement to send: everything up to the head is persisted.
    pub fn ack_seq(&self) -> u64 {
        self.head.seq
    }
}
