//! Row replication for the fail-over pair (F-SYS-04, R-OPS-02; CR-017, V9 migration): the
//! primary captures changed rows of the replicated tables in `cluster_outbox`, reads each row as
//! JSON for a log entry, and the standby applies it with an upsert. Only the tables listed in
//! [`REPLICATED`] and their real columns are touched; table and column names from a payload are
//! checked against that list and the schema before use.

use rusqlite::types::{Value, ValueRef};
use rusqlite::OptionalExtension;
use serde_json::{json, Map, Value as Json};

use crate::db::{Database, StoreError};

/// Replicated tables with their primary-key columns, in dependency order (a row's references
/// come earlier in this list).
pub const REPLICATED: &[(&str, &[&str])] = &[
    ("crew_member", &["id"]),
    ("user", &["id"]),
    ("device", &["id"]),
    ("config_bundle", &["id"]),
    ("observation", &["id"]),
    ("waveform_chunk", &["id"]),
    ("baseline", &["crew_member_id", "parameter"]),
    ("alarm", &["id"]),
    ("alarm_notification", &["alarm_id", "stage"]),
    ("encounter", &["id"]),
    ("attachment", &["id"]),
    ("checklist_run", &["id"]),
    ("checkin", &["id"]),
    ("medication_entry", &["id"]),
    ("audit_entry", &["seq"]),
];

/// Columns of the audit replica besides its origin.
const AUDIT_COLUMNS: [&str; 11] = [
    "seq",
    "id",
    "at_tai",
    "actor",
    "role",
    "action",
    "target",
    "details",
    "prev_hash",
    "hash",
    "signature",
];

fn invalid(why: impl Into<String>) -> StoreError {
    StoreError::Invalid(why.into())
}

fn pk_of(table: &str) -> Result<&'static [&'static str], StoreError> {
    REPLICATED
        .iter()
        .find(|(t, _)| *t == table)
        .map(|(_, pk)| *pk)
        .ok_or_else(|| invalid(format!("table {table} is not replicated")))
}

fn to_json(v: ValueRef<'_>) -> Json {
    match v {
        ValueRef::Null => Json::Null,
        ValueRef::Integer(i) => json!(i),
        ValueRef::Real(f) => json!(f),
        ValueRef::Text(t) => Json::String(String::from_utf8_lossy(t).into_owned()),
        ValueRef::Blob(b) => {
            json!({ "blob": b.iter().map(|x| format!("{x:02x}")).collect::<String>() })
        }
    }
}

fn from_json(v: &Json) -> Result<Value, StoreError> {
    Ok(match v {
        Json::Null => Value::Null,
        Json::Bool(b) => Value::Integer(i64::from(*b)),
        Json::Number(n) => match n.as_i64() {
            Some(i) => Value::Integer(i),
            None => Value::Real(n.as_f64().ok_or_else(|| invalid("number out of range"))?),
        },
        Json::String(s) => Value::Text(s.clone()),
        Json::Object(o) => {
            let hex = o
                .get("blob")
                .and_then(Json::as_str)
                .ok_or_else(|| invalid("an object value must be a blob"))?;
            if hex.len() % 2 != 0 || !hex.is_ascii() {
                return Err(invalid("blob is not hex"));
            }
            let bytes = (0..hex.len())
                .step_by(2)
                .map(|i| u8::from_str_radix(&hex[i..i + 2], 16))
                .collect::<Result<Vec<u8>, _>>()
                .map_err(|_| invalid("blob is not hex"))?;
            Value::Blob(bytes)
        }
        Json::Array(_) => return Err(invalid("arrays are not column values")),
    })
}

fn marks(n: usize) -> String {
    (1..=n)
        .map(|i| format!("?{i}"))
        .collect::<Vec<_>>()
        .join(", ")
}

impl Database {
    /// Turns capture on (the primary) or off.
    pub fn cluster_set_capture(&self, on: bool) -> Result<(), StoreError> {
        self.cluster_set_state("capture", if on { "on" } else { "off" })
    }

    /// Queues every existing row of the replicated tables, in dependency order, once: the first
    /// primary of a pair sends what it held before the pair was formed. Returns whether it ran.
    pub fn cluster_seed_outbox(&self) -> Result<bool, StoreError> {
        if self.cluster_state("seeded")?.as_deref() == Some("yes") {
            return Ok(false);
        }
        for (table, pk) in REPLICATED {
            let key = format!("json_array({})", pk.join(", "));
            let filter = if *table == "config_bundle" {
                " WHERE kind <> 'cluster'"
            } else {
                ""
            };
            let order = pk.join(", ");
            self.connection().execute(
                &format!(
                    "INSERT INTO cluster_outbox (tbl, key) SELECT ?1, {key} FROM \"{table}\"{filter} ORDER BY {order}"
                ),
                [table],
            )?;
        }
        self.cluster_set_state("seeded", "yes")?;
        Ok(true)
    }

