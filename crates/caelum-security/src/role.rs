//! Roles, principals and command access (`docs/ROLE_MATRIX.md`, F-SYS-01, R-SEC-02).
//!
//! Every IPC command is checked here before it runs (CLAUDE.md rule 6). The table in
//! [`Command::access`] is tested against `docs/ROLE_MATRIX.md` §3, so the code and the approved
//! matrix cannot drift apart.

use std::fmt;

use caelum_domain::ids::{CrewMemberId, UserId};
use serde::{Deserialize, Serialize};

use crate::audit::AuditCategory;

/// A role (`docs/ROLE_MATRIX.md` §1).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Role {
    /// Astronaut: own data only.
    Astronaut,
    /// Crew Medical Officer: all crew onboard.
    Cmo,
    /// Flight Surgeon (ground): all crew.
    FlightSurgeon,
    /// Biomedical Engineer (ground): devices, technical alarms, data quality.
    Bme,
    /// Mission Control Operator (ground): operational status only.
    Mcc,
    /// Administrator: configuration only, no clinical content.
    Administrator,
    /// Device gateway machine identity: ingest only.
    DeviceGateway,
}

impl Role {
    /// Every role.
    pub const ALL: [Role; 7] = [
        Role::Astronaut,
        Role::Cmo,
        Role::FlightSurgeon,
        Role::Bme,
        Role::Mcc,
        Role::Administrator,
        Role::DeviceGateway,
    ];

    /// The stored name (database `user.role`).
    pub const fn as_str(self) -> &'static str {
        match self {
            Role::Astronaut => "astronaut",
            Role::Cmo => "cmo",
            Role::FlightSurgeon => "flight_surgeon",
            Role::Bme => "bme",
            Role::Mcc => "mcc",
            Role::Administrator => "administrator",
            Role::DeviceGateway => "device_gateway",
        }
    }

    /// Parses a stored name.
    pub fn parse(name: &str) -> Option<Role> {
        Role::ALL.into_iter().find(|r| r.as_str() == name)
    }

    /// Crew roles sign in onboard with a PIN (`docs/ROLE_MATRIX.md` §4) and must belong to a
    /// crew member (their data scope).
    pub const fn is_onboard_crew(self) -> bool {
        matches!(self, Role::Astronaut | Role::Cmo)
    }

    /// Machine identities authenticate with keys, never through the interactive sign-in.
    pub const fn is_machine(self) -> bool {
        matches!(self, Role::DeviceGateway)
    }

    /// Audit entries this role may read (`docs/ROLE_MATRIX.md` §2, "Audit log").
    pub const fn audit_categories(self) -> &'static [AuditCategory] {
        match self {
            Role::Cmo | Role::FlightSurgeon => &AuditCategory::ALL,
            Role::Bme => &[AuditCategory::Technical],
            Role::Administrator => &[AuditCategory::Configuration, AuditCategory::Security],
            Role::Astronaut | Role::Mcc | Role::DeviceGateway => &[],
        }
    }
}

impl fmt::Display for Role {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

/// What a user may do with a physiological alarm (`docs/ROLE_MATRIX.md` §2 "Alarm actions", §5).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AlarmAuthority {
    /// Acknowledge and silence the alarm onboard.
    Onboard,
    /// Acknowledge only the ground escalation stage ("seen by ground"), §5.4.
    GroundStage,
    /// No alarm actions.
    None,
}

/// Alarm actions by role and crew scope: astronauts their own alarms, the CMO anyone's, the flight
/// surgeon the ground stage only; BME, MCC, administrator and device gateway none (§5.1–5.4).
pub fn alarm_authority(user: &UserIdentity, alarm_crew: CrewMemberId) -> AlarmAuthority {
    match user.role {
        Role::Astronaut if user.crew_member_id == Some(alarm_crew) => AlarmAuthority::Onboard,
        Role::Cmo => AlarmAuthority::Onboard,
        Role::FlightSurgeon => AlarmAuthority::GroundStage,
        Role::Astronaut | Role::Bme | Role::Mcc | Role::Administrator | Role::DeviceGateway => {
            AlarmAuthority::None
        }
    }
}

