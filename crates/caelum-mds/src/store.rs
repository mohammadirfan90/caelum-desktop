//! Persistence interfaces so the outbox and the read-back state survive a restart (F-GND-12,
//! R-INT-07). The core implements them on the encrypted SQLite store; [`MemoryStore`] is the
//! in-memory implementation for tests (test equipment only).

use crate::inbox::InboxSnapshot;
use crate::outbox::OutboxRecord;

/// A persistence failure. The connector stops and reports it; nothing is assumed written.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("MDS store: {0}")]
pub struct StoreError(pub String);

/// Durable outbox records.
pub trait OutboxStore {
    /// Inserts or replaces the record with this client id. Must be durable when it returns `Ok`.
    fn put_record(&mut self, record: &OutboxRecord) -> Result<(), StoreError>;
    /// Every record, in any order.
    fn load_records(&mut self) -> Result<Vec<OutboxRecord>, StoreError>;
}

/// Durable read-back state: the `_lastUpdated` high-water mark and the source ids already seen.
pub trait InboxStore {
    /// The saved state (empty on first start).
    fn load_inbox(&mut self) -> Result<InboxSnapshot, StoreError>;
    /// Records that `source_id` was processed at `version`.
    fn put_seen(&mut self, source_id: &str, version: &str) -> Result<(), StoreError>;
    /// Saves the high-water mark (`meta.lastUpdated` as received).
    fn put_high_water(&mut self, last_updated: &str) -> Result<(), StoreError>;
}

#[cfg(any(test, feature = "test-equipment"))]
pub use memory::MemoryStore;

#[cfg(any(test, feature = "test-equipment"))]
mod memory {
    use std::collections::BTreeMap;

    use super::{InboxSnapshot, InboxStore, OutboxRecord, OutboxStore, StoreError};
    use crate::ids::ClientId;

    /// In-memory store for tests. `fail_writes` makes every write fail.
    #[derive(Debug, Clone, Default)]
    pub struct MemoryStore {
        records: BTreeMap<ClientId, OutboxRecord>,
        seen: BTreeMap<String, String>,
        high_water: Option<String>,
        /// When true, every write returns an error (failure-path tests).
        pub fail_writes: bool,
    }

    impl MemoryStore {
        fn check(&self) -> Result<(), StoreError> {
            if self.fail_writes {
                Err(StoreError("write failed (test)".into()))
            } else {
                Ok(())
            }
        }
    }

    impl OutboxStore for MemoryStore {
        fn put_record(&mut self, record: &OutboxRecord) -> Result<(), StoreError> {
            self.check()?;
            self.records.insert(record.client_id, record.clone());
            Ok(())
        }

        fn load_records(&mut self) -> Result<Vec<OutboxRecord>, StoreError> {
            Ok(self.records.values().cloned().collect())
        }
    }

    impl InboxStore for MemoryStore {
        fn load_inbox(&mut self) -> Result<InboxSnapshot, StoreError> {
            Ok(InboxSnapshot {
                high_water: self.high_water.clone(),
                seen: self
                    .seen
                    .iter()
                    .map(|(k, v)| (k.clone(), v.clone()))
                    .collect(),
            })
        }

        fn put_seen(&mut self, source_id: &str, version: &str) -> Result<(), StoreError> {
            self.check()?;
            self.seen.insert(source_id.to_owned(), version.to_owned());
            Ok(())
        }

        fn put_high_water(&mut self, last_updated: &str) -> Result<(), StoreError> {
            self.check()?;
            self.high_water = Some(last_updated.to_owned());
            Ok(())
        }
    }
}
