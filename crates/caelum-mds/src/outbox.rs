//! Per-resource outbox (F-GND-12, ICD-MDS §6 and §9, docs/04 §3a, R-INT-07).
//!
//! Each record has the idempotent [`ClientId`] of its local record. States:
//! `Pending → InFlight → Acknowledged` (server id kept) or `HeldForReview` (4xx: not retried,
//! reason kept until a person releases it); a 5xx, timeout, transport or protocol failure returns
//! the record to `Pending` with deterministic exponential back-off. Every transition is persisted
//! through [`OutboxStore`] *before* it takes effect in memory, so the outbox survives a restart; an
//! `InFlight` record found at start-up is retried, which is safe because the create is
//! conditional on the client id. Nothing is ever dropped while the medical system is unreachable.
//!
//! Order: alarm-related records before summaries (ICD-MDS §6: alarm-related within 5 s), then by
//! creation time.
//!
//! Conflict policy (docs/04 §3a): CAELUM's own resources are authoritative in the local record; a
//! create that finds the client id already present (200) is an acknowledgement, never an
//! overwrite in either direction. Records the medical system authored win (see `crate::inbox`).

use std::collections::BTreeMap;

use caelum_domain::time::TaiNs;
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::fhir::caelum_client_id;
use crate::ids::{ClientId, ResourceKind};
use crate::store::{OutboxStore, StoreError};
use crate::transport::{CreateOutcome, TransportError};

/// Send priority. `Alarm` sorts first.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub enum Priority {
    /// Alarm events and alarm-related values (ICD-MDS §6: within 5 s).
    Alarm,
    /// Periodic summaries, devices and everything else.
    Routine,
}

/// Where a record is in its life cycle.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum RecordState {
    /// Waiting to be sent (first time or after a retryable failure).
    Pending,
    /// Sent; no answer recorded yet. Reset to `Pending` on restart.
    InFlight,
    /// Stored by the medical system.
    Acknowledged {
        /// Server logical id.
        server_id: String,
        /// When the acknowledgement was received.
        at: TaiNs,
    },
    /// Rejected with 4xx; kept, not retried, until released after review (ICD-MDS §9).
    HeldForReview {
        /// HTTP status.
        status: u16,
        /// `OperationOutcome` text.
        reason: String,
        /// When it was rejected.
        at: TaiNs,
    },
}

/// One outbox record.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct OutboxRecord {
    /// Idempotent client id (primary key).
    pub client_id: ClientId,
    /// Resource kind.
    pub kind: ResourceKind,
    /// Send priority.
    pub priority: Priority,
    /// The FHIR resource, with the client-id identifier.
    pub resource: Value,
    /// When it was enqueued.
    pub created_at: TaiNs,
    /// Life-cycle state.
    pub state: RecordState,
    /// Send attempts so far.
    pub attempts: u32,
    /// Earliest time of the next attempt (meaningful while `Pending`).
    pub next_attempt_at: TaiNs,
    /// The last failure, if any.
    pub last_error: Option<String>,
}

/// Deterministic exponential back-off: `base · 2^(attempts−1)`, capped at `max` (no jitter).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct Backoff {
    /// First retry delay in nanoseconds.
    pub base_ns: i64,
    /// Longest delay in nanoseconds.
    pub max_ns: i64,
}

impl Backoff {
    /// Delay before the next attempt after `attempts` failed attempts (`attempts ≥ 1`).
    pub fn delay_ns(&self, attempts: u32) -> i64 {
        let shift = attempts.saturating_sub(1).min(62);
        self.base_ns
            .max(0)
            .saturating_mul(1i64 << shift)
            .min(self.max_ns.max(0))
    }
}

/// Outbox configuration (from signed mission configuration; no built-in defaults).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct OutboxConfig {
    /// Back-off for alarm-priority records.
    pub alarm: Backoff,
    /// Back-off for routine records.
    pub routine: Backoff,
}