/// A signed-in user.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct UserIdentity {
    /// Account ID.
    pub user_id: UserId,
    /// Sign-in name.
    pub username: String,
    /// Role.
    pub role: Role,
    /// The crew member the account belongs to (scope of "own data").
    pub crew_member_id: Option<CrewMemberId>,
}

/// Who is calling a command.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Principal {
    /// Nobody is signed in.
    Anonymous,
    /// The emergency view was opened without sign-in (SR-014).
    BreakGlass,
    /// A signed-in user whose session is locked: only the session commands (identity, unlock by
    /// re-authentication, sign out) are available (`docs/ROLE_MATRIX.md` §4).
    Locked(UserIdentity),
    /// A signed-in, unlocked user.
    User(UserIdentity),
}

/// The IPC commands (`docs/ROLE_MATRIX.md` §3).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Command {
    /// Product name and version.
    AppInfo,
    /// Sign in.
    AuthLogin,
    /// Sign out.
    AuthLogout,
    /// Own identity, role and scope.
    AuthMe,
    /// Re-authentication before sensitive actions; also unlocks a locked session.
    AuthReauth,
    /// Opens the emergency view without sign-in.
    BreakGlassOpen,
    /// Audit entries, filtered by role.
    AuditList,
    /// Audit chain and signature status.
    AuditVerify,
    /// Alarm ruleset in force (SR-009 banner).
    RulesetStatus,
    /// Acknowledge an alarm (onboard, or the ground stage for the flight surgeon).
    AlarmAcknowledge,
    /// Silence an alarm (SR-008).
    AlarmSilence,
    /// Escalate an alarm to its next stage.
    AlarmEscalate,
    /// Whether any account exists (first start).
    SetupStatus,
    /// Create the first administrator (only while no account exists).
    SetupCreateAdministrator,
    /// Crew roster (CR-012 §2): own entry for an astronaut, all for the others.
    CrewList,
    /// Apply a signed crew-roster bundle (F-SYS-07); re-authentication required.
    ConfigApplyCrew,
    /// Accounts: all for CMO, flight surgeon, BME and administrator; self otherwise.
    UserList,
    /// Create an account (CR-007 §4); re-authentication required.
    UserCreate,
    /// Disable an account; re-authentication required.
    UserDisable,
    /// Newest value per parameter for a crew member in scope.
    VitalsLatest,
    /// One parameter over a window, with the personal baseline if any.
    VitalsTrend,
    /// Recent waveform chunks of one signal.
    WaveformRecent,
    /// Active and recent alarms (OPS fields only for MCC).
    AlarmList,
    /// One alarm's why, values, timeline and actions.
    AlarmDetail,
    /// Alarm history, statistics and fatigue indicators (F-ALM-09).
    AlarmReport,
    /// Device registry with link health (F-ACQ-09, F-ACQ-10).
    DeviceList,
    /// Assign a device to a crew member; re-authentication required.
    DeviceAssign,
    /// Remove a device's assignment; re-authentication required.
    DeviceUnassign,
    /// Nearby devices with supported profiles (F-ACQ-01).
    DeviceScan,
    /// Pair and register a device (F-ACQ-01).
    DevicePair,
    /// Every active crew member's top alarm and key vitals (F-CLN-01).
    CrewOverview,
    /// A crew member's encounters.
    EncounterList,
    /// Create or update an unsigned encounter.
    EncounterSave,
    /// Sign an encounter; re-authentication required.
    EncounterSign,
    /// Submit the own daily check-in.
    CheckinSubmit,
    /// Check-ins by privacy tier.
    CheckinList,
    /// Store a PVT run's result.
    PvtSubmit,
    /// The medication and medical-kit log.
    MedicationLog,
    /// Record a dose or kit use or restock.
    MedicationAdd,
    /// The signed protocol library (public, SR-014).
    ProtocolList,
    /// Apply a signed protocol library; re-authentication required.
    ConfigApplyProtocols,
    /// Start a protocol checklist.
    ChecklistStart,
    /// Mark a checklist step or complete the run.
    ChecklistUpdate,
    /// Checklist runs in scope.
    ChecklistList,
    /// Import a file into the encrypted attachment store.
    AttachmentImport,
    /// Attachment metadata.
    AttachmentList,
    /// One decrypted, integrity-checked attachment.
    AttachmentGet,
    /// Apply a signed vehicle MID table; re-authentication required.
    ConfigApplyVehicle,
    /// Vehicle bridge health: datagrams, drops, clock correlation.
    VehicleLinkStatus,
    /// Apply the signed ground configuration; re-authentication required.
    ConfigApplyGround,
    /// Ground and path B link status (counts only, no physiology).
    GroundStatus,
    /// Installed advisory models with validation status (F-ML-01).
    ModelList,
    /// Open the waveform viewer window (F-SYS-13).
    WindowOpenWaveform,
    /// Export a crew member's observations as a FHIR R4 bundle (F-SYS-08).
    ExportFhir,
    /// Export one parameter as CSV (F-SYS-08).
    ExportCsv,
    /// Apply the signed medical data system configuration; re-authentication required.
    ConfigApplyMds,
    /// Set the station's client certificate and key for the medical data system.
    MdsIdentitySet,
    /// Medical data system link status (counts only).
    MdsStatus,
    /// Verify and apply an uplinked ruleset, protocol library or configuration (F-GND-07).
    UplinkApply,
    /// Import observations from a FHIR R4 resource or bundle (F-ACQ-08).
    FhirImport,
    /// An imported ECG recording, decoded for review (F-CLN-03).
    EcgView,
    /// An imported DICOM image, decoded for the viewer (F-CLN-04).
    DicomView,
    /// Apply the signed fail-over and crew-LAN configuration (CR-017 §4).
    ConfigApplyCluster,
    /// Set this station's crew-LAN certificate chain and key (CR-017 §4).
    ClusterIdentitySet,
    /// Apply the signed crew-station configuration (secondary display or kiosk; CR-017 §4).
    ConfigApplyCrewStation,
    /// This crew station: link to the primary and the alarms it streams (F-SYS-05).
    CrewLanStatus,
    /// Sign a user in on the primary from this crew station (F-SYS-05).
    CrewLanSignIn,
    /// Sign the crew-station user out (F-SYS-05).
    CrewLanSignOut,
    /// Apply the signed retention limits (F-SYS-08, CR-022).
    ConfigApplyRetention,
    /// Apply the signed DICOM folder-watch configuration (F-ACQ-06, CR-022).
    ConfigApplyImport,
    /// Apply the signed check schedule (F-CLN-06, F-CLN-07; CR-022).
    ConfigApplySchedule,
    /// The next scheduled check (F-CLN-06, F-CLN-07).
    ScheduleNext,
    /// Export a segment of an imported ECG recording as EDF+ (F-CLN-03).
    EcgExportSegment,
    /// Apply the signed contact-window schedule (F-GND-05, CR-024).
    ConfigApplyContact,
    /// Make this installation a ground console (F-GND-08, CR-025).
    ConfigApplyConsole,
    /// Downlinked MEDICAL summaries of a crew member (F-GND-08).
    ConsoleSummaries,
    /// Files received by CFDP (event packets; F-GND-08).
    ConsoleFiles,
    /// Downlinked OPS records (F-GND-08).
    ConsoleOps,
    /// Draft a private medical conference record (F-GND-08).
    PmcSave,
    /// Sign a private medical conference record (F-GND-08).
    PmcSign,
    /// Private medical conference records of a crew member (F-GND-08).
    PmcList,
    /// The content of a received file (F-GND-08).
    ConsoleFile,
}

