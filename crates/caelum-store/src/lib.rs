//! Encrypted local storage: SQLCipher database with the key from the OS keystore, refinery
//! migrations, repositories, retention and export.
//!
//! The key is supplied by `caelum-security` (keystore); this crate never reads or stores it in a
//! plain file (R-SEC-01).
//!
//! Implemented in M1 (F-SYS-02, R-SEC-01); retention and export in M8 and M10 (F-SYS-08).

mod db;
mod key;
mod repo;
mod repo_acquisition;
mod repo_clinical;
mod repo_cluster;
mod repo_crew;
mod repo_mds;
mod repo_reads;

pub use db::{Database, StoreError};
pub use key::DbKey;
pub use repo::{AuditRecord, ChainHead, UserRecord};
pub use repo_acquisition::{AlarmRecord, DeviceRecord, ObservationRecord, OpenAlarmRecord};
pub use repo_clinical::{
    AttachmentRecord, CheckinRecord, ChecklistRunRecord, EncounterRecord, MedicationRecord,
};
pub use repo_cluster::ClusterLogRecord;
pub use repo_crew::CrewMemberRecord;
pub use repo_reads::{AlarmView, BaselineRecord, NotificationRecord, WaveformChunkRecord};