/// Outbox errors.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum OutboxError {
    /// Persistence failed; the in-memory state is unchanged.
    #[error(transparent)]
    Store(#[from] StoreError),
    /// The resource's `resourceType` does not match the kind.
    #[error("resource type does not match {0:?}")]
    KindMismatch(ResourceKind),
    /// The resource does not carry the record's client id identifier.
    #[error("resource lacks the urn:caelum:client-id identifier {0}")]
    MissingClientId(ClientId),
    /// No record with this client id.
    #[error("no outbox record {0}")]
    UnknownRecord(ClientId),
    /// The record is not in the state the operation needs.
    #[error("record {0} is not in the required state")]
    WrongState(ClientId),
}

/// A new outbox entry.
#[derive(Debug, Clone, PartialEq)]
pub struct OutboxItem {
    /// Client id (from [`ClientId::derive`]) also present in `resource.identifier`.
    pub client_id: ClientId,
    /// Resource kind.
    pub kind: ResourceKind,
    /// Priority.
    pub priority: Priority,
    /// The FHIR resource.
    pub resource: Value,
}

/// Result of [`Outbox::enqueue`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EnqueueOutcome {
    /// Added.
    Queued,
    /// A record with this client id exists already; nothing changed (no duplicate).
    AlreadyQueued,
}

/// What happened to a record after an attempt.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Disposition {
    /// Stored by the medical system.
    Acknowledged,
    /// 4xx; held for review.
    HeldForReview,
    /// Retry at this time.
    RetryAt(TaiNs),
}

/// The outbox: an in-memory index over the persisted records.
#[derive(Debug, Clone)]
pub struct Outbox {
    config: OutboxConfig,
    records: BTreeMap<ClientId, OutboxRecord>,
}

impl Outbox {
    /// Loads the outbox, returning `InFlight` records to `Pending` (their outcome is unknown).
    pub fn load(store: &mut dyn OutboxStore, config: OutboxConfig) -> Result<Self, OutboxError> {
        let records = store
            .load_records()?
            .into_iter()
            .map(|r| (r.client_id, r))
            .collect();
        let mut outbox = Self { config, records };
        outbox.recover_in_flight(store)?;
        Ok(outbox)
    }

    /// Returns every `InFlight` record to `Pending` (persisted first). Used at start-up and after a
    /// store failure left a record in flight, so no record is ever stuck unsent (R-INT-07).
    /// Returns how many records were recovered.
    pub fn recover_in_flight(&mut self, store: &mut dyn OutboxStore) -> Result<usize, OutboxError> {
        let stuck: Vec<ClientId> = self
            .records
            .values()
            .filter(|r| r.state == RecordState::InFlight)
            .map(|r| r.client_id)
            .collect();
        for &id in &stuck {
            self.update(store, id, |r| {
                r.state = RecordState::Pending;
                Ok(())
            })?;
        }
        Ok(stuck.len())
    }

    /// Adds a record (persisted first). Re-enqueueing the same client id changes nothing.
    pub fn enqueue(
        &mut self,
        store: &mut dyn OutboxStore,
        item: OutboxItem,
        now: TaiNs,
    ) -> Result<EnqueueOutcome, OutboxError> {
        if item.resource["resourceType"] != item.kind.resource_type() {
            return Err(OutboxError::KindMismatch(item.kind));
        }
        if caelum_client_id(&item.resource) != Some(item.client_id) {
            return Err(OutboxError::MissingClientId(item.client_id));
        }
        if self.records.contains_key(&item.client_id) {
            return Ok(EnqueueOutcome::AlreadyQueued);
        }
        let record = OutboxRecord {
            client_id: item.client_id,
            kind: item.kind,
            priority: item.priority,
            resource: item.resource,
            created_at: now,
            state: RecordState::Pending,
            attempts: 0,
            next_attempt_at: now,
            last_error: None,
        };
        store.put_record(&record)?;
        self.records.insert(record.client_id, record);
        Ok(EnqueueOutcome::Queued)
    }

    /// Pending records due at `now`, alarm priority first, then oldest first.
    pub fn due(&self, now: TaiNs) -> Vec<ClientId> {
        let mut due: Vec<&OutboxRecord> = self
            .records
            .values()
            .filter(|r| r.state == RecordState::Pending && r.next_attempt_at <= now)
            .collect();
        due.sort_by_key(|r| (r.priority, r.created_at, r.client_id));
        due.into_iter().map(|r| r.client_id).collect()
    }

    /// Earliest next attempt among pending records.
    pub fn next_due_at(&self) -> Option<TaiNs> {
        self.records
            .values()
            .filter(|r| r.state == RecordState::Pending)
            .map(|r| r.next_attempt_at)
            .min()
    }

