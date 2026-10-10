//! Repositories for the security tables: `user` and `audit_entry` (F-SYS-01, F-SYS-03).
//!
//! Rows are plain records; `caelum-security` owns their meaning (roles, credentials, hashing and
//! signing). Audit entries are only ever appended, inside an immediate transaction so the chain
//! position and the previous hash cannot change between reading and writing.

use rusqlite::{OptionalExtension, Row, Transaction, TransactionBehavior};

use crate::db::{Database, StoreError};

/// A row of the `user` table.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UserRecord {
    /// User ID (UUID text).
    pub id: String,
    /// Unique sign-in name.
    pub username: String,
    /// Role as stored (`docs/ROLE_MATRIX.md` §1).
    pub role: String,
    /// Crew member this account belongs to (astronaut and CMO accounts).
    pub crew_member_id: Option<String>,
    /// argon2id PHC string.
    pub credential: String,
    /// Identifier of a device-bound key, when one is enrolled.
    pub key_id: Option<String>,
    /// Disabled accounts cannot sign in.
    pub disabled: bool,
}

/// A row of the `audit_entry` table.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AuditRecord {
    /// Position in the chain, starting at 1.
    pub seq: i64,
    /// Entry ID (UUID text).
    pub id: String,
    /// TAI nanoseconds.
    pub at_tai: i64,
    /// Who acted (user ID, or a fixed actor such as `break-glass`).
    pub actor: String,
    /// The actor's role at the time.
    pub role: String,
    /// Action name.
    pub action: String,
    /// What the action applied to.
    pub target: Option<String>,
    /// JSON details.
    pub details: String,
    /// Hash of the previous entry (hex).
    pub prev_hash: String,
    /// Hash of this entry (hex).
    pub hash: String,
    /// Signature over the hash (hex).
    pub signature: String,
}

/// The chain head an appended entry links to.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ChainHead {
    /// Sequence number of the last entry.
    pub seq: i64,
    /// Hash of the last entry (hex).
    pub hash: String,
}

fn user_from_row(r: &Row<'_>) -> rusqlite::Result<UserRecord> {
    Ok(UserRecord {
        id: r.get(0)?,
        username: r.get(1)?,
        role: r.get(2)?,
        crew_member_id: r.get(3)?,
        credential: r.get(4)?,
        key_id: r.get(5)?,
        disabled: r.get::<_, i64>(6)? != 0,
    })
}

fn audit_from_row(r: &Row<'_>) -> rusqlite::Result<AuditRecord> {
    Ok(AuditRecord {
        seq: r.get(0)?,
        id: r.get(1)?,
        at_tai: r.get(2)?,
        actor: r.get(3)?,
        role: r.get(4)?,
        action: r.get(5)?,
        target: r.get(6)?,
        details: r.get(7)?,
        prev_hash: r.get(8)?,
        hash: r.get(9)?,
        signature: r.get(10)?,
    })
}

const USER_COLUMNS: &str = "id, username, role, crew_member_id, credential, key_id, disabled";
const AUDIT_COLUMNS: &str =
    "seq, id, at_tai, actor, role, action, target, details, prev_hash, hash, signature";

impl Database {
    /// Adds a user account.
    pub fn insert_user(&self, user: &UserRecord) -> Result<(), StoreError> {
        self.connection().execute(
            "INSERT INTO user (id, username, role, crew_member_id, credential, key_id, disabled)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
            (
                &user.id,
                &user.username,
                &user.role,
                &user.crew_member_id,
                &user.credential,
                &user.key_id,
                i64::from(user.disabled),
            ),
        )?;
        Ok(())
    }

    /// The account with this sign-in name, if any.
    pub fn user_by_username(&self, username: &str) -> Result<Option<UserRecord>, StoreError> {
        let user = self
            .connection()
            .query_row(
                &format!("SELECT {USER_COLUMNS} FROM user WHERE username = ?1"),
                [username],
                user_from_row,
            )
            .optional()?;
        Ok(user)
    }

    /// Number of user accounts.
    pub fn user_count(&self) -> Result<i64, StoreError> {
        Ok(self
            .connection()
            .query_row("SELECT count(*) FROM user", [], |r| r.get(0))?)
    }

    /// Appends one audit entry. `build` receives the current chain head (`None` for the first
    /// entry) and returns the complete record to insert; reading the head and inserting happen in
    /// one immediate transaction.
    pub fn append_audit(
        &self,
        build: impl FnOnce(Option<&ChainHead>) -> AuditRecord,
    ) -> Result<AuditRecord, StoreError> {
        let tx = Transaction::new_unchecked(self.connection(), TransactionBehavior::Immediate)?;
        let head = tx
            .query_row(
                "SELECT seq, hash FROM audit_entry ORDER BY seq DESC LIMIT 1",
                [],
                |r| {
                    Ok(ChainHead {
                        seq: r.get(0)?,
                        hash: r.get(1)?,
                    })
                },
            )
            .optional()?;
        let record = build(head.as_ref());
        tx.execute(
            &format!(
                "INSERT INTO audit_entry ({AUDIT_COLUMNS})
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11)"
            ),
            rusqlite::params![
                record.seq,
                record.id,
                record.at_tai,
                record.actor,
                record.role,
                record.action,
                record.target,
                record.details,
                record.prev_hash,
                record.hash,
                record.signature,
            ],
        )?;
        tx.commit()?;
        Ok(record)
    }

    /// Every audit entry in chain order.
    pub fn audit_records(&self) -> Result<Vec<AuditRecord>, StoreError> {
        let mut stmt = self.connection().prepare(&format!(
            "SELECT {AUDIT_COLUMNS} FROM audit_entry ORDER BY seq"
        ))?;
        let rows = stmt
            .query_map([], audit_from_row)?
            .collect::<Result<Vec<_>, _>>()?;
        Ok(rows)
    }
}
