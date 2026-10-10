//! Observation qualifiers (docs/05_DATA_MODEL.md §2–3).

use serde::{Deserialize, Serialize};

/// Signal quality reported with a reading. POOR and UNUSABLE pause physiological rules (SR-005).
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub enum Quality {
    /// Good signal.
    Good,
    /// Usable with reduced confidence.
    Degraded,
    /// Too poor for physiological rules.
    Poor,
    /// No usable signal.
    Unusable,
}

impl Quality {
    /// Whether physiological rules may evaluate a reading of this quality (SR-005).
    pub fn usable_for_rules(self) -> bool {
        matches!(self, Self::Good | Self::Degraded)
    }
}

/// Where a value came from. There is no simulated variant in release builds: test data cannot be
/// constructed outside `test-equipment` builds (docs/05 §3, CLAUDE.md rule 3).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum Classification {
    /// Measured by a device.
    Measured,
    /// Computed from measured values by a versioned algorithm.
    Derived,
    /// Entered by a person.
    Entered,
    /// Imported from a record (FHIR, files).
    Imported,
    /// Produced by test equipment. Exists only in `test-equipment` builds.
    #[cfg(feature = "test-equipment")]
    Simulated,
}

/// How a reading was taken; spot readings use confirmation instead of persistence (SR-019).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum MeasurementMode {
    /// Streamed continuously.
    Continuous,
    /// A single reading (spot-check oximeter, cuff, thermometer, scale, glucose, manual entry).
    Spot,
}

#[cfg(test)]
mod tests {
    use super::*;

    /// SR-005: only GOOD and DEGRADED readings reach physiological rules.
    #[test]
    fn sr_005_poor_and_unusable_pause_rules() {
        assert!(Quality::Good.usable_for_rules());
        assert!(Quality::Degraded.usable_for_rules());
        assert!(!Quality::Poor.usable_for_rules());
        assert!(!Quality::Unusable.usable_for_rules());
    }
}
