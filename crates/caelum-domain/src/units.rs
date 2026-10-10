//! Units (UCUM codes) and the only conversions CAELUM performs (R-CLN-02: no guessing).
//!
//! Every conversion is listed explicitly in [`convert`]; anything else is an error, never a
//! silent pass-through. Glucose mass/molar conversion is parameter-specific and only allowed for
//! glucose.

use serde::{Deserialize, Serialize};

/// UCUM units used by the parameter catalog and by device decoders.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum Unit {
    /// Per minute (`/min`): heart, pulse and respiration rates.
    PerMinute,
    /// Percent (`%`).
    Percent,
    /// Dimensionless (`1`): fractions, probabilities, indices, counts.
    One,
    /// Degree Celsius (`Cel`).
    Celsius,
    /// Degree Fahrenheit (`[degF]`).
    Fahrenheit,
    /// Kelvin (`K`).
    Kelvin,
    /// Millimetre of mercury (`mm[Hg]`).
    MmHg,
    /// Kilopascal (`kPa`).
    KiloPascal,
    /// Kilogram (`kg`).
    Kilogram,
    /// Pound (`[lb_av]`).
    Pound,
    /// Milligram per decilitre (`mg/dL`).
    MgPerDl,
    /// Millimole per litre (`mmol/L`).
    MmolPerL,
    /// Millisecond (`ms`).
    Millisecond,
    /// Second (`s`).
    Second,
    /// Minute (`min`).
    Minute,
    /// Microsiemens (`uS`).
    Microsiemens,
    /// Microsievert per hour (`uSv/h`).
    MicrosievertPerHour,
    /// Millisievert (`mSv`).
    Millisievert,
    /// Watt (`W`).
    Watt,
    /// Decibel-milliwatt (`dB[mW]`).
    DbMilliwatt,
}

impl Unit {
    /// The UCUM code.
    pub fn ucum(self) -> &'static str {
        match self {
            Self::PerMinute => "/min",
            Self::Percent => "%",
            Self::One => "1",
            Self::Celsius => "Cel",
            Self::Fahrenheit => "[degF]",
            Self::Kelvin => "K",
            Self::MmHg => "mm[Hg]",
            Self::KiloPascal => "kPa",
            Self::Kilogram => "kg",
            Self::Pound => "[lb_av]",
            Self::MgPerDl => "mg/dL",
            Self::MmolPerL => "mmol/L",
            Self::Millisecond => "ms",
            Self::Second => "s",
            Self::Minute => "min",
            Self::Microsiemens => "uS",
            Self::MicrosievertPerHour => "uSv/h",
            Self::Millisievert => "mSv",
            Self::Watt => "W",
            Self::DbMilliwatt => "dB[mW]",
        }
    }

    /// Parses a UCUM code (exact match only).
    pub fn from_ucum(code: &str) -> Option<Self> {
        ALL.iter().copied().find(|u| u.ucum() == code)
    }
}

const ALL: [Unit; 20] = [
    Unit::PerMinute,
    Unit::Percent,
    Unit::One,
    Unit::Celsius,
    Unit::Fahrenheit,
    Unit::Kelvin,
    Unit::MmHg,
    Unit::KiloPascal,
    Unit::Kilogram,
    Unit::Pound,
    Unit::MgPerDl,
    Unit::MmolPerL,
    Unit::Millisecond,
    Unit::Second,
    Unit::Minute,
    Unit::Microsiemens,
    Unit::MicrosievertPerHour,
    Unit::Millisievert,
    Unit::Watt,
    Unit::DbMilliwatt,
];

/// mmHg per kPa (1 mmHg = 133.322387415 Pa).
const MMHG_PER_KPA: f64 = 1_000.0 / 133.322_387_415;
/// kg per international avoirdupois pound.
const KG_PER_LB: f64 = 0.453_592_37;
/// mg/dL per mmol/L for glucose (molar mass 180.156 g/mol).
const GLUCOSE_MGDL_PER_MMOLL: f64 = 18.0156;

/// Conversion errors.
#[derive(Debug, Clone, PartialEq, thiserror::Error)]
pub enum UnitError {
    /// The value is NaN or infinite.
    #[error("value is not finite")]
    NotFinite,
    /// No conversion is defined between these units (for this parameter).
    #[error("no conversion from {from} to {to}")]
    Unsupported {
        /// Source UCUM code.
        from: &'static str,
        /// Target UCUM code.
        to: &'static str,
    },
}

/// Whether the quantity is glucose (the only parameter with a mass/molar conversion).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Substance {
    /// Glucose: mg/dL ↔ mmol/L allowed.
    Glucose,
    /// Anything else.
    Other,
}