    fn update(
        &mut self,
        store: &mut dyn OutboxStore,
        id: ClientId,
        f: impl FnOnce(&mut OutboxRecord) -> Result<(), OutboxError>,
    ) -> Result<&OutboxRecord, OutboxError> {
        let mut r = self
            .records
            .get(&id)
            .cloned()
            .ok_or(OutboxError::UnknownRecord(id))?;
        f(&mut r)?;
        store.put_record(&r)?;
        self.records.insert(id, r);
        self.records.get(&id).ok_or(OutboxError::UnknownRecord(id))
    }

    /// Marks a pending record in flight (persisted before the request is sent).
    pub fn mark_in_flight(
        &mut self,
        store: &mut dyn OutboxStore,
        id: ClientId,
    ) -> Result<&OutboxRecord, OutboxError> {
        self.update(store, id, |r| {
            if r.state != RecordState::Pending {
                return Err(OutboxError::WrongState(id));
            }
            r.state = RecordState::InFlight;
            r.attempts += 1;
            Ok(())
        })
    }

    /// Applies the result of the request for an in-flight record.
    pub fn apply(
        &mut self,
        store: &mut dyn OutboxStore,
        id: ClientId,
        result: &Result<CreateOutcome, TransportError>,
        now: TaiNs,
    ) -> Result<Disposition, OutboxError> {
        let config = self.config;
        let mut disposition = Disposition::Acknowledged;
        self.update(store, id, |r| {
            if r.state != RecordState::InFlight {
                return Err(OutboxError::WrongState(id));
            }
            match result {
                Ok(o) => {
                    r.state = RecordState::Acknowledged {
                        server_id: o.server_id.clone(),
                        at: now,
                    };
                    r.last_error = None;
                }
                Err(e @ TransportError::Http4xx { status, outcome }) if !e.is_retryable() => {
                    r.state = RecordState::HeldForReview {
                        status: *status,
                        reason: outcome.clone(),
                        at: now,
                    };
                    r.last_error = Some(format!("HTTP {status}: {outcome}"));
                    disposition = Disposition::HeldForReview;
                }
                Err(e) => {
                    let backoff = match r.priority {
                        Priority::Alarm => config.alarm,
                        Priority::Routine => config.routine,
                    };
                    r.state = RecordState::Pending;
                    r.next_attempt_at = TaiNs(now.0.saturating_add(backoff.delay_ns(r.attempts)));
                    r.last_error = Some(e.to_string());
                    disposition = Disposition::RetryAt(r.next_attempt_at);
                }
            }
            Ok(())
        })?;
        Ok(disposition)
    }

    /// Returns a held record to `Pending` after review (e.g. once the cause was fixed).
    pub fn release_held(
        &mut self,
        store: &mut dyn OutboxStore,
        id: ClientId,
        now: TaiNs,
    ) -> Result<(), OutboxError> {
        self.update(store, id, |r| {
            if !matches!(r.state, RecordState::HeldForReview { .. }) {
                return Err(OutboxError::WrongState(id));
            }
            r.state = RecordState::Pending;
            r.attempts = 0;
            r.next_attempt_at = now;
            Ok(())
        })
        .map(|_| ())
    }

    /// A record by client id.
    pub fn get(&self, id: ClientId) -> Option<&OutboxRecord> {
        self.records.get(&id)
    }

    /// Every record, by client id.
    pub fn records(&self) -> impl Iterator<Item = &OutboxRecord> {
        self.records.values()
    }

    /// Records not yet acknowledged or held (pending or in flight).
    pub fn unsent_count(&self) -> usize {
        self.records
            .values()
            .filter(|r| matches!(r.state, RecordState::Pending | RecordState::InFlight))
            .count()
    }

