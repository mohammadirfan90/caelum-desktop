//! Ground integration: CCSDS Space Packets, CFDP, XTCE generator, Yamcs link, store-and-forward
//! downlink queue, uplink verifier, FHIR export, de-identified research export and the path B
//! status publisher (telemetry-only packet type).
//!
//! Implemented in M10 (CR-016): F-GND-01..03, F-GND-05..07, F-GND-09, F-GND-11 (publish side),
//! F-GND-13 (path B), F-GND-14 (summaries), F-SYS-08 (FHIR and research export), R-SEC-06,
//! SR-011, SR-015.
//!
//! Every module is sans-I/O except [`publisher`] and [`yamcs`], which own sockets. Time is always
//! passed in as TAI ([`caelum_domain::time::TaiNs`]); nothing reads the system clock.

pub mod cfdp;
pub mod config;
pub mod fhir;
pub mod medical;
pub mod ops;
pub mod packet;
pub mod publisher;
pub mod queue;
pub mod research;
pub mod uplink;
pub mod xtce;
pub mod yamcs;
