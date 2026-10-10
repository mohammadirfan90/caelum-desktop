//! Station side of the medical data system link (F-GND-12, SR-016; CR-016 §10): the signed
//! `mds` configuration and the client identity (administrator, re-authentication, audited),
//! read-back observations stored and evaluated like any reading, and the technical alarm.

use caelum_alarms::signing::{built_in_keys, verify_config, TrustPolicy};
use caelum_devices::registry::{DeviceIdentity, Registry};
use caelum_domain::observation::{Classification, MeasurementMode, Quality};
use caelum_ingest::{Observation, Provenance, TimeSource, Value};
use caelum_mds::fhir::InboundObservation;
use caelum_security::audit::{Actor, AuditAction};
use caelum_security::role::Command;
use serde_json::json;

use super::{Station, StationError};
use crate::acquisition::observation_record;
use crate::alarms::Output;
use crate::mds::{MdsConfig, IDENTITY_CERT, IDENTITY_KEY, MDS_KIND};

/// Technical alarm while the medical data system is unreachable (records are kept and resent).
pub const MDS_RULE: &str = "technical.mds_unreachable";
/// Adapter of the pseudo-device that read-back observations come from.
pub const MDS_ADAPTER: &str = "mds";

fn rejected(reason: impl Into<String>) -> StationError {
    StationError::Rejected(reason.into())
}

impl Station {
    /// `config_apply_mds`: the signed medical data system configuration. Audited.
    pub fn config_apply_mds(
        &mut self,
        bundle: &[u8],
        signature: &str,
    ) -> Result<String, StationError> {
        let user = self.admin_for(Command::ConfigApplyMds)?;
        let verified = verify_config(
            MDS_KIND,
            bundle,
            signature,
            &built_in_keys(),
            TrustPolicy::for_this_build(),
        )
        .map_err(|e| {
            rejected(format!(
                "the mds configuration signature is not accepted: {e}"
            ))
        })?;
        let text = std::str::from_utf8(bundle).map_err(|_| rejected("not UTF-8 text"))?;
        let config = MdsConfig::parse(text)
            .map_err(|e| rejected(format!("the mds configuration is invalid: {e}")))?;
        if config.version != verified.version {
            return Err(rejected(
                "the configuration version is not the signed version",
            ));
        }
        let now = self.now()?;
        self.db.put_config_bundle(
            &format!("{MDS_KIND}-{}", verified.sha256),
            MDS_KIND,
            &verified.version,
            signature,
            text,
            now.0,
        )?;
        self.log().append(
            &Actor::user(&user),
            AuditAction::ConfigApplied,
            Some(MDS_KIND),
            &json!({ "kind": MDS_KIND, "version": verified.version, "sha256": verified.sha256 }),
        )?;
        Ok(verified.version)
    }

    /// `mds_identity_set`: this station's client certificate and key for the medical data
    /// system (PEM), kept in the encrypted database. Audited without the key.
    pub fn mds_identity_set(&mut self, cert_pem: &str, key_pem: &str) -> Result<(), StationError> {
        let user = self.admin_for(Command::MdsIdentitySet)?;
        if !cert_pem.contains("BEGIN CERTIFICATE") || !key_pem.contains("PRIVATE KEY") {
            return Err(rejected("expected a PEM certificate and a PEM private key"));
        }
        self.db.mds_set_state(IDENTITY_CERT, cert_pem)?;
        self.db.mds_set_state(IDENTITY_KEY, key_pem)?;
        let digest = caelum_alarms::signing::digest(cert_pem.as_bytes());
        self.log().append(
            &Actor::user(&user),
            AuditAction::ConfigApplied,
            Some("mds-identity"),
            &json!({ "certificate_sha256": digest }),
        )?;
        Ok(())
    }

    /// `mds_status`: who may see the link status (counts only).
    pub fn mds_status_allowed(&mut self) -> Result<(), StationError> {
        self.user_for(Command::MdsStatus).map(|_| ())
    }

    /// A read-back observation (SR-016): stored as `imported` and evaluated like direct data.
    /// FHIR carries no signal quality; it is recorded as degraded, never assumed good.
    pub fn ingest_imported(
        &mut self,
        inbound: &InboundObservation,
    ) -> Result<(Output, Option<String>), StationError> {
        self.require_acting()?;
        let Some(crew) = inbound.crew else {
            return Ok((Output::default(), None));
        };
        if !self
            .db
            .crew_member(&crew.to_string())?
            .is_some_and(|c| c.active)
        {
            return Ok((Output::default(), None));
        }
        let device = Registry::new(&self.db)
            .register(&DeviceIdentity {
                adapter: MDS_ADAPTER.to_owned(),
                vendor: "medical data system".to_owned(),
                model: "FHIR read-back".to_owned(),
                serial: "mds".to_owned(),
                firmware: None,
                accuracy_class: None,
                calibration_due: None,
            })
            .map_err(|e| rejected(e.to_string()))?;
        let now = self.now()?;
        let obs = Observation {
            crew_member_id: crew,
            device_id: device,
            parameter: inbound.parameter,
            value: Value::Number(inbound.value),
            unit: inbound.unit,
            measured_at: inbound.effective,
            received_at: now,
            quality: Quality::Degraded,
            classification: Classification::Imported,
            mode: MeasurementMode::Spot,
            provenance: Provenance {
                time: TimeSource::Receipt,
                original: None,
                site: None,
            },
        };
        let row = observation_record(&obs).map_err(|e| rejected(e.to_string()))?;
        self.db.insert_observation(&row)?;
        Ok((self.observe(&obs)?, Some(crew.to_string())))
    }

    /// Raises or clears `technical.mds_unreachable` (station-wide).
    pub fn mds_technical(&mut self, active: bool) -> Result<(), StationError> {
        if active == self.alarms.technical_open(MDS_RULE, None) {
            return Ok(());
        }
        let now = self.now()?;
        let why = active.then(|| json!({ "mds": "the medical data system is unreachable; records are kept and resent" }));
        self.alarms.technical(&self.db, MDS_RULE, None, why, now)?;
        Ok(())
    }
}
