//! Primary/standby fail-over, signed event-log replication and the crew-station LAN service.
//!
//! Implemented in M11: F-SYS-04 (fail-over, ≤ 10 s takeover, no acknowledged data loss), F-SYS-05
//! (crew-station LAN service), R-OPS-02 (standby takeover within 10 s without losing acknowledged
//! data). Design and dependency decisions: `docs/change-requests/CR-017.md`; what the core wires:
//! `docs/integration/M11.md`.
//!
//! Layers, from pure to networked:
//! - [`fence`], [`timing`], [`failover`]: the sans-I/O fail-over state machine (heartbeat,
//!   missed-heartbeat detection, promotion with epoch fencing). Time is passed in, so tests drive
//!   it with a deterministic clock.
//! - [`log`], [`replication`]: the signed, hash-chained event log, the leader/follower replication
//!   protocol and the persistence traits the core implements on SQLite.
//! - [`lan`]: the mutual-TLS crew LAN service (axum, rustls, WebSockets): station identity by
//!   client certificate, user sessions, the role-checked read API and the alarm stream.
//! - [`node`]: the async driver that runs one instance (state machine + stores + LAN service).

pub mod failover;
pub mod fence;
pub mod lan;
pub mod log;
pub mod node;
pub mod replication;
pub mod timing;
pub mod wire;

/// A persistence failure reported by a store the core implements.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("store: {0}")]
pub struct StoreFailure(pub String);