/// Converts `value` from `from` to `to`. Only the conversions listed here exist.
pub fn convert(value: f64, from: Unit, to: Unit, substance: Substance) -> Result<f64, UnitError> {
    use Unit as U;
    if !value.is_finite() {
        return Err(UnitError::NotFinite);
    }
    let out = match (from, to) {
        _ if from == to => value,
        (U::Fahrenheit, U::Celsius) => (value - 32.0) * 5.0 / 9.0,
        (U::Celsius, U::Fahrenheit) => value * 9.0 / 5.0 + 32.0,
        (U::Kelvin, U::Celsius) => value - 273.15,
        (U::Celsius, U::Kelvin) => value + 273.15,
        (U::KiloPascal, U::MmHg) => value * MMHG_PER_KPA,
        (U::MmHg, U::KiloPascal) => value / MMHG_PER_KPA,
        (U::Pound, U::Kilogram) => value * KG_PER_LB,
        (U::Kilogram, U::Pound) => value / KG_PER_LB,
        (U::Second, U::Millisecond) => value * 1_000.0,
        (U::Millisecond, U::Second) => value / 1_000.0,
        (U::Minute, U::Second) => value * 60.0,
        (U::Second, U::Minute) => value / 60.0,
        (U::One, U::Percent) => value * 100.0,
        (U::Percent, U::One) => value / 100.0,
        (U::MgPerDl, U::MmolPerL) if substance == Substance::Glucose => {
            value / GLUCOSE_MGDL_PER_MMOLL
        }
        (U::MmolPerL, U::MgPerDl) if substance == Substance::Glucose => {
            value * GLUCOSE_MGDL_PER_MMOLL
        }
        _ => {
            return Err(UnitError::Unsupported {
                from: from.ucum(),
                to: to.ucum(),
            })
        }
    };
    if out.is_finite() {
        Ok(out)
    } else {
        Err(UnitError::NotFinite)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ucum_codes_round_trip_and_are_unique() {
        for u in ALL {
            assert_eq!(Unit::from_ucum(u.ucum()), Some(u));
        }
        let mut codes: Vec<&str> = ALL.iter().map(|u| u.ucum()).collect();
        codes.sort_unstable();
        codes.dedup();
        assert_eq!(codes.len(), ALL.len());
        assert_eq!(Unit::from_ucum("furlongs"), None);
    }

    #[test]
    fn reference_values() {
        let c = convert(98.6, Unit::Fahrenheit, Unit::Celsius, Substance::Other).expect("defined");
        assert!((c - 37.0).abs() < 1e-9);
        let mmhg = convert(16.0, Unit::KiloPascal, Unit::MmHg, Substance::Other).expect("defined");
        assert!((mmhg - 120.009).abs() < 1e-3);
        let mmol =
            convert(90.0, Unit::MgPerDl, Unit::MmolPerL, Substance::Glucose).expect("defined");
        assert!((mmol - 4.9957).abs() < 1e-3);
    }

    /// R-CLN-02: no silent pass-through for undefined conversions or non-glucose molar units.
    #[test]
    fn undefined_conversions_are_errors() {
        assert!(convert(1.0, Unit::Celsius, Unit::MmHg, Substance::Other).is_err());
        assert!(convert(90.0, Unit::MgPerDl, Unit::MmolPerL, Substance::Other).is_err());
        assert_eq!(
            convert(f64::NAN, Unit::Celsius, Unit::Celsius, Substance::Other),
            Err(UnitError::NotFinite)
        );
        assert_eq!(
            convert(f64::MAX, Unit::One, Unit::Percent, Substance::Other),
            Err(UnitError::NotFinite)
        );
    }

    mod properties {
        use super::*;
        use proptest::prelude::*;

        fn pairs() -> Vec<(Unit, Unit, Substance)> {
            use Unit as U;
            vec![
                (U::Fahrenheit, U::Celsius, Substance::Other),
                (U::Kelvin, U::Celsius, Substance::Other),
                (U::KiloPascal, U::MmHg, Substance::Other),
                (U::Pound, U::Kilogram, Substance::Other),
                (U::Second, U::Millisecond, Substance::Other),
                (U::Minute, U::Second, Substance::Other),
                (U::One, U::Percent, Substance::Other),
                (U::MgPerDl, U::MmolPerL, Substance::Glucose),
            ]
        }

        proptest! {
            /// Every defined conversion round-trips within floating-point tolerance.
            #[test]
            fn round_trip(v in -1.0e6f64..1.0e6, i in 0usize..8) {
                let (a, b, s) = pairs()[i];
                let there = convert(v, a, b, s).expect("defined");
                let back = convert(there, b, a, s).expect("defined");
                prop_assert!((back - v).abs() <= 1e-9 * v.abs().max(1.0));
            }

            /// Every defined conversion is strictly increasing (ordering of readings is kept).
            #[test]
            fn monotonic(v in -1.0e6f64..1.0e6, d in 1.0e-3f64..1.0e3, i in 0usize..8) {
                let (a, b, s) = pairs()[i];
                prop_assert!(convert(v + d, a, b, s).expect("defined") > convert(v, a, b, s).expect("defined"));
            }
        }
    }
}
