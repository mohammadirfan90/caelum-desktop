//! Onboard medical data system (MDA-class) connector: HL7 FHIR R4 resources, outbox / inbox
//! synchronisation, read-back with de-duplication, health monitoring and routing modes A / A' / B
//! (M10; docs/03 §1, docs/04 §3a, `docs/icd/ICD-MDS.md`).
//!
//! Requirements implemented:
//! - F-GND-12, R-INT-05: two-way FHIR R4 read/write ([`fhir`], [`outbox`], [`inbox`]), sync
//!   state and connection health ([`health`], [`connector`]).
//! - F-GND-13, SR-015, R-INT-06: dual outbound routing ([`routing`]): alarm events and ground
//!   escalations always take path B (direct to the vehicle, OPS channel) whatever the state of
//!   the medical data system; the medical record path is A, or A' / hold per mission when the
//!   medical data system is not usable.
//! - F-GND-14: summary-first data policy (raw waveforms only when event-linked or requested).
//! - SR-016: read-back observations are de-duplicated by source id and must pass through the
//!   normal ingest validation and alarm evaluation.
//! - R-INT-07: records are kept in a persistent outbox while the medical data system is
//!   unavailable and delivered exactly once on reconnect (idempotent client ids, [`ids`]).
//!
//! The crate is sans-I/O: time is injected as TAI ([`caelum_domain::time::TaiNs`]) and the FHIR
//! transport is a trait ([`transport::FhirTransport`]). The HTTPS transport is supplied by the
//! core (CR-016). The interface is create/search only: no command reaches the medical data
//! system (ICD-MDS §7). `fake` and `store::MemoryStore` are test equipment, compiled only for
//! tests or with the `test-equipment` feature.

pub mod connector;
pub mod fhir;
pub mod health;
pub mod ids;
pub mod inbox;
pub mod instant;
pub mod outbox;
pub mod routing;
pub mod store;
pub mod transport;

#[cfg(any(test, feature = "test-equipment"))]
pub mod fake;
