//! Read-back from the medical data system with de-duplication (SR-016, F-GND-12, ICD-MDS §6).
//!
//! Polls `Observation?_lastUpdated=ge<high-water>` and turns new resources into
//! [`InboundObservation`]s. **The core must send every returned observation through the normal
//! ingest validation (plausibility, staleness, quality) and alarm evaluation, exactly like direct
//! device data, with classification `Imported`** (SR-016); this module only parses and
//! de-duplicates.
//!
//! - De-duplication by source id (`Observation/<id>`) and version: the `ge` search returns the
//!   resources at the high-water mark again, and they are counted as duplicates, not re-delivered.
//! - Conflict policy (docs/04 §3a): resources CAELUM wrote (recognised by the
//!   `urn:caelum:client-id` identifier) are never ingested — CAELUM's local record is
//!   authoritative for its own observations. Resources the medical system authored win: a new
//!   version of one is delivered as an amendment, and the core replaces its imported copy.
//! - Delivery is at least once: the seen set and the high-water mark are persisted after the
//!   observations are handed over, so a crash or store failure can only cause a re-delivery, which
//!   the core's own source-id de-duplication removes. Nothing is silently dropped: rejected
//!   resources are returned with their reason.

use std::collections::HashMap;

use caelum_domain::time::{LeapSeconds, TaiNs};
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::fhir::{
    caelum_client_id, parse_observation, source_id, version_marker, InboundObservation, Rejection,
    SourceId,
};
use crate::instant::parse_instant;
use crate::store::{InboxStore, StoreError};
use crate::transport::SearchQuery;

/// Who authored a read-back resource (docs/04 §3a conflict policy).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum Author {
    /// CAELUM wrote it: the local record wins; the read-back copy is ignored.
    Caelum,
    /// The medical system authored it: its version wins.
    MedicalSystem,
}

/// The author of a resource, by the presence of the CAELUM client-id identifier.
pub fn author(resource: &Value) -> Author {
    if caelum_client_id(resource).is_some() {
        Author::Caelum
    } else {
        Author::MedicalSystem
    }
}

/// Persisted read-back state.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct InboxSnapshot {
    /// `meta.lastUpdated` of the newest processed resource, as received.
    pub high_water: Option<String>,
    /// `(source id, version marker)` of every processed resource.
    pub seen: Vec<(String, String)>,
}

/// A resource (or component) that was not accepted.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RejectedResource {
    /// Its source id, if it has one.
    pub source_id: Option<SourceId>,
    /// Why.
    pub reason: Rejection,
}

/// The outcome of processing one search result.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct PollResult {
    /// New readings: send through ingest validation and alarm evaluation (SR-016).
    pub new: Vec<InboundObservation>,
    /// New versions of medical-system resources seen before (medical system wins): replace the
    /// imported copy, then validate and evaluate like new readings.
    pub amended: Vec<InboundObservation>,
    /// Rejected resources with reasons (to be logged for review, never silently dropped).
    pub rejected: Vec<RejectedResource>,
    /// Resources CAELUM wrote itself (ignored).
    pub own_ignored: usize,
    /// Resources already processed at the same version (ignored).
    pub duplicates: usize,
    /// A persistence failure; the readings above are still delivered (at least once).
    pub store_error: Option<StoreError>,
}

/// Inbox errors at load time.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum InboxError {
    /// The store failed.
    #[error(transparent)]
    Store(#[from] StoreError),
    /// The stored high-water mark is not a valid instant.
    #[error("stored high-water mark is invalid: {0}")]
    CorruptHighWater(String),
}

/// Read-back state: high-water mark and seen source ids.
#[derive(Debug, Clone)]
pub struct Inbox {
    high_water: Option<(String, TaiNs)>,
    seen: HashMap<String, String>,
    page_size: u32,
}

impl Inbox {
    /// Loads the persisted state. `page_size` is the search `_count`.
    pub fn load(
        store: &mut dyn InboxStore,
        leap: &LeapSeconds,
        page_size: u32,
    ) -> Result<Self, InboxError> {
        let snap = store.load_inbox()?;
        let high_water = match snap.high_water {
            Some(s) => {
                let (t, _) =
                    parse_instant(&s, leap).map_err(|_| InboxError::CorruptHighWater(s.clone()))?;
                Some((s, t))
            }
            None => None,
        };
        Ok(Self {
            high_water,
            seen: snap.seen.into_iter().collect(),
            page_size,
        })
    }

