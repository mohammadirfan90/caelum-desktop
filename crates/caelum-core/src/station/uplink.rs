//! Uplink of approved items (F-GND-07, CR-016): signed rulesets, protocol libraries and
//! configuration bundles, verified before anything is applied and audited. An uplinked ruleset
//! takes effect at the next start (open alarms resume at their stage, SR-007); the other kinds
//! apply now through their own commands' checks.

use caelum_alarms::signing::{built_in_keys, TrustPolicy};
use caelum_ground::uplink::{verify_uplink, UplinkKind, VerifiedUplink};
use caelum_security::audit::{Actor, AuditAction};
use caelum_security::role::Command;
use serde_json::json;

use super::{Station, StationError};

/// Configuration kind of an uplinked ruleset (read at start-up).
pub const RULESET_UPLINK_KIND: &str = "alarm-ruleset-uplink";

fn rejected(reason: impl Into<String>) -> StationError {
    StationError::Rejected(reason.into())
}

/// What an uplink did.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UplinkResult {
    /// The verified version.
    pub version: String,
    /// Takes effect at the next start (rulesets).
    pub at_next_start: bool,
}

impl Station {
    /// `uplink_apply`: verifies and applies one uplinked item. `kind` is `ruleset`, `protocols`,
    /// `crew`, `vehicle`, `ground`, `mds` or `cluster`. Administrator, re-authentication; audited.
    pub fn uplink_apply(
        &mut self,
        kind: &str,
        content: &[u8],
        envelope: &str,
    ) -> Result<UplinkResult, StationError> {
        let user = self.admin_for(Command::UplinkApply)?;
        let result = match kind {
            "ruleset" => {
                let verified = verify_uplink(
                    &UplinkKind::Ruleset,
                    content,
                    envelope,
                    &built_in_keys(),
                    TrustPolicy::for_this_build(),
                )
                .map_err(|e| rejected(format!("the ruleset is not accepted: {e}")))?;
                let VerifiedUplink::Ruleset(v) = &verified else {
                    return Err(rejected("not a ruleset"));
                };
                if let Some((version, _, stored)) =
                    self.db.latest_config_bundle(RULESET_UPLINK_KIND)?
                {
                    if version == v.ruleset.version && stored.as_bytes() != content {
                        return Err(rejected(
                            "a different ruleset with this version was applied already",
                        ));
                    }
                }
                let text = std::str::from_utf8(content).map_err(|_| rejected("not UTF-8 text"))?;
                let now = self.now()?;
                self.db.put_config_bundle(
                    &format!("{RULESET_UPLINK_KIND}-{}", v.sha256),
                    RULESET_UPLINK_KIND,
                    &v.ruleset.version,
                    envelope,
                    text,
                    now.0,
                )?;
                UplinkResult {
                    version: v.ruleset.version.clone(),
                    at_next_start: true,
                }
            }
            // The other kinds use their own commands' verification, checks and audit; the
            // re-authentication above is still in force for them.
            "protocols" => {
                self.config_apply_protocols(content, envelope)?;
                UplinkResult {
                    version: version_of(content),
                    at_next_start: false,
                }
            }
            "crew" => UplinkResult {
                version: self.config_apply_crew(content, envelope)?.version,
                at_next_start: false,
            },
            "vehicle" => UplinkResult {
                version: self.config_apply_vehicle(content, envelope)?,
                at_next_start: false,
            },
            "ground" => UplinkResult {
                version: self.config_apply_ground(content, envelope)?,
                at_next_start: false,
            },
            "mds" => UplinkResult {
                version: self.config_apply_mds(content, envelope)?,
                at_next_start: false,
            },
            // The fail-over and crew-LAN configuration takes effect at the next start (CR-017 §4).
            "cluster" => UplinkResult {
                version: self.config_apply_cluster(content, envelope)?,
                at_next_start: true,
            },
            other => return Err(rejected(format!("unknown uplink kind {other}"))),
        };
        self.log().append(
            &Actor::user(&user),
            AuditAction::ConfigApplied,
            Some("uplink"),
            &json!({ "uplink_kind": kind, "version": result.version, "at_next_start": result.at_next_start,
                     "sha256": caelum_alarms::signing::digest(content) }),
        )?;
        Ok(result)
    }
}

/// The `version = "..."` of a bundle's first table, for the uplink record.
fn version_of(content: &[u8]) -> String {
    std::str::from_utf8(content)
        .ok()
        .and_then(|t| {
            t.lines().find_map(|l| {
                l.trim()
                    .strip_prefix("version = ")
                    .map(|v| v.trim_matches('"').to_owned())
            })
        })
        .unwrap_or_default()
}