    /// Records held for review.
    pub fn held(&self) -> impl Iterator<Item = &OutboxRecord> {
        self.records
            .values()
            .filter(|r| matches!(r.state, RecordState::HeldForReview { .. }))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::fhir::CLIENT_ID_SYSTEM;
    use crate::store::MemoryStore;
    use serde_json::json;

    const S: i64 = 1_000_000_000;

    fn config() -> OutboxConfig {
        OutboxConfig {
            alarm: Backoff {
                base_ns: S,
                max_ns: 4 * S,
            },
            routine: Backoff {
                base_ns: 10 * S,
                max_ns: 300 * S,
            },
        }
    }

    fn item(local: &str, priority: Priority) -> OutboxItem {
        let client_id = ClientId::derive(ResourceKind::Observation, local);
        OutboxItem {
            client_id,
            kind: ResourceKind::Observation,
            priority,
            resource: json!({ "resourceType": "Observation",
                "identifier": [{ "system": CLIENT_ID_SYSTEM, "value": client_id.to_string() }] }),
        }
    }

    fn ack(id: &str) -> Result<CreateOutcome, TransportError> {
        Ok(CreateOutcome {
            server_id: id.into(),
            created: true,
        })
    }

    #[test]
    fn backoff_is_exponential_capped_and_deterministic() {
        let b = Backoff {
            base_ns: 2,
            max_ns: 50,
        };
        let d: Vec<i64> = (1..=7).map(|a| b.delay_ns(a)).collect();
        assert_eq!(d, [2, 4, 8, 16, 32, 50, 50]);
        assert_eq!(b.delay_ns(u32::MAX), 50);
        assert_eq!(b.delay_ns(0), 2);
    }

    /// F-GND-12, ICD-MDS §6: alarm records go first; re-enqueue does not duplicate.
    #[test]
    fn f_gnd_12_priority_order_and_idempotent_enqueue() {
        let mut store = MemoryStore::default();
        let mut ob = Outbox::load(&mut store, config()).expect("load");
        let t = TaiNs(100 * S);
        let r1 = item("summary-1", Priority::Routine);
        let a1 = item("alarm-1", Priority::Alarm);
        assert_eq!(
            ob.enqueue(&mut store, r1.clone(), t),
            Ok(EnqueueOutcome::Queued)
        );
        assert_eq!(
            ob.enqueue(&mut store, a1.clone(), TaiNs(t.0 + 1)),
            Ok(EnqueueOutcome::Queued)
        );
        assert_eq!(
            ob.enqueue(&mut store, r1.clone(), t),
            Ok(EnqueueOutcome::AlreadyQueued)
        );
        assert_eq!(ob.due(TaiNs(t.0 + 1)), [a1.client_id, r1.client_id]);
        assert_eq!(ob.due(TaiNs(t.0)), [r1.client_id]);
        assert_eq!(ob.records().count(), 2);
    }

    /// ICD-MDS §9: retry with back-off on 5xx/timeout/unreachable; hold on 4xx; ack stores id.
    #[test]
    fn icd_mds_9_state_machine() {
        let mut store = MemoryStore::default();
        let mut ob = Outbox::load(&mut store, config()).expect("load");
        let t = TaiNs(100 * S);
        let a = item("alarm", Priority::Alarm);
        let r = item("routine", Priority::Routine);
        ob.enqueue(&mut store, a.clone(), t).expect("enqueue");
        ob.enqueue(&mut store, r.clone(), t).expect("enqueue");

        ob.mark_in_flight(&mut store, a.client_id)
            .expect("in flight");
        assert_eq!(
            ob.mark_in_flight(&mut store, a.client_id).err(),
            Some(OutboxError::WrongState(a.client_id))
        );
        let d = ob
            .apply(
                &mut store,
                a.client_id,
                &Err(TransportError::Http5xx(503)),
                t,
            )
            .expect("apply");
        assert_eq!(d, Disposition::RetryAt(TaiNs(t.0 + S)));
        ob.mark_in_flight(&mut store, a.client_id)
            .expect("in flight");
        let d = ob
            .apply(&mut store, a.client_id, &Err(TransportError::Timeout), t)
            .expect("apply");
        assert_eq!(d, Disposition::RetryAt(TaiNs(t.0 + 2 * S)));
        assert_eq!(ob.next_due_at(), Some(t), "routine still due now");
        ob.mark_in_flight(&mut store, a.client_id)
            .expect("in flight");
        assert_eq!(
            ob.apply(&mut store, a.client_id, &ack("srv-1"), t),
            Ok(Disposition::Acknowledged)
        );
        assert!(matches!(&ob.get(a.client_id).expect("kept").state,
            RecordState::Acknowledged { server_id, .. } if server_id == "srv-1"));
        assert_eq!(
            ob.apply(&mut store, a.client_id, &ack("x"), t),
            Err(OutboxError::WrongState(a.client_id))
        );

        ob.mark_in_flight(&mut store, r.client_id)
            .expect("in flight");
        let rejected = Err(TransportError::Http4xx {
            status: 422,
            outcome: "profile".into(),
        });
        assert_eq!(
            ob.apply(&mut store, r.client_id, &rejected, t),
            Ok(Disposition::HeldForReview)
        );
        assert!(
            ob.due(TaiNs(t.0 + 1_000 * S)).is_empty(),
            "held records are not retried"
        );
        assert_eq!(ob.held().count(), 1);
        assert_eq!(ob.unsent_count(), 0);
        ob.mark_in_flight(&mut store, r.client_id)
            .expect_err("held is not pending");
        ob.release_held(&mut store, r.client_id, t)
            .expect("release");
        assert_eq!(
            ob.release_held(&mut store, r.client_id, t),
            Err(OutboxError::WrongState(r.client_id))
        );
        ob.mark_in_flight(&mut store, r.client_id)
            .expect("in flight");
        let throttled = Err(TransportError::Http4xx {
            status: 429,
            outcome: String::new(),
        });
        assert_eq!(
            ob.apply(&mut store, r.client_id, &throttled, t),
            Ok(Disposition::RetryAt(TaiNs(t.0 + 10 * S)))
        );
        let unknown = ClientId::derive(ResourceKind::Device, "nope");
        assert_eq!(
            ob.mark_in_flight(&mut store, unknown).err(),
            Some(OutboxError::UnknownRecord(unknown))
        );
    }

    #[test]
    fn enqueue_validates_the_resource() {
        let mut store = MemoryStore::default();
        let mut ob = Outbox::load(&mut store, config()).expect("load");
        let mut wrong_kind = item("a", Priority::Routine);
        wrong_kind.kind = ResourceKind::Device;
        assert_eq!(
            ob.enqueue(&mut store, wrong_kind, TaiNs(0)),
            Err(OutboxError::KindMismatch(ResourceKind::Device))
        );
        let mut no_id = item("b", Priority::Routine);
        no_id.resource = json!({ "resourceType": "Observation" });
        assert_eq!(
            ob.enqueue(&mut store, no_id.clone(), TaiNs(0)),
            Err(OutboxError::MissingClientId(no_id.client_id))
        );
    }

    /// R-INT-07: a failed write leaves memory unchanged; a restart resumes in-flight records.
    #[test]
    fn r_int_07_persistence_and_restart() {
        let mut store = MemoryStore::default();
        let mut ob = Outbox::load(&mut store, config()).expect("load");
        let a = item("a", Priority::Alarm);
        store.fail_writes = true;
        assert!(matches!(
            ob.enqueue(&mut store, a.clone(), TaiNs(0)),
            Err(OutboxError::Store(_))
        ));
        assert_eq!(ob.records().count(), 0);
        store.fail_writes = false;
        ob.enqueue(&mut store, a.clone(), TaiNs(0))
            .expect("enqueue");
        ob.mark_in_flight(&mut store, a.client_id)
            .expect("in flight");
        store.fail_writes = true;
        assert!(ob
            .apply(&mut store, a.client_id, &ack("1"), TaiNs(0))
            .is_err());
        assert_eq!(
            ob.get(a.client_id).expect("kept").state,
            RecordState::InFlight
        );
        store.fail_writes = false;
        // Crash with the record in flight: on restart it is pending again and due.
        let reopened = Outbox::load(&mut store, config()).expect("reload");
        assert_eq!(
            reopened.get(a.client_id).expect("kept").state,
            RecordState::Pending
        );
        assert_eq!(reopened.get(a.client_id).expect("kept").attempts, 1);
        assert_eq!(reopened.due(TaiNs(0)), [a.client_id]);
        // Without a restart, the stuck record is recovered in place.
        assert_eq!(ob.recover_in_flight(&mut store), Ok(1));
        assert_eq!(ob.due(TaiNs(0)), [a.client_id]);
        assert_eq!(ob.recover_in_flight(&mut store), Ok(0));
    }
}
