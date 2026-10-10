//! In-memory fake FHIR server for tests (test equipment: compiled only for tests or with the
//! `test-equipment` feature, never in release builds).
//!
//! Supports conditional create by the CAELUM client id (idempotent: same id ⇒ same server id, no
//! duplicate) and search by `_lastUpdated`. It can be unplugged, made to time out, return 5xx or
//! 4xx, or apply a create and then lose the response (to prove retries do not duplicate).

use caelum_domain::time::{LeapSeconds, TaiNs};
use serde_json::{json, Value};

use crate::fhir::caelum_client_id;
use crate::ids::ClientId;
use crate::instant::{format_instant, parse_instant};
use crate::transport::{CreateOutcome, FhirTransport, SearchQuery, TransportError};

/// How the fake answers.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FakeMode {
    /// Normal operation.
    Healthy,
    /// Link down: every call fails with `Unreachable`.
    Unplugged,
    /// Every call times out without being applied.
    Timeout,
    /// Every call fails with this 5xx status.
    ServerError(u16),
    /// Every create fails with this 4xx status and `OperationOutcome` text.
    ClientError(u16, String),
}

#[derive(Debug, Clone)]
struct Stored {
    resource_type: String,
    id: String,
    client_id: Option<ClientId>,
    version: u64,
    last_updated: TaiNs,
    body: Value,
}

/// The fake server. Its clock is set by the test.
#[derive(Debug)]
pub struct FakeFhirServer {
    leap: LeapSeconds,
    now: TaiNs,
    mode: FakeMode,
    lose_responses: u32,
    next_id: u64,
    stored: Vec<Stored>,
    /// Number of create requests received (including failed ones).
    pub create_calls: u32,
    /// Number of search requests received (including failed ones).
    pub search_calls: u32,
}

impl FakeFhirServer {
    /// A healthy, empty server.
    pub fn new(leap: LeapSeconds, now: TaiNs) -> Self {
        Self {
            leap,
            now,
            mode: FakeMode::Healthy,
            lose_responses: 0,
            next_id: 1,
            stored: Vec::new(),
            create_calls: 0,
            search_calls: 0,
        }
    }

    /// Sets the server clock (used for `meta.lastUpdated`).
    pub fn set_now(&mut self, now: TaiNs) {
        self.now = now;
    }

    /// Sets the failure mode.
    pub fn set_mode(&mut self, mode: FakeMode) {
        self.mode = mode;
    }

    /// The next `n` creates are applied but their responses are lost (`Timeout`).
    pub fn lose_next_responses(&mut self, n: u32) {
        self.lose_responses = n;
    }

    fn stamp(&mut self, idx: usize) {
        let lu = format_instant(self.now, &self.leap).ok();
        let s = &mut self.stored[idx];
        s.last_updated = self.now;
        if let Some(o) = s.body.as_object_mut() {
            o.insert("id".into(), s.id.clone().into());
            o.insert(
                "meta".into(),
                json!({ "versionId": s.version.to_string(), "lastUpdated": lu }),
            );
        }
    }

    fn insert(&mut self, resource_type: &str, client_id: Option<ClientId>, body: Value) -> String {
        let id = self.next_id.to_string();
        self.next_id += 1;
        self.stored.push(Stored {
            resource_type: resource_type.to_owned(),
            id: id.clone(),
            client_id,
            version: 1,
            last_updated: self.now,
            body,
        });
        self.stamp(self.stored.len() - 1);
        id
    }

    /// Adds a resource authored by the medical data system itself; returns its id.
    pub fn insert_external(&mut self, resource: Value) -> String {
        let rt = resource["resourceType"].as_str().unwrap_or("").to_owned();
        let cid = caelum_client_id(&resource);
        self.insert(&rt, cid, resource)
    }

    /// Replaces a resource's content as a new version (an amendment by the medical system).
    pub fn amend(&mut self, resource_type: &str, id: &str, resource: Value) -> bool {
        let Some(idx) = self
            .stored
            .iter()
            .position(|s| s.resource_type == resource_type && s.id == id)
        else {
            return false;
        };
        self.stored[idx].body = resource;
        self.stored[idx].version += 1;
        self.stamp(idx);
        true
    }

    /// Number of stored resources of a type.
    pub fn count(&self, resource_type: &str) -> usize {
        self.stored
            .iter()
            .filter(|s| s.resource_type == resource_type)
            .count()
    }

    /// Number of stored resources carrying this CAELUM client id.
    pub fn count_client_id(&self, client_id: ClientId) -> usize {
        self.stored
            .iter()
            .filter(|s| s.client_id == Some(client_id))
            .count()
    }

    fn fail(&self) -> Option<TransportError> {
        match &self.mode {
            FakeMode::Healthy | FakeMode::ClientError(..) => None,
            FakeMode::Unplugged => Some(TransportError::Unreachable("unplugged".into())),
            FakeMode::Timeout => Some(TransportError::Timeout),
            FakeMode::ServerError(s) => Some(TransportError::Http5xx(*s)),
        }
    }
}