    /// Up to `limit` queued changes, oldest first: (outbox seq, table, key JSON).
    pub fn cluster_outbox(&self, limit: i64) -> Result<Vec<(i64, String, String)>, StoreError> {
        let conn = self.connection();
        let mut stmt =
            conn.prepare("SELECT seq, tbl, key FROM cluster_outbox ORDER BY seq LIMIT ?1")?;
        let rows = stmt.query_map([limit], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))?;
        Ok(rows.collect::<Result<_, _>>()?)
    }

    /// Removes queued changes up to `seq` (sent).
    pub fn cluster_outbox_done(&self, seq: i64) -> Result<(), StoreError> {
        self.connection()
            .execute("DELETE FROM cluster_outbox WHERE seq <= ?1", [seq])?;
        Ok(())
    }

    /// The current row of `table` with primary key `key` (a JSON array), as a JSON object;
    /// `None` if it no longer exists.
    pub fn replica_row(
        &self,
        table: &str,
        key: &Json,
    ) -> Result<Option<Map<String, Json>>, StoreError> {
        let pk = pk_of(table)?;
        let values = key
            .as_array()
            .filter(|a| a.len() == pk.len())
            .ok_or_else(|| invalid("key does not match the primary key"))?
            .iter()
            .map(from_json)
            .collect::<Result<Vec<_>, _>>()?;
        let filter = pk
            .iter()
            .enumerate()
            .map(|(i, c)| format!("\"{c}\" = ?{}", i + 1))
            .collect::<Vec<_>>()
            .join(" AND ");
        let conn = self.connection();
        let mut stmt = conn.prepare(&format!("SELECT * FROM \"{table}\" WHERE {filter}"))?;
        let names: Vec<String> = stmt
            .column_names()
            .iter()
            .map(|s| (*s).to_owned())
            .collect();
        Ok(stmt
            .query_row(rusqlite::params_from_iter(values), |r| {
                let mut m = Map::new();
                for (i, n) in names.iter().enumerate() {
                    m.insert(n.clone(), to_json(r.get_ref(i)?));
                }
                Ok(m)
            })
            .optional()?)
    }

    fn columns(&self, table: &str) -> Result<Vec<String>, StoreError> {
        let conn = self.connection();
        let mut stmt = conn.prepare(&format!("PRAGMA table_info(\"{table}\")"))?;
        let rows = stmt.query_map([], |r| r.get::<_, String>(1))?;
        Ok(rows.collect::<Result<_, _>>()?)
    }

    /// Applies a replicated row (insert, or update of the existing row). `audit_entry` rows from
    /// `origin` go to `audit_replica`, never into this station's own chain.
    pub fn replica_apply(
        &self,
        origin: &str,
        table: &str,
        row: &Map<String, Json>,
    ) -> Result<(), StoreError> {
        let pk = pk_of(table)?;
        if table == "audit_entry" {
            let mut params = vec![Value::Text(origin.to_owned())];
            for c in AUDIT_COLUMNS {
                params.push(from_json(row.get(c).unwrap_or(&Json::Null))?);
            }
            self.connection().execute(
                &format!(
                    "INSERT OR IGNORE INTO audit_replica (origin, {}) VALUES ({})",
                    AUDIT_COLUMNS.join(", "),
                    marks(params.len())
                ),
                rusqlite::params_from_iter(params),
            )?;
            return Ok(());
        }
        if pk.iter().any(|p| !row.contains_key(*p)) {
            return Err(invalid("the row lacks its primary key"));
        }
        let known = self.columns(table)?;
        let mut cols = Vec::new();
        let mut params = Vec::new();
        let mut updates = Vec::new();
        for (name, value) in row {
            if !known.iter().any(|k| k == name) {
                return Err(invalid(format!("{table} has no column {name}")));
            }
            cols.push(format!("\"{name}\""));
            params.push(from_json(value)?);
            if !pk.contains(&name.as_str()) {
                updates.push(format!("\"{name}\" = excluded.\"{name}\""));
            }
        }
        let conflict = pk
            .iter()
            .map(|p| format!("\"{p}\""))
            .collect::<Vec<_>>()
            .join(", ");
        let tail = if updates.is_empty() {
            "DO NOTHING".to_owned()
        } else {
            format!("DO UPDATE SET {}", updates.join(", "))
        };
        self.connection().execute(
            &format!(
                "INSERT INTO \"{table}\" ({}) VALUES ({}) ON CONFLICT ({conflict}) {tail}",
                cols.join(", "),
                marks(params.len())
            ),
            rusqlite::params_from_iter(params),
        )?;
        Ok(())
    }

    /// How many audit entries from `origin` this station holds as replicas.
    pub fn audit_replica_count(&self, origin: &str) -> Result<i64, StoreError> {
        Ok(self.connection().query_row(
            "SELECT count(*) FROM audit_replica WHERE origin = ?1",
            [origin],
            |r| r.get(0),
        )?)
    }
}