    /// The next poll's search.
    pub fn query(&self) -> SearchQuery {
        SearchQuery {
            resource_type: "Observation".into(),
            last_updated_ge: self.high_water.as_ref().map(|(s, _)| s.clone()),
            count: self.page_size,
        }
    }

    /// The high-water mark (`meta.lastUpdated` as received).
    pub fn high_water(&self) -> Option<&str> {
        self.high_water.as_ref().map(|(s, _)| s.as_str())
    }

    /// Processes one page of search results (SR-016).
    pub fn accept(
        &mut self,
        store: &mut dyn InboxStore,
        resources: &[Value],
        leap: &LeapSeconds,
    ) -> PollResult {
        let mut out = PollResult::default();
        let mut newest = self.high_water.clone();
        for res in resources {
            if let Some(lu) = res["meta"]["lastUpdated"].as_str() {
                if let Ok((t, _)) = parse_instant(lu, leap) {
                    if newest.as_ref().is_none_or(|(_, n)| t > *n) {
                        newest = Some((lu.to_owned(), t));
                    }
                }
            }
            if author(res) == Author::Caelum {
                out.own_ignored += 1;
                continue;
            }
            let Some(source) = source_id(res) else {
                out.rejected.push(RejectedResource {
                    source_id: None,
                    reason: Rejection::NoSourceId,
                });
                continue;
            };
            let version = version_marker(res).unwrap_or_default();
            let amended = match self.seen.get(&source.0) {
                Some(v) if *v == version => {
                    out.duplicates += 1;
                    continue;
                }
                Some(_) => true,
                None => false,
            };
            match parse_observation(res, leap) {
                Err(reason) => out.rejected.push(RejectedResource {
                    source_id: Some(source.clone()),
                    reason,
                }),
                Ok(parts) => {
                    for part in parts {
                        match part {
                            Ok(o) if amended => out.amended.push(o),
                            Ok(o) => out.new.push(o),
                            Err(reason) => out.rejected.push(RejectedResource {
                                source_id: Some(source.clone()),
                                reason,
                            }),
                        }
                    }
                }
            }
            if out.store_error.is_none() {
                out.store_error = store.put_seen(&source.0, &version).err();
            }
            self.seen.insert(source.0, version);
        }
        if let Some((s, t)) = newest {
            if self.high_water.as_ref().is_none_or(|(_, h)| t > *h) {
                if out.store_error.is_none() {
                    out.store_error = store.put_high_water(&s).err();
                }
                self.high_water = Some((s, t));
            }
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::fhir::{CLIENT_ID_SYSTEM, LOINC_SYSTEM, UCUM_SYSTEM};
    use crate::store::MemoryStore;
    use serde_json::json;

    fn leap() -> LeapSeconds {
        LeapSeconds::bundled().expect("table")
    }

    fn mds(id: &str, version: &str, lu: &str, hr: f64) -> Value {
        json!({ "resourceType": "Observation", "id": id,
            "meta": { "versionId": version, "lastUpdated": lu }, "status": "final",
            "code": { "coding": [{ "system": LOINC_SYSTEM, "code": "8867-4" }] },
            "valueQuantity": { "value": hr, "system": UCUM_SYSTEM, "code": "/min" },
            "effectiveDateTime": "2026-10-10T12:00:00Z" })
    }

    /// SR-016: duplicates by source id are removed, own resources ignored, amendments delivered.
    #[test]
    fn sr_016_deduplicates_by_source_id() {
        let l = leap();
        let mut store = MemoryStore::default();
        let mut inbox = Inbox::load(&mut store, &l, 50).expect("load");
        assert_eq!(inbox.query().last_updated_ge, None);
        let a = mds("1", "1", "2026-10-10T12:00:01.000Z", 60.0);
        let b = mds("2", "1", "2026-10-10T12:00:02.000Z", 61.0);
        let own = json!({ "resourceType": "Observation", "id": "3",
            "meta": { "versionId": "1", "lastUpdated": "2026-10-10T12:00:03.000Z" },
            "identifier": [{ "system": CLIENT_ID_SYSTEM,
                             "value": "67e55044-10b1-426f-9247-bb680e5fe0c8" }] });
        let r = inbox.accept(&mut store, &[a.clone(), b.clone(), own.clone()], &l);
        assert_eq!((r.new.len(), r.own_ignored, r.duplicates), (2, 1, 0));
        assert_eq!(r.store_error, None);
        assert_eq!(inbox.high_water(), Some("2026-10-10T12:00:03.000Z"));
        assert_eq!(
            inbox.query().last_updated_ge.as_deref(),
            Some("2026-10-10T12:00:03.000Z")
        );

        // The `ge` search returns the boundary again: duplicates, not new readings.
        let r = inbox.accept(&mut store, &[b.clone(), own], &l);
        assert_eq!((r.new.len(), r.duplicates, r.own_ignored), (0, 1, 1));

        // A new version of a medical-system record is an amendment (medical system wins).
        let b2 = mds("2", "2", "2026-10-10T12:00:04.000Z", 65.0);
        let r = inbox.accept(&mut store, &[b2], &l);
        assert_eq!((r.new.len(), r.amended.len()), (0, 1));
        assert_eq!(r.amended[0].value, 65.0);

        // Restart: the persisted state still de-duplicates.
        let mut reopened = Inbox::load(&mut store, &l, 50).expect("reload");
        assert_eq!(reopened.high_water(), Some("2026-10-10T12:00:04.000Z"));
        let r = reopened.accept(&mut store, &[a], &l);
        assert_eq!((r.new.len(), r.duplicates), (0, 1));
    }

    /// SR-016: rejections are reported with their source id and are not retried every poll.
    #[test]
    fn sr_016_rejections_are_reported_once() {
        let l = leap();
        let mut store = MemoryStore::default();
        let mut inbox = Inbox::load(&mut store, &l, 50).expect("load");
        let mut bad = mds("9", "1", "2026-10-10T12:00:01.000Z", 60.0);
        bad["valueQuantity"]["code"] = "furlong".into();
        let anon = json!({ "resourceType": "Observation", "status": "final" });
        let r = inbox.accept(&mut store, &[bad.clone(), anon], &l);
        assert_eq!(r.rejected.len(), 2);
        assert_eq!(
            r.rejected[0].source_id,
            Some(SourceId("Observation/9".into()))
        );
        assert_eq!(r.rejected[1].reason, Rejection::NoSourceId);
        let r = inbox.accept(&mut store, &[bad], &l);
        assert_eq!((r.rejected.len(), r.duplicates), (0, 1));
        let mut not_obs = mds("10", "1", "2026-10-10T12:00:01.000Z", 60.0);
        not_obs["resourceType"] = "Patient".into();
        let r = inbox.accept(&mut store, &[not_obs], &l);
        assert_eq!(r.rejected[0].reason, Rejection::NotAnObservation);
    }

    /// A store failure still delivers the readings (at least once) and reports the error.
    #[test]
    fn store_failure_delivers_and_reports() {
        let l = leap();
        let mut store = MemoryStore::default();
        let mut inbox = Inbox::load(&mut store, &l, 50).expect("load");
        store.fail_writes = true;
        let r = inbox.accept(
            &mut store,
            &[mds("1", "1", "2026-10-10T12:00:01.000Z", 60.0)],
            &l,
        );
        assert_eq!(r.new.len(), 1);
        assert!(r.store_error.is_some());
        // After a restart the reading is re-delivered (the core de-duplicates by source id).
        store.fail_writes = false;
        let mut reopened = Inbox::load(&mut store, &l, 50).expect("reload");
        assert_eq!(reopened.high_water(), None);
        let r = reopened.accept(
            &mut store,
            &[mds("1", "1", "2026-10-10T12:00:01.000Z", 60.0)],
            &l,
        );
        assert_eq!(r.new.len(), 1);
    }

    #[test]
    fn corrupt_high_water_is_an_error() {
        let l = leap();
        let mut store = MemoryStore::default();
        store.put_high_water("yesterday").expect("write");
        assert!(matches!(
            Inbox::load(&mut store, &l, 10),
            Err(InboxError::CorruptHighWater(_))
        ));
    }
}