impl FhirTransport for FakeFhirServer {
    fn create_conditional(
        &mut self,
        resource_type: &str,
        client_id: ClientId,
        resource: &Value,
    ) -> Result<CreateOutcome, TransportError> {
        self.create_calls += 1;
        if let Some(e) = self.fail() {
            return Err(e);
        }
        if let FakeMode::ClientError(status, outcome) = &self.mode {
            return Err(TransportError::Http4xx {
                status: *status,
                outcome: outcome.clone(),
            });
        }
        if resource["resourceType"] != resource_type {
            return Err(TransportError::Http4xx {
                status: 400,
                outcome: "resourceType does not match the endpoint".into(),
            });
        }
        let existing = self
            .stored
            .iter()
            .find(|s| s.resource_type == resource_type && s.client_id == Some(client_id))
            .map(|s| s.id.clone());
        let outcome = match existing {
            Some(server_id) => CreateOutcome {
                server_id,
                created: false,
            },
            None => CreateOutcome {
                server_id: self.insert(resource_type, Some(client_id), resource.clone()),
                created: true,
            },
        };
        if self.lose_responses > 0 {
            self.lose_responses -= 1;
            return Err(TransportError::Timeout);
        }
        Ok(outcome)
    }

    fn search(&mut self, query: &SearchQuery) -> Result<Vec<Value>, TransportError> {
        self.search_calls += 1;
        if let Some(e) = self.fail() {
            return Err(e);
        }
        let since = match &query.last_updated_ge {
            Some(s) => Some(
                parse_instant(s, &self.leap)
                    .map_err(|e| TransportError::Http4xx {
                        status: 400,
                        outcome: e.to_string(),
                    })?
                    .0,
            ),
            None => None,
        };
        let mut hits: Vec<&Stored> = self
            .stored
            .iter()
            .filter(|s| s.resource_type == query.resource_type)
            .filter(|s| since.is_none_or(|t| s.last_updated >= t))
            .collect();
        hits.sort_by(|a, b| (a.last_updated, &a.id).cmp(&(b.last_updated, &b.id)));
        Ok(hits
            .into_iter()
            .take(query.count as usize)
            .map(|s| s.body.clone())
            .collect())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ids::ResourceKind;
    use crate::instant::tai_from_unix_s;

    fn server() -> FakeFhirServer {
        let leap = LeapSeconds::bundled().expect("table");
        let now = tai_from_unix_s(1_791_633_600, &leap).expect("time");
        FakeFhirServer::new(leap, now)
    }

    fn obs(id: ClientId) -> Value {
        json!({ "resourceType": "Observation",
                "identifier": [{ "system": crate::fhir::CLIENT_ID_SYSTEM, "value": id.to_string() }] })
    }

    /// ICD-MDS §4: conditional create is idempotent per client id, even after a lost response.
    #[test]
    fn icd_mds_4_conditional_create_is_idempotent() {
        let mut s = server();
        let id = ClientId::derive(ResourceKind::Observation, "a");
        let first = s
            .create_conditional("Observation", id, &obs(id))
            .expect("created");
        assert!(first.created);
        let again = s
            .create_conditional("Observation", id, &obs(id))
            .expect("exists");
        assert_eq!((again.server_id, again.created), (first.server_id, false));
        let id2 = ClientId::derive(ResourceKind::Observation, "b");
        s.lose_next_responses(1);
        assert_eq!(
            s.create_conditional("Observation", id2, &obs(id2)),
            Err(TransportError::Timeout)
        );
        assert!(
            !s.create_conditional("Observation", id2, &obs(id2))
                .expect("exists")
                .created
        );
        assert_eq!((s.count("Observation"), s.count_client_id(id2)), (2, 1));
        assert!(s.create_conditional("Device", id, &obs(id)).is_err());
    }

    #[test]
    fn failure_modes_and_search() {
        let mut s = server();
        let id = ClientId::derive(ResourceKind::Observation, "a");
        for (mode, want) in [
            (
                FakeMode::Unplugged,
                TransportError::Unreachable("unplugged".into()),
            ),
            (FakeMode::Timeout, TransportError::Timeout),
            (FakeMode::ServerError(503), TransportError::Http5xx(503)),
            (
                FakeMode::ClientError(422, "bad".into()),
                TransportError::Http4xx {
                    status: 422,
                    outcome: "bad".into(),
                },
            ),
        ] {
            s.set_mode(mode);
            assert_eq!(s.create_conditional("Observation", id, &obs(id)), Err(want));
        }
        assert_eq!(s.count("Observation"), 0);
        s.set_mode(FakeMode::Healthy);
        let ext = s.insert_external(json!({ "resourceType": "Observation", "status": "final" }));
        let later = TaiNs(s.now.0 + 1_000_000_000);
        s.set_now(later);
        s.create_conditional("Observation", id, &obs(id))
            .expect("created");
        let q = |ge: Option<String>| SearchQuery {
            resource_type: "Observation".into(),
            last_updated_ge: ge,
            count: 10,
        };
        assert_eq!(s.search(&q(None)).expect("ok").len(), 2);
        let since = format_instant(later, &s.leap).expect("fmt");
        let hits = s.search(&q(Some(since))).expect("ok");
        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0]["meta"]["versionId"], "1");
        assert!(s.amend(
            "Observation",
            &ext,
            json!({ "resourceType": "Observation" })
        ));
        assert!(!s.amend("Observation", "999", json!({})));
        assert!(s.search(&q(Some("garbage".into()))).is_err());
        s.set_mode(FakeMode::Unplugged);
        assert!(s.search(&q(None)).is_err());
    }
}
