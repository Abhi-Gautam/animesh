//! Small namespaced key/value store backed by the `kv` table.
//!
//! Used for state that doesn't deserve its own table: last sync
//! timestamps, the most recent sync error, AniList rate-limit
//! headroom snapshot, future config.

use anyhow::{Context, Result};
use rusqlite::{params, OptionalExtension};

use super::Db;

impl Db {
    /// UPSERT a key. `updated_at` is unix seconds.
    pub(crate) fn kv_set(&self, key: &str, value: &str, updated_at: i64) -> Result<()> {
        self.conn()
            .execute(
                "INSERT INTO kv(key, value, updated_at) VALUES (?1, ?2, ?3) \
                 ON CONFLICT(key) DO UPDATE SET \
                    value = excluded.value, \
                    updated_at = excluded.updated_at",
                params![key, value, updated_at],
            )
            .with_context(|| format!("kv_set {key}"))?;
        Ok(())
    }

    /// Get a value + its `updated_at`. `None` if the key was never set.
    pub(crate) fn kv_get(&self, key: &str) -> Result<Option<(String, i64)>> {
        self.conn()
            .query_row(
                "SELECT value, updated_at FROM kv WHERE key = ?1",
                params![key],
                |row| Ok((row.get::<_, String>(0)?, row.get::<_, i64>(1)?)),
            )
            .optional()
            .with_context(|| format!("kv_get {key}"))
    }

    /// Remove a key. No-op if missing.
    pub(crate) fn kv_delete(&self, key: &str) -> Result<()> {
        self.conn()
            .execute("DELETE FROM kv WHERE key = ?1", params![key])
            .with_context(|| format!("kv_delete {key}"))?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn set_get_round_trip() {
        let db = Db::open_in_memory().unwrap();
        db.kv_set("x", "1", 100).unwrap();
        assert_eq!(db.kv_get("x").unwrap(), Some(("1".to_string(), 100)));
    }

    #[test]
    fn set_overwrites_existing_value_and_updated_at() {
        let db = Db::open_in_memory().unwrap();
        db.kv_set("x", "1", 100).unwrap();
        db.kv_set("x", "2", 200).unwrap();
        assert_eq!(db.kv_get("x").unwrap(), Some(("2".to_string(), 200)));
    }

    #[test]
    fn missing_key_returns_none() {
        let db = Db::open_in_memory().unwrap();
        assert!(db.kv_get("nope").unwrap().is_none());
    }

    #[test]
    fn delete_removes_key() {
        let db = Db::open_in_memory().unwrap();
        db.kv_set("x", "1", 100).unwrap();
        db.kv_delete("x").unwrap();
        assert!(db.kv_get("x").unwrap().is_none());
        // Deleting a missing key is a no-op.
        db.kv_delete("x").unwrap();
    }
}
