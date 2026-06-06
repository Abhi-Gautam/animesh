//! Durable watch progress per tracked item.
//!
//! Lives next to `tracked_item` philosophically: it is *your*
//! relationship with a show. Losing it is data loss. The `w` key in
//! the TUI writes here; SP-3 and SP-4 read from here.

use anyhow::{Context, Result};
use rusqlite::{params, OptionalExtension};

use super::Db;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct WatchProgress {
    pub seen: i64,
    pub updated_at: i64,
}

impl Db {
    /// Read watch progress. None if the user has never set it.
    pub fn get_watch(&self, source: &str, source_id: &str) -> Result<Option<WatchProgress>> {
        self.conn()
            .query_row(
                "SELECT seen, updated_at FROM watch_progress \
                 WHERE source = ?1 AND source_id = ?2",
                params![source, source_id],
                |row| {
                    Ok(WatchProgress {
                        seen: row.get(0)?,
                        updated_at: row.get(1)?,
                    })
                },
            )
            .optional()
            .context("get_watch")
    }

    /// Overwrite to an absolute value (rare; mostly for `--seen=N` flags).
    pub fn set_watch(&self, source: &str, source_id: &str, seen: i64, now: i64) -> Result<()> {
        self.conn()
            .execute(
                "INSERT INTO watch_progress(source, source_id, seen, updated_at) \
                 VALUES (?1, ?2, ?3, ?4) \
                 ON CONFLICT(source, source_id) DO UPDATE SET \
                    seen = excluded.seen, updated_at = excluded.updated_at",
                params![source, source_id, seen, now],
            )
            .context("set_watch")?;
        Ok(())
    }

    /// Increment by 1 (the `w` key). Returns the new `seen` value.
    /// Capped at `total` if provided.
    pub fn increment_watch(
        &self,
        source: &str,
        source_id: &str,
        total: Option<i64>,
        now: i64,
    ) -> Result<i64> {
        let prev = self
            .get_watch(source, source_id)?
            .map(|w| w.seen)
            .unwrap_or(0);
        let mut next = prev + 1;
        if let Some(t) = total {
            if next > t {
                next = t;
            }
        }
        self.set_watch(source, source_id, next, now)?;
        Ok(next)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn get_returns_none_when_unset() {
        let db = Db::open_in_memory().unwrap();
        assert!(db.get_watch("anilist", "21").unwrap().is_none());
    }

    #[test]
    fn set_then_get_round_trips() {
        let db = Db::open_in_memory().unwrap();
        db.set_watch("anilist", "21", 5, 1_000).unwrap();
        let w = db.get_watch("anilist", "21").unwrap().unwrap();
        assert_eq!(w.seen, 5);
        assert_eq!(w.updated_at, 1_000);
    }

    #[test]
    fn set_overwrites_existing() {
        let db = Db::open_in_memory().unwrap();
        db.set_watch("anilist", "21", 5, 1_000).unwrap();
        db.set_watch("anilist", "21", 12, 2_000).unwrap();
        let w = db.get_watch("anilist", "21").unwrap().unwrap();
        assert_eq!(w.seen, 12);
        assert_eq!(w.updated_at, 2_000);
    }

    #[test]
    fn increment_starts_from_zero_when_unset() {
        let db = Db::open_in_memory().unwrap();
        let n = db.increment_watch("anilist", "21", None, 100).unwrap();
        assert_eq!(n, 1);
        let w = db.get_watch("anilist", "21").unwrap().unwrap();
        assert_eq!(w.seen, 1);
        assert_eq!(w.updated_at, 100);
    }

    #[test]
    fn increment_caps_at_total() {
        let db = Db::open_in_memory().unwrap();
        db.set_watch("anilist", "21", 11, 1_000).unwrap();
        let n = db.increment_watch("anilist", "21", Some(12), 2_000).unwrap();
        assert_eq!(n, 12);
        // Already at total — stays at total.
        let n2 = db.increment_watch("anilist", "21", Some(12), 3_000).unwrap();
        assert_eq!(n2, 12);
    }

    #[test]
    fn increment_handles_no_total_gracefully() {
        let db = Db::open_in_memory().unwrap();
        db.set_watch("anilist", "21", 99, 1_000).unwrap();
        let n = db.increment_watch("anilist", "21", None, 2_000).unwrap();
        assert_eq!(n, 100);
    }
}
