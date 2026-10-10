//! Test equipment: device emulators, scenario player and fault injection.
//!
//! Never part of a release build (CLAUDE.md rule 3): other crates may use it only as a
//! dev-dependency or behind their `test-equipment` feature, which the CI release guard checks.

/// Adapter name of emulated devices; every reading from it is test data (docs/05 §3).
pub const ADAPTER: &str = "test-equipment";

pub mod sig;
