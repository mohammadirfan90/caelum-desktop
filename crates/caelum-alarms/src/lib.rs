//! Alarm core: signed ruleset loader, deterministic rule engine (threshold, absence-of-event, spot
//! confirmation, baseline), escalation and technical alarms.
//!
//! Safety-critical (IEC 62304 class C, docs/02_NASA_REQUIREMENTS.md §4).
//! Implemented in M3 and M4: F-ALM-01..08, SR-001..SR-019.

pub mod engine;
pub mod ruleset;
pub mod signing;

/// The bundled ruleset and its signatures (CLAUDE.md rule 7: single source of truth).
pub const BUNDLED_RULESET: &str = include_str!("../rules/ruleset.toml");
/// Signatures of [`BUNDLED_RULESET`].
pub const BUNDLED_SIGNATURES: &str = include_str!("../rules/ruleset.sig.toml");
