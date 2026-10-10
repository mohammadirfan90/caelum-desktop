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

/// Where a thermometer reading was taken: the Bluetooth Health Thermometer "Temperature Type"
/// (GATT Specification Supplement 3.218). `Unknown` when the device sends none; never guessed
/// (docs/05 §1, CR-002 B6).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TemperatureSite {
    /// 1: armpit.
    Armpit,
    /// 2: body (general).
    Body,
    /// 3: ear (usually earlobe).
    Ear,
    /// 4: finger.
    Finger,
    /// 5: gastro-intestinal tract.
    GiTract,
    /// 6: mouth.
    Mouth,
    /// 7: rectum.
    Rectum,
    /// 8: toe.
    Toe,
    /// 9: tympanum (ear drum).
    Tympanum,
    /// Not reported by the device, or a reserved code.
    Unknown,
}

/// ⚕ DRAFT site class for the temperature rules (CR-003 §3, medical lead to confirm).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SiteClass {
    /// Uses the `core_temp` thresholds.
    CoreEquivalent,
    /// Separate, draft thresholds.
    Peripheral,
    /// Site not known: the most conservative threshold applies.
    Unknown,
}

impl TemperatureSite {
    /// The site for an HTS Temperature Type code; reserved codes are `Unknown`.
    pub fn from_hts_code(code: u8) -> Self {
        match code {
            1 => Self::Armpit,
            2 => Self::Body,
            3 => Self::Ear,
            4 => Self::Finger,
            5 => Self::GiTract,
            6 => Self::Mouth,
            7 => Self::Rectum,
            8 => Self::Toe,
            9 => Self::Tympanum,
            _ => Self::Unknown,
        }
    }

    /// ⚕ DRAFT class (CR-003 §3): core-equivalent = HTS 2, 3, 5, 6, 7, 9; peripheral = 1, 4, 8.
    pub fn class(self) -> SiteClass {
        match self {
            Self::Body
            | Self::Ear
            | Self::GiTract
            | Self::Mouth
            | Self::Rectum
            | Self::Tympanum => SiteClass::CoreEquivalent,
            Self::Armpit | Self::Finger | Self::Toe => SiteClass::Peripheral,
            Self::Unknown => SiteClass::Unknown,
        }
    }
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

    /// CR-003 §3 ⚕: HTS codes map to sites and classes; absent or reserved codes are unknown.
    #[test]
    fn hts_temperature_types_map_to_sites_and_draft_classes() {
        let classes: Vec<SiteClass> = (0..=10)
            .map(|c| TemperatureSite::from_hts_code(c).class())
            .collect();
        use SiteClass::{CoreEquivalent as C, Peripheral as P, Unknown as U};
        assert_eq!(classes, [U, P, C, C, P, C, C, C, P, C, U]);
        assert_eq!(
            TemperatureSite::from_hts_code(255),
            TemperatureSite::Unknown
        );
    }
}
