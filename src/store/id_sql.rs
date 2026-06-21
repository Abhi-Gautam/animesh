//! SQLite conversion glue for domain identifiers.
//!
//! Keep rusqlite-specific trait impls in `store/` so `ids.rs` remains pure
//! domain identity code.

use rusqlite::types::{FromSql, FromSqlError, FromSqlResult, ToSqlOutput, ValueRef};
use rusqlite::ToSql;

use crate::ids::CanonicalId;

impl ToSql for CanonicalId {
    fn to_sql(&self) -> rusqlite::Result<ToSqlOutput<'_>> {
        Ok(ToSqlOutput::Borrowed(ValueRef::Text(
            self.as_str().as_bytes(),
        )))
    }
}

impl FromSql for CanonicalId {
    fn column_result(value: ValueRef<'_>) -> FromSqlResult<Self> {
        let s = <String as FromSql>::column_result(value)?;
        CanonicalId::parse(&s).map_err(|e| {
            FromSqlError::Other(Box::new(std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                e.to_string(),
            )))
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ids::ReleaseKind;

    #[test]
    fn rusqlite_round_trip() {
        let conn = rusqlite::Connection::open_in_memory().unwrap();
        conn.execute_batch("CREATE TABLE t (id TEXT PRIMARY KEY)")
            .unwrap();
        let id = CanonicalId::new(ReleaseKind::Tv, "severance").unwrap();
        conn.execute("INSERT INTO t VALUES (?1)", rusqlite::params![id])
            .unwrap();
        let back: CanonicalId = conn
            .query_row("SELECT id FROM t", [], |r| r.get(0))
            .unwrap();
        assert_eq!(back, id);
    }

    #[test]
    fn rusqlite_load_rejects_malformed_id_from_db() {
        let conn = rusqlite::Connection::open_in_memory().unwrap();
        conn.execute_batch("CREATE TABLE t (id TEXT PRIMARY KEY)")
            .unwrap();
        conn.execute("INSERT INTO t VALUES ('not-a-canonical-id')", [])
            .unwrap();
        let res: rusqlite::Result<CanonicalId> =
            conn.query_row("SELECT id FROM t", [], |r| r.get(0));
        assert!(res.is_err(), "FromSql must reject malformed id");
    }
}
