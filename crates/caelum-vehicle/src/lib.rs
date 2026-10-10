//! cFS Software Bus bridge, subscribe only, and environment and radiation normalisation.
//!
//! This crate has no write API (SR-011, R-SEC-04, CLAUDE.md rule 5): it receives cFS telemetry
//! datagrams and decodes them; there is no function anywhere in it that sends, writes, connects
//! or commands. CAELUM status telemetry is published by caelum-ground, not here. The CI write
//! guard (`tools/ci`) scans this crate with no exceptions, and `tests/no_send_api.rs` checks the
//! same over the source.
//!
//! Layers:
//! - [`config`]: the signed MID table (kind `vehicle`), parsed and validated (CR-015 §3);
//! - [`packet`]: SBN framing, CCSDS primary header and cFS secondary header (sans-I/O);
//! - [`time`]: vehicle time → TAI, MET from the signed mission epoch, clock correlation;
//! - [`decode`]: MID → readings normalised to the parameter catalog (sans-I/O);
//! - [`receiver`]: the thin tokio UDP receiver.
//!
//! Implemented in M9: F-ENV-01, F-ENV-02, F-ENV-03, F-GND-11 (subscribe side), R-INT-04,
//! F-ACQ-11 (vehicle time correlation). Integration: docs/integration/M9.md.

pub mod config;
pub mod decode;
pub mod packet;
pub mod receiver;
pub mod spe;
pub mod time;

pub use config::{ConfigError, VehicleConfig};
pub use decode::{Decoded, Decoder, Rejection, Stats, Subject, VehicleReading, VehicleValue};
pub use receiver::{RecvError, VehicleReceiver};
pub use spe::SPE_STATES;
pub use time::{ClockState, CorrelationStatus, TimeSource};

/// Source label for observations from this crate (store column / provenance `source`).
pub const SOURCE: &str = "vehicle";
