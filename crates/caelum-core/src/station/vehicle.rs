//! Vehicle data into the station (M9 integration; F-ENV-01, F-ENV-02, F-ENV-03, F-GND-11,
//! R-INT-04; CR-015, docs/integration/M9.md): the signed `vehicle` MID table, and each decoded
//! reading stored and evaluated like any observation, so `co2.warning` and `o2.critical` raise
//! ENVIRONMENT alarms through the normal escalation. Read-only to the vehicle (CLAUDE.md rule 5).
//!
//! Cabin readings apply to everyone aboard: they are stored once per active roster crew member.
//! `spe_state` values wait for ICD-VEH OI-2 (the catalog has no agreed states) and are not stored.

use caelum_alarms::signing::{built_in_keys, verify_config, TrustPolicy};
use caelum_devices::registry::{DeviceIdentity, Registry};
use caelum_domain::ids::{CrewMemberId, DeviceId};
use caelum_domain::observation::Classification;
use caelum_ingest::{Observation, Original, Provenance, TimeSource, Value};
use caelum_security::audit::{Actor, AuditAction};
use caelum_security::role::Command;
use caelum_vehicle::{Subject, VehicleConfig, VehicleReading, VehicleValue};
use serde_json::json;

use super::{Station, StationError};
use crate::acquisition::observation_record;
use crate::alarms::Output;

/// Configuration kind of the vehicle MID table.
pub const VEHICLE_KIND: &str = "vehicle";
/// Adapter of the per-MID pseudo-devices (staleness and technical alarms per message).
pub const VEHICLE_ADAPTER: &str = "vehicle";

/// What a batch of vehicle readings produced.
#[derive(Debug, Default)]
pub struct VehicleIngested {
    /// Alarm events and notices for the alarm bus.
    pub output: Output,
    /// Crew members whose data changed (value-free events).
    pub crews: Vec<String>,
    /// Readings stored (one per crew member for cabin readings).
    pub stored: usize,
    /// Readings not stored, with the reason code.
    pub skipped: Vec<&'static str>,
}

fn rejected(reason: impl Into<String>) -> StationError {
    StationError::Rejected(reason.into())
}

impl Station {
    /// `config_apply_vehicle`: verifies and applies a signed MID table (administrator,
    /// re-authentication). The device runtime rebinds the receiver on the new version. Audited.
    pub fn config_apply_vehicle(
        &mut self,
        bundle: &[u8],
        signature: &str,
    ) -> Result<String, StationError> {
        let user = self.admin_for(Command::ConfigApplyVehicle)?;
        let verified = verify_config(
            VEHICLE_KIND,
            bundle,
            signature,
            &built_in_keys(),
            TrustPolicy::for_this_build(),
        )
        .map_err(|e| rejected(format!("the vehicle table signature is not accepted: {e}")))?;
        let text = std::str::from_utf8(bundle)
            .map_err(|_| rejected("the vehicle table is not UTF-8 text"))?;
        let config = VehicleConfig::from_toml(text)
            .map_err(|e| rejected(format!("the vehicle table is invalid: {e}")))?;
        if config.version != verified.version {
            return Err(rejected("the table version is not the signed version"));
        }
        let now = self.now()?;
        self.db.put_config_bundle(
            &format!("{VEHICLE_KIND}-{}", verified.sha256),
            VEHICLE_KIND,
            &verified.version,
            signature,
            text,
            now.0,
        )?;
        self.log().append(
            &Actor::user(&user),
            AuditAction::ConfigApplied,
            Some(VEHICLE_KIND),
            &json!({ "kind": VEHICLE_KIND, "version": verified.version, "sha256": verified.sha256,
                     "development": verified.development }),
        )?;
        Ok(verified.version)
    }

