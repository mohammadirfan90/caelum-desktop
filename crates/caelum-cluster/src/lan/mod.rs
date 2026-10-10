//! The crew LAN service (F-SYS-05, CLAUDE.md rule 6, CR-017).
//!
//! Every connection is mutual TLS 1.3. A client must present a certificate issued by the crew-LAN
//! certificate authority **and** listed in the provisioned [`StationRegistry`] (pinned by the
//! SHA-256 fingerprint of the certificate); otherwise the connection is dropped before any HTTP
//! is read. The certificate gives the station identity and kind; user requests from a secondary
//! station also carry the user's session token, issued by this service after the user signs in
//! from that station. Every endpoint is checked against [`access::Endpoint::access`] before it
//! runs; physiology leaves only through role- and scope-checked projections ([`access::project`]).

pub mod access;
pub mod client;
pub mod server;
pub mod sessions;
pub mod tls;

use std::collections::HashMap;

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::fence::NodeId;

/// SHA-256 of a certificate's DER encoding.
pub type Fingerprint = [u8; 32];

/// Fingerprint of a DER certificate.
pub fn fingerprint(cert_der: &[u8]) -> Fingerprint {
    Sha256::digest(cert_der).into()
}

/// What a station certificate is for.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum StationKind {
    /// The other CAELUM instance of the fail-over pair: replication and status only.
    Peer,
    /// A crew laptop or tablet used as a secondary display; users sign in on it.
    Secondary,
    /// A wall-mounted crew display (F-SYS-13): the crew-wide emergency view without sign-in
    /// (SR-014), audited.
    Kiosk,
}

/// A provisioned station.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct StationIdentity {
    /// Station ID (for a peer: its node ID).
    pub station_id: NodeId,
    /// Kind.
    pub kind: StationKind,
}

/// The provisioned stations by certificate fingerprint (from the signed configuration; CR-017 §4).
#[derive(Debug, Clone, Default)]
pub struct StationRegistry(HashMap<Fingerprint, StationIdentity>);

impl StationRegistry {
    /// An empty registry: refuses every client.
    pub fn new() -> Self {
        Self::default()
    }

    /// Registers the station whose certificate has `fingerprint`.
    pub fn insert(&mut self, fingerprint: Fingerprint, station: StationIdentity) {
        self.0.insert(fingerprint, station);
    }

    /// The station presenting the certificate `cert_der`, if provisioned.
    pub fn identify(&self, cert_der: &[u8]) -> Option<&StationIdentity> {
        self.0.get(&fingerprint(cert_der))
    }

    /// The station with this fingerprint, if provisioned.
    pub fn get(&self, fingerprint: &Fingerprint) -> Option<&StationIdentity> {
        self.0.get(fingerprint)
    }
}
