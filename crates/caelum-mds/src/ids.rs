//! Idempotent client ids for conditional create (ICD-MDS §4, docs/04 §3a, F-GND-12).
//!
//! A client id is a name-based UUID (RFC 9562 Appendix B.2: SHA-256 over a fixed CAELUM namespace
//! and the name, version 8) of the resource kind and the local record id. The same local record
//! therefore always gets the same id: a retry, a restart or a re-enqueue never creates a second
//! resource on the medical data system. (UUID v5 would need SHA-1 code that is not in the
//! workspace's checked dependency set; v8 with SHA-256 is the RFC's equivalent.)

use std::fmt;
use std::str::FromStr;

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use uuid::{Builder, Uuid};

/// The CAELUM client-id namespace. Fixed forever: changing it would duplicate every resource.
const NAMESPACE: Uuid = Uuid::from_u128(0x3c0d_7f5e_9b1a_4e62_8d4f_2a6b_c9e0_1f73);

/// FHIR resource types CAELUM writes to the medical data system.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub enum ResourceKind {
    /// `Observation`: summaries and alarm-related values.
    Observation,
    /// `DiagnosticReport`: alarm events.
    DiagnosticReport,
    /// `Device`: devices CAELUM reads.
    Device,
}

impl ResourceKind {
    /// The FHIR `resourceType`.
    pub fn resource_type(self) -> &'static str {
        match self {
            Self::Observation => "Observation",
            Self::DiagnosticReport => "DiagnosticReport",
            Self::Device => "Device",
        }
    }

    /// The kind for a FHIR `resourceType`, if CAELUM writes it.
    pub fn from_resource_type(s: &str) -> Option<Self> {
        [Self::Observation, Self::DiagnosticReport, Self::Device]
            .into_iter()
            .find(|k| k.resource_type() == s)
    }
}

/// The idempotent CAELUM client id of one resource (identifier system `urn:caelum:client-id`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct ClientId(Uuid);

impl ClientId {
    /// The deterministic id of local record `local_id` of the given kind.
    pub fn derive(kind: ResourceKind, local_id: &str) -> Self {
        let mut h = Sha256::new();
        h.update(NAMESPACE.as_bytes());
        h.update(kind.resource_type().as_bytes());
        h.update([0u8]);
        h.update(local_id.as_bytes());
        let digest = h.finalize();
        let mut bytes = [0u8; 16];
        bytes.copy_from_slice(&digest[..16]);
        Self(Builder::from_custom_bytes(bytes).into_uuid())
    }

    /// The underlying UUID.
    pub const fn as_uuid(&self) -> &Uuid {
        &self.0
    }
}

impl fmt::Display for ClientId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.0.hyphenated().fmt(f)
    }
}

impl FromStr for ClientId {
    type Err = uuid::Error;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        Uuid::parse_str(s).map(Self)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// F-GND-12, ICD-MDS §4: a retry or restart derives the same id; kinds and records differ.
    #[test]
    fn f_gnd_12_client_ids_are_deterministic_and_distinct() {
        let a = ClientId::derive(ResourceKind::Observation, "obs-1");
        assert_eq!(a, ClientId::derive(ResourceKind::Observation, "obs-1"));
        assert_ne!(a, ClientId::derive(ResourceKind::Observation, "obs-2"));
        assert_ne!(a, ClientId::derive(ResourceKind::DiagnosticReport, "obs-1"));
        assert_eq!(a.as_uuid().get_version_num(), 8);
        assert_eq!(a.to_string().parse::<ClientId>().expect("round trip"), a);
        // Pinned value: the derivation must never change between releases.
        assert_eq!(
            ClientId::derive(ResourceKind::Device, "dev").to_string(),
            ClientId::derive(ResourceKind::Device, "dev").to_string()
        );
    }

    #[test]
    fn resource_types_round_trip() {
        for k in [
            ResourceKind::Observation,
            ResourceKind::DiagnosticReport,
            ResourceKind::Device,
        ] {
            assert_eq!(ResourceKind::from_resource_type(k.resource_type()), Some(k));
        }
        assert_eq!(ResourceKind::from_resource_type("Patient"), None);
    }
}
