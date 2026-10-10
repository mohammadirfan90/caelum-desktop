//! Wiring: task supervisor, event bus, configuration and self-test.
//!
//! M6: crew roster and user administration in [`station`] (F-SYS-07, CR-012).
//! M3: [`alarms`] (signed ruleset with SR-009 fallback, engine in the ingest path).
//! M2: [`acquisition`] (notification → decode → ingest → store; F-ACQ-02/09/10/11).
//! M1: [`station`] (records, keys, session and the M1 commands; F-SYS-01/02/03). Self-test and
//! watchdogs arrive in M4 (F-SYS-09), the rest as each subsystem lands.

pub mod acquisition;
pub mod alarms;
pub mod station;

/// Security types used by the application shell (roles, sessions, audit).
pub use caelum_security as security;
pub use caelum_store as store;
