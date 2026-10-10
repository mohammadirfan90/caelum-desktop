//! Station side of the fail-over pair and crew LAN (F-SYS-04, F-SYS-05; CR-017 §4): the signed
//! `cluster` configuration and this station's crew-LAN identity, both set by an administrator
//! after re-authentication and audited. They take effect when the application next starts.

use caelum_alarms::signing::{built_in_keys, verify_config, TrustPolicy};
use caelum_security::audit::{Actor, AuditAction};
use caelum_security::role::Command;
use serde_json::json;

use super::{Station, StationError};
use crate::cluster::{ClusterConfig, CLUSTER_KIND, IDENTITY_CHAIN, IDENTITY_KEY};

/// Technical alarm of the fail-over pair (CR-011 table).
pub const CLUSTER_RULE: &str = "technical.cluster";

fn rejected(reason: impl Into<String>) -> StationError {
    StationError::Rejected(reason.into())
}

impl Station {
    /// `config_apply_cluster`: the signed configuration of the pair and the crew stations.
    pub fn config_apply_cluster(
        &mut self,
        bundle: &[u8],
        signature: &str,
    ) -> Result<String, StationError> {
        let user = self.admin_for(Command::ConfigApplyCluster)?;
        let verified = verify_config(
            CLUSTER_KIND,
            bundle,
            signature,
            &built_in_keys(),
            TrustPolicy::for_this_build(),
        )
        .map_err(|e| {
            rejected(format!(
                "the cluster configuration signature is not accepted: {e}"
            ))
        })?;
        let text = std::str::from_utf8(bundle).map_err(|_| rejected("not UTF-8 text"))?;
        let plan = ClusterConfig::parse(text)
            .map_err(|e| rejected(format!("the cluster configuration is invalid: {e}")))?;
        if plan.config.version != verified.version {
            return Err(rejected(
                "the configuration version is not the signed version",
            ));
        }
        let now = self.now()?;
        self.db.put_config_bundle(
            &format!("{CLUSTER_KIND}-{}", verified.sha256),
            CLUSTER_KIND,
            &verified.version,
            signature,
            text,
            now.0,
        )?;
        self.log().append(
            &Actor::user(&user),
            AuditAction::ConfigApplied,
            Some(CLUSTER_KIND),
            &json!({ "kind": CLUSTER_KIND, "version": verified.version, "sha256": verified.sha256 }),
        )?;
        Ok(verified.version)
    }

    /// `cluster_identity_set`: this station's crew-LAN certificate chain and private key (PEM),
    /// kept in the encrypted records. Audited without the key.
    pub fn cluster_identity_set(
        &mut self,
        chain_pem: &str,
        key_pem: &str,
    ) -> Result<(), StationError> {
        let user = self.admin_for(Command::ClusterIdentitySet)?;
        if !chain_pem.contains("BEGIN CERTIFICATE") || !key_pem.contains("PRIVATE KEY") {
            return Err(rejected(
                "expected a PEM certificate chain and a PEM private key",
            ));
        }
        self.db.cluster_set_state(IDENTITY_CHAIN, chain_pem)?;
        self.db.cluster_set_state(IDENTITY_KEY, key_pem)?;
        let digest = caelum_alarms::signing::digest(chain_pem.as_bytes());
        self.log().append(
            &Actor::user(&user),
            AuditAction::ConfigApplied,
            Some("cluster-identity"),
            &json!({ "chain_sha256": digest }),
        )?;
        Ok(())
    }

    /// Raises (`Some(reason)`) or clears (`None`) `technical.cluster` (station-wide).
    pub fn cluster_technical(&mut self, reason: Option<&str>) -> Result<(), StationError> {
        if reason.is_some() == self.alarms.technical_open(CLUSTER_RULE, None) {
            return Ok(());
        }
        let now = self.now()?;
        let why = reason.map(|r| json!({ "cluster": r }));
        self.alarms
            .technical(&self.db, CLUSTER_RULE, None, why, now)?;
        Ok(())
    }
}