/// Who may call a command.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Access {
    /// Anyone, without sign-in.
    Public,
    /// Any signed-in user, also while the session is locked (session commands only).
    AnySignedIn,
    /// Signed-in users with one of these roles.
    Roles(&'static [Role]),
}

impl Command {
    /// Every command.
    pub const ALL: [Command; 82] = [
        Command::AppInfo,
        Command::AuthLogin,
        Command::AuthLogout,
        Command::AuthMe,
        Command::AuthReauth,
        Command::BreakGlassOpen,
        Command::AuditList,
        Command::AuditVerify,
        Command::RulesetStatus,
        Command::AlarmAcknowledge,
        Command::AlarmSilence,
        Command::AlarmEscalate,
        Command::SetupStatus,
        Command::SetupCreateAdministrator,
        Command::CrewList,
        Command::ConfigApplyCrew,
        Command::UserList,
        Command::UserCreate,
        Command::UserDisable,
        Command::VitalsLatest,
        Command::VitalsTrend,
        Command::WaveformRecent,
        Command::AlarmList,
        Command::AlarmDetail,
        Command::AlarmReport,
        Command::DeviceList,
        Command::DeviceAssign,
        Command::DeviceUnassign,
        Command::DeviceScan,
        Command::DevicePair,
        Command::CrewOverview,
        Command::EncounterList,
        Command::EncounterSave,
        Command::EncounterSign,
        Command::CheckinSubmit,
        Command::CheckinList,
        Command::PvtSubmit,
        Command::MedicationLog,
        Command::MedicationAdd,
        Command::ProtocolList,
        Command::ConfigApplyProtocols,
        Command::ChecklistStart,
        Command::ChecklistUpdate,
        Command::ChecklistList,
        Command::AttachmentImport,
        Command::AttachmentList,
        Command::AttachmentGet,
        Command::ConfigApplyVehicle,
        Command::VehicleLinkStatus,
        Command::ConfigApplyGround,
        Command::GroundStatus,
        Command::ModelList,
        Command::WindowOpenWaveform,
        Command::ExportFhir,
        Command::ExportCsv,
        Command::ConfigApplyMds,
        Command::MdsIdentitySet,
        Command::MdsStatus,
        Command::UplinkApply,
        Command::FhirImport,
        Command::EcgView,
        Command::DicomView,
        Command::ConfigApplyCluster,
        Command::ClusterIdentitySet,
        Command::ConfigApplyCrewStation,
        Command::CrewLanStatus,
        Command::CrewLanSignIn,
        Command::CrewLanSignOut,
        Command::ConfigApplyRetention,
        Command::ConfigApplyImport,
        Command::ConfigApplySchedule,
        Command::ScheduleNext,
        Command::EcgExportSegment,
        Command::ConfigApplyContact,
        Command::ConfigApplyConsole,
        Command::ConsoleSummaries,
        Command::ConsoleFiles,
        Command::ConsoleOps,
        Command::PmcSave,
        Command::PmcSign,
        Command::PmcList,
        Command::ConsoleFile,
    ];

