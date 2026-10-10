//! The ingest pipeline (docs/03 §3 step 2): assignment, demonstration/test flags, value type,
//! explicit unit conversion, plausibility, time correction and provenance.
//!
//! A reading either becomes one [`Observation`] or is rejected with a [`Rejection`] reason; no
//! field is ever defaulted (CLAUDE.md rule 2, R-CLN-02). Rejections are counted per device as
//! dropped readings (device health, F-ACQ-10).

use caelum_domain::ids::CrewMemberId;
use caelum_domain::observation::{Classification, TemperatureSite};
use caelum_domain::parameters::{Parameter, ValueType};
use caelum_domain::units::{convert, Substance, UnitError};

use crate::clock::ClockOffset;
use crate::reading::{Observation, Original, Provenance, RawReading, RawValue, TimeSource, Value};

/// A corrected time later than this after receipt is a clock fault, not a reading (ns).
pub const MAX_FUTURE_NS: i64 = 1_000_000_000;

/// Why a reading was not accepted.
#[derive(Debug, Clone, PartialEq, thiserror::Error)]
pub enum Rejection {
    /// The device is not assigned to a crew member, so the reading has no owner.
    #[error("device is not assigned to a crew member")]
    Unassigned,
    /// SR-017: the device marked the reading as demonstration data.
    #[error("device marked the reading as demonstration data")]
    DemonstrationData,
    /// SR-017: the device marked the reading as test data.
    #[error("device marked the reading as test data")]
    TestData,
    /// `core_temp` is the F-SIG-05 estimate only; devices never write it (docs/05 §1).
    #[error("core_temp is derived only; thermometer readings go to body_temp_measured")]
    CoreTempFromDevice,
    /// A thermometer site was given for a parameter other than `body_temp_measured`.
    #[error("a thermometer site was given for {0:?}")]
    UnexpectedSite(Parameter),
    /// The device sent no value.
    #[error("no value")]
    MissingValue,
    /// A number without a unit (never assumed).
    #[error("no unit")]
    MissingUnit,
    /// A number for an enumerated parameter or a state for a numeric one.
    #[error("value type does not match the parameter")]
    WrongValueType,
    /// A state the catalog does not define.
    #[error("state {0:?} is not defined for this parameter")]
    UnknownState(String),
    /// The parameter's states are not defined yet (no guessed values).
    #[error("states for this parameter are not defined yet ({0})")]
    StatesPending(&'static str),
    /// No conversion from the device's unit to the canonical unit.
    #[error("no conversion from {from} to {to}")]
    UnitNotConvertible {
        /// Device unit.
        from: &'static str,
        /// Canonical unit.
        to: &'static str,
    },
    /// NaN or infinite.
    #[error("value is not finite")]
    NotFinite,
    /// Outside the sensor-fault limits of the catalog (⚕ DRAFT, CR-002 B3).
    #[error("value {value} outside plausible range {min}..={max}")]
    Implausible {
        /// Converted value.
        value: f64,
        /// Lowest plausible value.
        min: f64,
        /// Highest plausible value.
        max: f64,
    },
    /// The corrected measurement time is after receipt by more than [`MAX_FUTURE_NS`].
    #[error("measurement time is {ahead_ms} ms after receipt (clock fault)")]
    TimeInFuture {
        /// How far ahead, ms.
        ahead_ms: i64,
    },
}

impl Rejection {
    /// Stable code for counters and logs.
    pub fn code(&self) -> &'static str {
        match self {
            Self::Unassigned => "unassigned",
            Self::DemonstrationData => "demonstration_data",
            Self::TestData => "test_data",
            Self::CoreTempFromDevice => "core_temp_from_device",
            Self::UnexpectedSite(_) => "unexpected_site",
            Self::MissingValue => "missing_value",
            Self::MissingUnit => "missing_unit",
            Self::WrongValueType => "wrong_value_type",
            Self::UnknownState(_) => "unknown_state",
            Self::StatesPending(_) => "states_pending",
            Self::UnitNotConvertible { .. } => "unit_not_convertible",
            Self::NotFinite => "not_finite",
            Self::Implausible { .. } => "implausible",
            Self::TimeInFuture { .. } => "time_in_future",
        }
    }
}