    /// The vehicle MID table in force (re-verified from its stored bundle), if any.
    pub fn vehicle_config(&self) -> Result<Option<VehicleConfig>, StationError> {
        let Some((version, signature, content)) = self.db.latest_config_bundle(VEHICLE_KIND)?
        else {
            return Ok(None);
        };
        verify_config(
            VEHICLE_KIND,
            content.as_bytes(),
            &signature,
            &built_in_keys(),
            TrustPolicy::for_this_build(),
        )
        .map_err(|e| rejected(format!("the stored vehicle table no longer verifies: {e}")))?;
        let config = VehicleConfig::from_toml(&content).map_err(|e| rejected(e.to_string()))?;
        Ok((config.version == version).then_some(config))
    }

    /// `vehicle_link_status`: may the caller see the bridge health (technical, no physiology)?
    pub fn vehicle_link_allowed(&mut self) -> Result<(), StationError> {
        self.user_for(Command::VehicleLinkStatus).map(|_| ())
    }

    /// Station TAI now (for the receiver's clock).
    pub fn now_tai(&self) -> Result<caelum_domain::time::TaiNs, StationError> {
        self.now()
    }

    /// The pseudo-device of one MID, registered on first use.
    fn vehicle_device(&self, mid: u16, message: &str) -> Result<DeviceId, StationError> {
        Registry::new(&self.db)
            .register(&DeviceIdentity {
                adapter: VEHICLE_ADAPTER.to_owned(),
                vendor: "vehicle".to_owned(),
                model: message.to_owned(),
                serial: format!("0x{mid:04X}"),
                firmware: None,
                accuracy_class: None,
                calibration_due: None,
            })
            .map_err(|e| rejected(e.to_string()))
    }

    /// Stores and evaluates decoded vehicle readings (docs/03 §3 step 5: synchronously).
    pub fn ingest_vehicle(
        &mut self,
        readings: &[VehicleReading],
    ) -> Result<VehicleIngested, StationError> {
        self.require_acting()?;
        let mut out = VehicleIngested::default();
        let active: Vec<CrewMemberId> = self
            .db
            .crew_members()?
            .into_iter()
            .filter(|c| c.active)
            .filter_map(|c| c.id.parse().ok())
            .collect();
        for r in readings {
            let VehicleValue::Number(v) = r.value else {
                out.skipped.push("state_pending_oi_2");
                continue;
            };
            let crews: Vec<CrewMemberId> = match &r.subject {
                Subject::Crew(c) if active.contains(c) => vec![*c],
                Subject::Crew(_) => {
                    out.skipped.push("crew_not_on_roster");
                    continue;
                }
                Subject::Cabin { .. } => active.clone(),
            };
            let device = self.vehicle_device(r.mid, &r.message)?;
            let time = match r.time {
                caelum_vehicle::TimeSource::Vehicle { offset_ns, .. } => {
                    TimeSource::DeviceCorrected {
                        offset_ns,
                        residual_ns: 0,
                    }
                }
                caelum_vehicle::TimeSource::ReceiptUnsynchronised => {
                    TimeSource::ReceiptUnsynchronised
                }
            };
            for crew in crews {
                let obs = Observation {
                    crew_member_id: crew,
                    device_id: device,
                    parameter: r.parameter,
                    value: Value::Number(v),
                    unit: r.unit,
                    measured_at: r.measured_at,
                    received_at: r.received_at,
                    quality: r.quality,
                    classification: Classification::Measured,
                    mode: r.mode,
                    provenance: Provenance {
                        time,
                        original: r.original.map(|o| Original {
                            value: o.value,
                            unit: o.unit,
                        }),
                        site: None,
                    },
                };
                let row = observation_record(&obs).map_err(|e| rejected(e.to_string()))?;
                self.db.insert_observation(&row)?;
                out.stored += 1;
                let o = self.observe(&obs)?;
                out.output.events.extend(o.events);
                out.output.notices.extend(o.notices);
                let id = crew.to_string();
                if !out.crews.contains(&id) {
                    out.crews.push(id);
                }
            }
        }
        Ok(out)
    }
}
