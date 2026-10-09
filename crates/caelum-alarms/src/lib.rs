//! Alarm core: signed ruleset loader, deterministic rule engine (threshold, absence-of-event, spot
//! confirmation, baseline), escalation and technical alarms.
//!
//! Safety-critical (IEC 62304 class C, docs/02_NASA_REQUIREMENTS.md §4).
//! Implemented in M3 and M4: F-ALM-01..08, SR-001..SR-019.