/// SR-017: release builds reject flagged readings; test-equipment builds keep them, classified
/// as simulated so they can never leave the test environment (docs/05 §3).
fn classification_for(reading: &RawReading) -> Result<Classification, Rejection> {
    let flagged = reading.flags.demonstration || reading.flags.test_data;
    #[cfg(feature = "test-equipment")]
    if flagged {
        return Ok(Classification::Simulated);
    }
    if flagged {
        return Err(if reading.flags.demonstration {
            Rejection::DemonstrationData
        } else {
            Rejection::TestData
        });
    }
    Ok(Classification::Measured)
}

/// Validates one device reading. `assignment` is the crew member the device registry assigns the
/// device to; `clock` is the device's measured offset, if any.
pub fn ingest(
    reading: RawReading,
    assignment: Option<CrewMemberId>,
    clock: Option<&ClockOffset>,
) -> Result<Observation, Rejection> {
    let crew_member_id = assignment.ok_or(Rejection::Unassigned)?;
    let classification = classification_for(&reading)?;
    let parameter = reading.parameter;
    if parameter == Parameter::CoreTemp {
        return Err(Rejection::CoreTempFromDevice);
    }
    let thermometer = parameter == Parameter::BodyTempMeasured;
    if !thermometer && reading.site.is_some() {
        return Err(Rejection::UnexpectedSite(parameter));
    }
    let spec = parameter.spec();
    let raw = reading.value.as_ref().ok_or(Rejection::MissingValue)?;

    let (value, original) = match (spec.value, raw) {
        (ValueType::Numeric { min, max }, RawValue::Number(v)) => {
            let from = reading.unit.ok_or(Rejection::MissingUnit)?;
            let substance = if parameter == Parameter::Glucose {
                Substance::Glucose
            } else {
                Substance::Other
            };
            let converted = convert(*v, from, spec.unit, substance).map_err(|e| match e {
                UnitError::NotFinite => Rejection::NotFinite,
                UnitError::Unsupported { from, to } => Rejection::UnitNotConvertible { from, to },
            })?;
            if !(min..=max).contains(&converted) {
                return Err(Rejection::Implausible {
                    value: converted,
                    min,
                    max,
                });
            }
            let original = Original {
                value: *v,
                unit: from.ucum(),
            };
            (Value::Number(converted), Some(original))
        }
        (ValueType::Enum(states), RawValue::State(s)) => {
            let state = states
                .iter()
                .copied()
                .find(|known| *known == s.as_str())
                .ok_or_else(|| Rejection::UnknownState(s.clone()))?;
            (Value::State(state), None)
        }
        (ValueType::EnumPending(why), _) => return Err(Rejection::StatesPending(why)),
        (ValueType::Numeric { .. }, RawValue::State(_))
        | (ValueType::Enum(_), RawValue::Number(_)) => return Err(Rejection::WrongValueType),
    };

    let (measured_at, time) = match (reading.device_time, clock) {
        (Some(device_time), Some(offset)) => (
            offset.correct(device_time),
            TimeSource::DeviceCorrected {
                offset_ns: offset.offset_ns,
                residual_ns: offset.residual_ns,
            },
        ),
        (Some(_), None) => (reading.received_at, TimeSource::ReceiptUnsynchronised),
        (None, _) => (reading.received_at, TimeSource::Receipt),
    };
    let ahead = measured_at.0.saturating_sub(reading.received_at.0);
    if ahead > MAX_FUTURE_NS {
        return Err(Rejection::TimeInFuture {
            ahead_ms: ahead / 1_000_000,
        });
    }

    Ok(Observation {
        crew_member_id,
        device_id: reading.device_id,
        parameter,
        value,
        unit: spec.unit,
        measured_at,
        received_at: reading.received_at,
        quality: reading.quality,
        classification,
        mode: reading.mode,
        provenance: Provenance {
            time,
            original,
            site: thermometer.then(|| reading.site.unwrap_or(TemperatureSite::Unknown)),
        },
    })
}
