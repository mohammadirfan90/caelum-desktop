//! Retention (F-SYS-08, M8 gate; CR-022): limits per retention class from a signed `retention`
//! configuration, applied once a day. Without that configuration nothing is ever removed.
//!
//! - `Mission` data (every physiological, clinical, environment and derived parameter, alarms,
//!   encounters, audit) is never removed by retention.
//! - `Technical` parameters (link and data quality) are removed after `technical_days`.
//! - Raw waveform chunks are removed after `waveform_days`.
//! - Nothing within `alarm_guard_minutes` of an alarm of the same crew member is removed, so the
//!   data around every alarm stays for review. Each purge is audited with its counts.

use caelum_alarms::signing::{built_in_keys, verify_config, TrustPolicy};
use caelum_domain::parameters::{Retention, CATALOG};
use caelum_domain::time::TaiNs;
use caelum_security::audit::{Actor, AuditAction};
use caelum_security::role::Command;
use serde::Deserialize;
use serde_json::json;

use super::{Station, StationError};

/// Configuration kind.
pub const RETENTION_KIND: &str = "retention";
const NS: i64 = 1_000_000_000;
const DAY_NS: i64 = 86_400 * NS;

fn rejected(reason: impl Into<String>) -> StationError {
    StationError::Rejected(reason.into())
}

/// The signed `retention` bundle. Every value is required: no defaults.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RetentionConfig {
    /// Bundle version.
    pub version: String,
    /// Technical parameters are kept this many days (≥ 1).
    pub technical_days: u32,
    /// Raw waveform chunks are kept this many days (≥ 1).
    pub waveform_days: u32,
    /// Data within this many minutes of an alarm is kept (≥ 1).
    pub alarm_guard_minutes: u32,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct RetentionFile {
    retention: RetentionConfig,
}

impl RetentionConfig {
    /// Parses and checks the bundle content.
    pub fn parse(text: &str) -> Result<Self, String> {
        let f: RetentionFile = toml::from_str(text).map_err(|e| e.to_string())?;
        let c = f.retention;
        if c.technical_days == 0 || c.waveform_days == 0 || c.alarm_guard_minutes == 0 {
            return Err("every limit must be at least 1".into());
        }
        Ok(c)
    }
}

/// What one purge removed.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Purged {
    /// Technical observations removed.
    pub observations: usize,
    /// Waveform chunks removed.
    pub waveforms: usize,
}

impl Station {
    /// `config_apply_retention`: the signed retention limits. Administrator, re-authentication;
    /// audited.
    pub fn config_apply_retention(
        &mut self,
        bundle: &[u8],
        signature: &str,
    ) -> Result<String, StationError> {
        let user = self.admin_for(Command::ConfigApplyRetention)?;
        let verified = verify_config(
            RETENTION_KIND,
            bundle,
            signature,
            &built_in_keys(),
            TrustPolicy::for_this_build(),
        )
        .map_err(|e| {
            rejected(format!(
                "the retention configuration signature is not accepted: {e}"
            ))
        })?;
        let text = std::str::from_utf8(bundle).map_err(|_| rejected("not UTF-8 text"))?;
        let config = RetentionConfig::parse(text)
            .map_err(|e| rejected(format!("the retention configuration is invalid: {e}")))?;
        if config.version != verified.version {
            return Err(rejected(
                "the configuration version is not the signed version",
            ));
        }
        let now = self.now()?;
        self.db.put_config_bundle(
            &format!("{RETENTION_KIND}-{}", verified.sha256),
            RETENTION_KIND,
            &verified.version,
            signature,
            text,
            now.0,
        )?;
        self.log().append(
            &Actor::user(&user),
            AuditAction::ConfigApplied,
            Some(RETENTION_KIND),
            &json!({ "kind": RETENTION_KIND, "version": verified.version, "sha256": verified.sha256 }),
        )?;
        Ok(verified.version)
    }

    /// The retention limits in force (re-verified, SR-009), if any.
    pub fn retention_config(&self) -> Result<Option<RetentionConfig>, StationError> {
        let Some((version, signature, content)) = self.db.latest_config_bundle(RETENTION_KIND)?
        else {
            return Ok(None);
        };
        verify_config(
            RETENTION_KIND,
            content.as_bytes(),
            &signature,
            &built_in_keys(),
            TrustPolicy::for_this_build(),
        )
        .map_err(|e| {
            rejected(format!(
                "the stored retention configuration no longer verifies: {e}"
            ))
        })?;
        let c = RetentionConfig::parse(&content).map_err(rejected)?;
        Ok((c.version == version).then_some(c))
    }

    /// Applies retention now (also run once a day from the tick). Audited when anything goes.
    pub fn retention_purge(&mut self, now: TaiNs) -> Result<Purged, StationError> {
        let Some(c) = self.retention_config()? else {
            return Ok(Purged::default());
        };
        let guard = i64::from(c.alarm_guard_minutes) * 60 * NS;
        let technical: Vec<&str> = CATALOG
            .iter()
            .filter(|s| s.retention == Retention::Technical)
            .map(|s| s.key)
            .collect();
        let purged = Purged {
            observations: self.db.purge_observations(
                &technical,
                now.0 - i64::from(c.technical_days) * DAY_NS,
                guard,
            )?,
            waveforms: self
                .db
                .purge_waveforms(now.0 - i64::from(c.waveform_days) * DAY_NS, guard)?,
        };
        if purged != Purged::default() {
            self.log().append(
                &Actor::system(),
                AuditAction::RetentionPurged,
                Some(RETENTION_KIND),
                &json!({
                    "version": c.version,
                    "technical_observations": purged.observations,
                    "waveform_chunks": purged.waveforms,
                }),
            )?;
        }
        Ok(purged)
    }

    /// Runs retention once a day (on the tick).
    pub(super) fn retention_tick(&mut self, now: TaiNs) -> Result<(), StationError> {
        if self.retention_at.is_some_and(|t| now.0 - t < DAY_NS) {
            return Ok(());
        }
        self.retention_at = Some(now.0);
        self.retention_purge(now).map(|_| ())
    }
}