    /// The IPC command name.
    pub const fn name(self) -> &'static str {
        match self {
            Command::AppInfo => "app_info",
            Command::AuthLogin => "auth_login",
            Command::AuthLogout => "auth_logout",
            Command::AuthMe => "auth_me",
            Command::AuthReauth => "auth_reauth",
            Command::BreakGlassOpen => "break_glass_open",
            Command::AuditList => "audit_list",
            Command::AuditVerify => "audit_verify",
            Command::RulesetStatus => "ruleset_status",
            Command::AlarmAcknowledge => "alarm_acknowledge",
            Command::AlarmSilence => "alarm_silence",
            Command::AlarmEscalate => "alarm_escalate",
            Command::SetupStatus => "setup_status",
            Command::SetupCreateAdministrator => "setup_create_administrator",
            Command::CrewList => "crew_list",
            Command::ConfigApplyCrew => "config_apply_crew",
            Command::UserList => "user_list",
            Command::UserCreate => "user_create",
            Command::UserDisable => "user_disable",
            Command::VitalsLatest => "vitals_latest",
            Command::VitalsTrend => "vitals_trend",
            Command::WaveformRecent => "waveform_recent",
            Command::AlarmList => "alarm_list",
            Command::AlarmDetail => "alarm_detail",
            Command::AlarmReport => "alarm_report",
            Command::DeviceList => "device_list",
            Command::DeviceAssign => "device_assign",
            Command::DeviceUnassign => "device_unassign",
            Command::DeviceScan => "device_scan",
            Command::DevicePair => "device_pair",
            Command::CrewOverview => "crew_overview",
            Command::EncounterList => "encounter_list",
            Command::EncounterSave => "encounter_save",
            Command::EncounterSign => "encounter_sign",
            Command::CheckinSubmit => "checkin_submit",
            Command::CheckinList => "checkin_list",
            Command::PvtSubmit => "pvt_submit",
            Command::MedicationLog => "medication_log",
            Command::MedicationAdd => "medication_add",
            Command::ProtocolList => "protocol_list",
            Command::ConfigApplyProtocols => "config_apply_protocols",
            Command::ChecklistStart => "checklist_start",
            Command::ChecklistUpdate => "checklist_update",
            Command::ChecklistList => "checklist_list",
            Command::AttachmentImport => "attachment_import",
            Command::AttachmentList => "attachment_list",
            Command::AttachmentGet => "attachment_get",
            Command::ConfigApplyVehicle => "config_apply_vehicle",
            Command::VehicleLinkStatus => "vehicle_link_status",
            Command::ConfigApplyGround => "config_apply_ground",
            Command::GroundStatus => "ground_status",
            Command::ModelList => "model_list",
            Command::WindowOpenWaveform => "window_open_waveform",
            Command::ExportFhir => "export_fhir",
            Command::ExportCsv => "export_csv",
            Command::ConfigApplyMds => "config_apply_mds",
            Command::MdsIdentitySet => "mds_identity_set",
            Command::MdsStatus => "mds_status",
            Command::UplinkApply => "uplink_apply",
            Command::FhirImport => "fhir_import",
            Command::EcgView => "ecg_view",
            Command::DicomView => "dicom_view",
            Command::ConfigApplyCluster => "config_apply_cluster",
            Command::ClusterIdentitySet => "cluster_identity_set",
            Command::ConfigApplyCrewStation => "config_apply_crew_station",
            Command::CrewLanStatus => "crew_lan_status",
            Command::CrewLanSignIn => "crew_lan_sign_in",
            Command::CrewLanSignOut => "crew_lan_sign_out",
            Command::ConfigApplyRetention => "config_apply_retention",
            Command::ConfigApplyImport => "config_apply_import",
            Command::ConfigApplySchedule => "config_apply_schedule",
            Command::ScheduleNext => "schedule_next",
            Command::EcgExportSegment => "ecg_export_segment",
            Command::ConfigApplyContact => "config_apply_contact",
            Command::ConfigApplyConsole => "config_apply_console",
            Command::ConsoleSummaries => "console_summaries",
            Command::ConsoleFiles => "console_files",
            Command::ConsoleOps => "console_ops",
            Command::PmcSave => "pmc_save",
            Command::PmcSign => "pmc_sign",
            Command::PmcList => "pmc_list",
            Command::ConsoleFile => "console_file",
        }
    }

    /// Looks a command up by its IPC name.
    pub fn from_name(name: &str) -> Option<Command> {
        Command::ALL.into_iter().find(|c| c.name() == name)
    }

    /// The access rule (`docs/ROLE_MATRIX.md` §3).
    pub const fn access(self) -> Access {
        match self {
            Command::AppInfo
            | Command::AuthLogin
            | Command::BreakGlassOpen
            | Command::RulesetStatus
            // CR-007 §4: public, and the station refuses it once any account exists.
            | Command::SetupStatus
            | Command::SetupCreateAdministrator => Access::Public,
            Command::AuthLogout | Command::AuthMe | Command::AuthReauth => Access::AnySignedIn,
            Command::AuditList => Access::Roles(&[
                Role::Cmo,
                Role::FlightSurgeon,
                Role::Bme,
                Role::Administrator,
            ]),
            Command::AuditVerify => Access::Roles(&[
                Role::Cmo,
                Role::FlightSurgeon,
                Role::Bme,
                Role::Administrator,
            ]),
            // Crew scope (own / all / ground stage) is checked per alarm: `alarm_authority`.
            Command::AlarmAcknowledge => {
                Access::Roles(&[Role::Astronaut, Role::Cmo, Role::FlightSurgeon])
            }
            Command::AlarmSilence | Command::AlarmEscalate => {
                Access::Roles(&[Role::Astronaut, Role::Cmo])
            }
            // CR-012 §2–4. Crew scope (own / all) and OPS-only views are applied per call.
            Command::CrewList => Access::Roles(&[
                Role::Astronaut,
                Role::Cmo,
                Role::FlightSurgeon,
                Role::Bme,
                Role::Administrator,
            ]),
            Command::ConfigApplyCrew | Command::UserCreate | Command::UserDisable => {
                Access::Roles(&[Role::Administrator])
            }
            Command::UserList => Access::Roles(&[
                Role::Astronaut,
                Role::Cmo,
                Role::FlightSurgeon,
                Role::Bme,
                Role::Mcc,
                Role::Administrator,
            ]),
            Command::VitalsLatest => Access::Roles(&[Role::Astronaut, Role::Cmo, Role::FlightSurgeon, Role::Bme]),
            Command::VitalsTrend => Access::Roles(&[Role::Astronaut, Role::Cmo, Role::FlightSurgeon, Role::Bme]),
            Command::WaveformRecent => Access::Roles(&[Role::Astronaut, Role::Cmo, Role::FlightSurgeon, Role::Bme]),
            Command::AlarmList => Access::Roles(&[Role::Astronaut, Role::Cmo, Role::FlightSurgeon, Role::Bme, Role::Mcc]),
            Command::AlarmDetail => Access::Roles(&[Role::Astronaut, Role::Cmo, Role::FlightSurgeon, Role::Bme]),
            Command::AlarmReport => Access::Roles(&[Role::Cmo, Role::FlightSurgeon, Role::Bme]),
            Command::DeviceList => Access::Roles(&[Role::Astronaut, Role::Cmo, Role::FlightSurgeon, Role::Bme, Role::Administrator]),
            Command::DeviceAssign => Access::Roles(&[Role::Cmo, Role::Bme, Role::Administrator]),
            Command::DeviceUnassign => Access::Roles(&[Role::Cmo, Role::Bme, Role::Administrator]),
            Command::DeviceScan => Access::Roles(&[Role::Cmo, Role::Bme, Role::Administrator]),
            Command::DevicePair => Access::Roles(&[Role::Cmo, Role::Bme, Role::Administrator]),
            Command::CrewOverview => Access::Roles(&[Role::Cmo, Role::FlightSurgeon]),
            Command::EncounterList => Access::Roles(&[Role::Astronaut, Role::Cmo, Role::FlightSurgeon]),
            Command::EncounterSave => Access::Roles(&[Role::Cmo, Role::FlightSurgeon]),
            Command::EncounterSign => Access::Roles(&[Role::Cmo, Role::FlightSurgeon]),
            Command::CheckinSubmit => Access::Roles(&[Role::Astronaut]),
            Command::CheckinList => Access::Roles(&[Role::Astronaut, Role::Cmo, Role::FlightSurgeon]),
            Command::PvtSubmit => Access::Roles(&[Role::Astronaut]),
            Command::MedicationLog => Access::Roles(&[Role::Astronaut, Role::Cmo, Role::FlightSurgeon]),
            Command::MedicationAdd => Access::Roles(&[Role::Cmo]),
            Command::ProtocolList => Access::Public,
            Command::ConfigApplyProtocols => Access::Roles(&[Role::Administrator]),
            Command::ChecklistStart => Access::Roles(&[Role::Astronaut, Role::Cmo]),
            Command::ChecklistUpdate => Access::Roles(&[Role::Astronaut, Role::Cmo]),
            Command::ChecklistList => Access::Roles(&[Role::Astronaut, Role::Cmo, Role::FlightSurgeon]),
            Command::AttachmentImport => Access::Roles(&[Role::Cmo, Role::FlightSurgeon]),
            Command::AttachmentList => Access::Roles(&[Role::Cmo, Role::FlightSurgeon]),
            Command::AttachmentGet => Access::Roles(&[Role::Cmo, Role::FlightSurgeon]),
            Command::ConfigApplyVehicle => Access::Roles(&[Role::Administrator]),
            Command::VehicleLinkStatus => Access::Roles(&[Role::Cmo, Role::FlightSurgeon, Role::Bme, Role::Administrator]),
            Command::ConfigApplyGround => Access::Roles(&[Role::Administrator]),
            Command::GroundStatus => Access::Roles(&[Role::Astronaut, Role::Cmo, Role::FlightSurgeon, Role::Bme, Role::Mcc, Role::Administrator]),
            Command::ModelList => Access::Roles(&[Role::Astronaut, Role::Cmo, Role::FlightSurgeon, Role::Bme]),
            Command::WindowOpenWaveform => Access::Roles(&[Role::Astronaut, Role::Cmo, Role::FlightSurgeon, Role::Bme]),
            Command::ExportFhir => Access::Roles(&[Role::Cmo, Role::FlightSurgeon]),
            Command::ExportCsv => Access::Roles(&[Role::Cmo, Role::FlightSurgeon, Role::Bme]),
            Command::ConfigApplyMds => Access::Roles(&[Role::Administrator]),
            Command::MdsIdentitySet => Access::Roles(&[Role::Administrator]),
            Command::MdsStatus => Access::Roles(&[Role::Cmo, Role::FlightSurgeon, Role::Bme, Role::Administrator]),
            Command::UplinkApply => Access::Roles(&[Role::Administrator]),
            Command::FhirImport => Access::Roles(&[Role::Cmo, Role::FlightSurgeon]),
            Command::EcgView => Access::Roles(&[Role::Cmo, Role::FlightSurgeon]),
            Command::DicomView => Access::Roles(&[Role::Cmo, Role::FlightSurgeon]),
            Command::ConfigApplyCluster => Access::Roles(&[Role::Administrator]),
            Command::ClusterIdentitySet => Access::Roles(&[Role::Administrator]),
            Command::ConfigApplyCrewStation => Access::Roles(&[Role::Administrator]),
            Command::CrewLanStatus => Access::Public,
            Command::CrewLanSignIn => Access::Public,
            Command::CrewLanSignOut => Access::Public,
            Command::ConfigApplyRetention => Access::Roles(&[Role::Administrator]),
            Command::ConfigApplyImport => Access::Roles(&[Role::Administrator]),
            Command::ConfigApplySchedule => Access::Roles(&[Role::Administrator]),
            Command::ScheduleNext => Access::Roles(&[Role::Astronaut, Role::Cmo, Role::FlightSurgeon]),
            Command::EcgExportSegment => Access::Roles(&[Role::Cmo, Role::FlightSurgeon]),
            Command::ConfigApplyContact => Access::Roles(&[Role::Administrator]),
            Command::ConfigApplyConsole => Access::Roles(&[Role::Administrator]),
            Command::ConsoleSummaries => Access::Roles(&[Role::FlightSurgeon]),
            Command::ConsoleFiles => Access::Roles(&[Role::FlightSurgeon]),
            Command::ConsoleOps => Access::Roles(&[Role::FlightSurgeon, Role::Bme, Role::Mcc]),
            Command::PmcSave => Access::Roles(&[Role::FlightSurgeon]),
            Command::PmcSign => Access::Roles(&[Role::FlightSurgeon]),
            Command::PmcList => Access::Roles(&[Role::FlightSurgeon]),
            Command::ConsoleFile => Access::Roles(&[Role::FlightSurgeon]),
        }
    }

    /// Checks that `principal` may call this command.
    pub fn authorize(self, principal: &Principal) -> Result<(), Denied> {
        let allowed = match (self.access(), principal) {
            (Access::Public, _) => true,
            (Access::AnySignedIn, Principal::User(_) | Principal::Locked(_)) => true,
            (Access::Roles(roles), Principal::User(u)) => roles.contains(&u.role),
            (Access::AnySignedIn | Access::Roles(_), _) => false,
        };
        if allowed {
            Ok(())
        } else {
            Err(Denied {
                command: self,
                reason: match principal {
                    Principal::User(_) => DeniedReason::Role,
                    Principal::Locked(_) => DeniedReason::Locked,
                    Principal::Anonymous | Principal::BreakGlass => DeniedReason::SignInRequired,
                },
            })
        }
    }
}

/// A refused command.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
#[error("{} is not permitted: {}", command.name(), reason.text())]
pub struct Denied {
    /// The command.
    pub command: Command,
    /// Why.
    pub reason: DeniedReason,
}

/// Why a command was refused.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DeniedReason {
    /// Nobody is signed in.
    SignInRequired,
    /// The session is locked; unlock first.
    Locked,
    /// The signed-in role may not use this command.
    Role,
}

impl DeniedReason {
    const fn text(self) -> &'static str {
        match self {
            DeniedReason::SignInRequired => "sign-in required",
            DeniedReason::Locked => "the session is locked",
            DeniedReason::Role => "not available for this role",
        }
    }
}
