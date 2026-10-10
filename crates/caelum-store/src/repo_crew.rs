//! Repository for the crew roster (`crew_member`, F-SYS-07, CR-012 §2). Crew members come only
//! from a verified, signed roster bundle; they are never deleted, only made inactive, because
//! their records keep their owner.

use rusqlite::{Row, Transaction, TransactionBehavior};

use crate::db::{Database, StoreError};

/// A row of the `crew_member` table.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CrewMemberRecord {
    /// Crew member ID (UUID text).
    pub id: String,
    /// Name as written in the roster.
    pub name: String,
    /// Crew position (stored in `role`; not a CAELUM access role).
    pub position: String,
    /// Mission.
    pub mission: String,
    /// On the roster in force.
    pub active: bool,
}

fn crew_from_row(r: &Row<'_>) -> rusqlite::Result<CrewMemberRecord> {
    Ok(CrewMemberRecord {
        id: r.get(0)?,
        name: r.get(1)?,
        position: r.get(2)?,
        mission: r.get(3)?,
        active: r.get::<_, i64>(4)? != 0,
    })
}

impl Database {
    /// Replaces the active roster in one transaction: every listed member is inserted or updated
    /// and active; every other member becomes inactive.
    pub fn apply_crew_roster(&self, members: &[CrewMemberRecord]) -> Result<(), StoreError> {
        let tx = Transaction::new_unchecked(self.connection(), TransactionBehavior::Immediate)?;
        tx.execute("UPDATE crew_member SET active = 0", [])?;
        for m in members {
            tx.execute(
                "INSERT INTO crew_member (id, name, role, mission, active) VALUES (?1, ?2, ?3, ?4, 1)
                 ON CONFLICT (id) DO UPDATE SET name = ?2, role = ?3, mission = ?4, active = 1",
                (&m.id, &m.name, &m.position, &m.mission),
            )?;
        }
        tx.commit()?;
        Ok(())
    }

    /// Every crew member, active first, then by name.
    pub fn crew_members(&self) -> Result<Vec<CrewMemberRecord>, StoreError> {
        let conn = self.connection();
        let mut stmt = conn.prepare(
            "SELECT id, name, role, mission, active FROM crew_member ORDER BY active DESC, name, id",
        )?;
        let rows = stmt.query_map([], crew_from_row)?;
        Ok(rows.collect::<Result<_, _>>()?)
    }

    /// One crew member.
    pub fn crew_member(&self, id: &str) -> Result<Option<CrewMemberRecord>, StoreError> {
        Ok(self.crew_members()?.into_iter().find(|c| c.id == id))
    }
}
