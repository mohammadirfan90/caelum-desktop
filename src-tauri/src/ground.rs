//! Ground link, path B and model status commands (M10/M12 integration; F-GND-11, SR-015,
//! F-ML-01; CR-016, CR-018). Counts and states only: no physiology (docs/06 §3a).

use caelum_core::ml::advisory::DisplayStatus;
use caelum_core::ml::manifest::ValidationStatus;
use serde::Serialize;
use specta::Type;
use tauri::State;

use crate::cluster::ClusterState;
use crate::commands::CommandError;
use crate::state::AppState;
use crate::vehicle::{VehicleLink, VehicleState};

/// Path B and vehicle-bridge status.
#[derive(Debug, Clone, PartialEq, Serialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct GroundStatus {
    /// A signed ground configuration is in force and path B is open.
    pub path_b_configured: bool,
    /// Ground configuration version.
    pub ground_version: Option<String>,
    /// OPS records waiting for the vehicle endpoint.
    pub path_b_waiting: u32,
    /// Why path B is not running.
    pub path_b_problem: Option<String>,
    /// Vehicle bridge (inbound) health.
    pub vehicle: VehicleLink,
    /// Fail-over role of this instance (F-SYS-04, docs/integration/M11.md): `standalone` without
    /// a signed cluster configuration, else `primary` / `promoting` / `standby`, or `unavailable`
    /// when the configured instance could not start (`technical.cluster` says why).
    pub instance_role: String,
    /// Ground downlink (MEDICAL summaries and event packets) configured.
    pub downlink_configured: bool,
    /// Inside a contact window now.
    pub downlink_in_contact: bool,
    /// Items waiting for a contact window.
    pub downlink_waiting: u32,
}

/// Ground and path B status (all signed-in roles; counts only).
#[tauri::command]
#[specta::specta]
pub async fn ground_status(
    state: State<'_, AppState>,
    vehicle: State<'_, VehicleState>,
    cluster: State<'_, ClusterState>,
) -> Result<GroundStatus, CommandError> {
    let (p, d) = state.with(|s| {
        s.ground_status_allowed()?;
        Ok((s.path_b_status(), s.downlink_status()))
    })?;
    let vehicle = vehicle.snapshot();
    Ok(GroundStatus {
        path_b_configured: p.configured,
        ground_version: p.version,
        path_b_waiting: u32::try_from(p.waiting).unwrap_or(u32::MAX),
        path_b_problem: p.problem,
        vehicle,
        instance_role: cluster.role(),
        downlink_configured: d.configured,
        downlink_in_contact: d.in_contact,
        downlink_waiting: u32::try_from(d.waiting).unwrap_or(u32::MAX),
    })
}

/// Applies the signed ground configuration (administrator, re-authentication).
#[tauri::command]
#[specta::specta]
pub async fn config_apply_ground(
    state: State<'_, AppState>,
    bundle: String,
    signature: String,
) -> Result<String, CommandError> {
    state.with(|s| Ok(s.config_apply_ground(bundle.as_bytes(), &signature)?))
}

/// An installed advisory model.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct ModelItem {
    /// Model ID.
    pub id: String,
    /// Name.
    pub name: String,
    /// Version.
    pub version: String,
    /// `VALIDATED` / `NOT_VALIDATED` / `RESEARCH`.
    pub validation: String,
    /// `VALIDATED` / `INVESTIGATIONAL` / `DISABLED` / `UNAVAILABLE`.
    pub status: String,
    /// Why disabled or unavailable.
    pub reason: Option<String>,
    /// Signed with development keys only.
    pub development_signed: bool,
    /// Intended use.
    pub intended_use: String,
    /// Limitations.
    pub limitations: String,
}

/// Installed advisory models with their validation status (F-ML-01).
#[tauri::command]
#[specta::specta]
pub async fn model_list(state: State<'_, AppState>) -> Result<Vec<ModelItem>, CommandError> {
    state.with(|s| {
        Ok(s.model_list()?
            .into_iter()
            .map(|m| ModelItem {
                id: m.id,
                name: m.name,
                version: m.version,
                validation: match m.validation {
                    ValidationStatus::Validated => "VALIDATED",
                    ValidationStatus::NotValidated => "NOT_VALIDATED",
                    ValidationStatus::Research => "RESEARCH",
                }
                .to_owned(),
                status: match m.status {
                    DisplayStatus::Validated => "VALIDATED",
                    DisplayStatus::Investigational => "INVESTIGATIONAL",
                    DisplayStatus::Disabled => "DISABLED",
                    DisplayStatus::Unavailable => "UNAVAILABLE",
                }
                .to_owned(),
                reason: m.reason,
                development_signed: m.development_signed,
                intended_use: m.intended_use,
                limitations: m.limitations,
            })
            .collect())
    })
}

/// Applies the signed contact-window schedule (administrator, re-authentication; CR-024).
#[tauri::command]
#[specta::specta]
pub async fn config_apply_contact(
    state: State<'_, AppState>,
    bundle: String,
    signature: String,
) -> Result<String, CommandError> {
    state.with(|s| Ok(s.config_apply_contact(bundle.as_bytes(), &signature)?))
}
