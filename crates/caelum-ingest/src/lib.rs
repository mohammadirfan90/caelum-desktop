//! Ingest pipeline: validation, unit conversion, plausibility, provenance, device clock correction
//! and rejection reasons. No default values are ever substituted for missing data.
//!
//! Safety-critical (IEC 62304 class C, docs/02_NASA_REQUIREMENTS.md §4).
//! Implemented in M2: R-CLN-02, F-ACQ-11, R-OPS-04, SR-017, SR-018.
