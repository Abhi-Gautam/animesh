//! CRUD over the durable `tracked_item` table.
//!
//! The follow act is *yours*; this module guards it. `add_follow` is
//! re-follow-safe (a previously-dropped show is restored, preserving
//! `followed_at` and `user_note`). `drop` is forgiving (idempotent).
//! `unfollow` is the hard delete and is the rare path.

use anyhow::{Context, Result};
use rusqlite::{params, OptionalExtension, Row};

use super::Db;

/// One row of the durable library.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TrackedItem {
    pub id: i64,
    pub source: String,
    pub source_id: String,
    pub kind: String,
    pub display_title: String,
    pub followed_at: i64,
    pub dropped_at: Option<i64>,
    pub user_note: Option<String>,
    pub cover_ascii: Option<String>,
    pub cover_color: Option<String>,
}

impl TrackedItem {
    fn from_row(row: &Row<'_>) -> rusqlite::Result<Self> {
        Ok(Self {
            id: row.get("id")?,
            source: row.get("source")?,
            source_id: row.get("source_id")?,
            kind: row.get("kind")?,
            display_title: row.get("display_title")?,
            followed_at: row.get("followed_at")?,
            dropped_at: row.get("dropped_at")?,
            user_note: row.get("user_note")?,
            cover_ascii: row.get("cover_ascii").ok(),
            cover_color: row.get("cover_color").ok(),
        })
    }
}

/// Outcome of `add_follow`. Distinguishes the three meaningful cases
/// so callers can give the user honest feedback.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FollowOutcome {
    /// The show was not previously in the library.
    NewlyFollowed,
    /// The show had been dropped; `dropped_at` was cleared.
    RestoredFromDrop,
    /// The show was already actively followed; nothing changed.
    AlreadyFollowing,
}

/// Filter for `list_follows`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ListFilter {
    /// Only `dropped_at IS NULL`.
    Active,
    /// Every row.
    All,
    /// Only `dropped_at IS NOT NULL`.
    Dropped,
}

impl Db {
    /// Add or re-add a show to the library. UPSERT-like: if the row
    /// exists and is dropped, restore it; if it exists and is active,
    /// report `AlreadyFollowing`; else insert.
    pub fn add_follow(
        &mut self,
        source: &str,
        source_id: &str,
        kind: &str,
        display_title: &str,
        followed_at: i64,
    ) -> Result<FollowOutcome> {
        // Single transaction so a concurrent caller cannot wedge a
        // half-followed row.
        let tx = self.conn_mut().transaction().context("begin tx for follow")?;
        let existing: Option<(i64, Option<i64>)> = tx
            .query_row(
                "SELECT id, dropped_at FROM tracked_item \
                 WHERE source = ?1 AND source_id = ?2",
                params![source, source_id],
                |row| Ok((row.get::<_, i64>(0)?, row.get::<_, Option<i64>>(1)?)),
            )
            .optional()
            .context("looking up existing tracked_item")?;
        let outcome = match existing {
            Some((_, None)) => FollowOutcome::AlreadyFollowing,
            Some((id, Some(_))) => {
                tx.execute(
                    "UPDATE tracked_item SET dropped_at = NULL WHERE id = ?1",
                    params![id],
                )
                .context("restoring dropped tracked_item")?;
                FollowOutcome::RestoredFromDrop
            }
            None => {
                tx.execute(
                    "INSERT INTO tracked_item \
                     (source, source_id, kind, display_title, followed_at) \
                     VALUES (?1, ?2, ?3, ?4, ?5)",
                    params![source, source_id, kind, display_title, followed_at],
                )
                .context("inserting tracked_item")?;
                FollowOutcome::NewlyFollowed
            }
        };
        tx.commit().context("commit follow tx")?;
        Ok(outcome)
    }

    /// Soft-delete. Idempotent: dropping an already-dropped show is a
    /// no-op; dropping a non-existent show returns `false`.
    pub fn drop_follow(&self, source: &str, source_id: &str, dropped_at: i64) -> Result<bool> {
        let updated = self
            .conn()
            .execute(
                "UPDATE tracked_item \
                 SET dropped_at = COALESCE(dropped_at, ?3) \
                 WHERE source = ?1 AND source_id = ?2",
                params![source, source_id, dropped_at],
            )
            .context("drop_follow update")?;
        Ok(updated > 0)
    }

    /// Save the rendered ASCII cover + accent color for a followed
    /// show. Idempotent: subsequent calls overwrite. No-op if the row
    /// doesn't exist (no error — the only caller already follows first).
    pub fn set_cover_ascii(
        &self,
        source: &str,
        source_id: &str,
        ascii: &str,
        color: Option<&str>,
    ) -> Result<()> {
        self.conn()
            .execute(
                "UPDATE tracked_item SET cover_ascii = ?3, cover_color = ?4 \
                 WHERE source = ?1 AND source_id = ?2",
                params![source, source_id, ascii, color],
            )
            .context("set_cover_ascii")?;
        Ok(())
    }

    /// Hard delete. Returns whether a row was removed.
    pub fn unfollow(&self, source: &str, source_id: &str) -> Result<bool> {
        let removed = self
            .conn()
            .execute(
                "DELETE FROM tracked_item WHERE source = ?1 AND source_id = ?2",
                params![source, source_id],
            )
            .context("unfollow delete")?;
        Ok(removed > 0)
    }

    /// List items per filter. Active is the default for the CLI's
    /// `list` command; All and Dropped serve the `--all` / `--dropped`
    /// flags.
    pub fn list_follows(&self, filter: ListFilter) -> Result<Vec<TrackedItem>> {
        let sql = match filter {
            ListFilter::Active => {
                "SELECT * FROM tracked_item WHERE dropped_at IS NULL ORDER BY followed_at DESC"
            }
            ListFilter::All => "SELECT * FROM tracked_item ORDER BY followed_at DESC",
            ListFilter::Dropped => {
                "SELECT * FROM tracked_item WHERE dropped_at IS NOT NULL ORDER BY dropped_at DESC"
            }
        };
        let conn = self.conn();
        let mut stmt = conn.prepare(sql).context("prepare list_follows")?;
        let rows = stmt
            .query_map([], TrackedItem::from_row)
            .context("query list_follows")?;
        rows.collect::<rusqlite::Result<Vec<_>>>()
            .context("collect list_follows rows")
    }

    /// Lookup by (source, source_id). None if not found.
    pub fn find_by_source(&self, source: &str, source_id: &str) -> Result<Option<TrackedItem>> {
        self.conn()
            .query_row(
                "SELECT * FROM tracked_item WHERE source = ?1 AND source_id = ?2",
                params![source, source_id],
                TrackedItem::from_row,
            )
            .optional()
            .context("find_by_source")
    }

    /// Count of active follows. Used by the empty-library hint in
    /// `schedule` and by `doctor`.
    pub fn count_active(&self) -> Result<i64> {
        self.conn()
            .query_row(
                "SELECT COUNT(*) FROM tracked_item WHERE dropped_at IS NULL",
                [],
                |row| row.get(0),
            )
            .context("count_active")
    }

    pub fn count_dropped(&self) -> Result<i64> {
        self.conn()
            .query_row(
                "SELECT COUNT(*) FROM tracked_item WHERE dropped_at IS NOT NULL",
                [],
                |row| row.get(0),
            )
            .context("count_dropped")
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use proptest::prelude::*;
    use std::collections::HashMap;

    fn fresh() -> Db {
        Db::open_in_memory().unwrap()
    }

    #[test]
    fn follow_unfollow_round_trip() {
        let mut db = fresh();
        let out = db.add_follow("anilist", "21", "anime", "One Piece", 100).unwrap();
        assert_eq!(out, FollowOutcome::NewlyFollowed);
        assert_eq!(db.count_active().unwrap(), 1);

        let again = db.add_follow("anilist", "21", "anime", "One Piece", 200).unwrap();
        assert_eq!(again, FollowOutcome::AlreadyFollowing);
        assert_eq!(db.count_active().unwrap(), 1, "no duplicate");

        let removed = db.unfollow("anilist", "21").unwrap();
        assert!(removed);
        assert_eq!(db.count_active().unwrap(), 0);
    }

    #[test]
    fn drop_then_follow_restores_existing_row() {
        let mut db = fresh();
        db.add_follow("anilist", "21", "anime", "One Piece", 100).unwrap();
        assert!(db.drop_follow("anilist", "21", 150).unwrap());
        assert_eq!(db.count_active().unwrap(), 0);
        assert_eq!(db.count_dropped().unwrap(), 1);

        let original_id = db.find_by_source("anilist", "21").unwrap().unwrap().id;
        let out = db.add_follow("anilist", "21", "anime", "One Piece", 200).unwrap();
        assert_eq!(out, FollowOutcome::RestoredFromDrop);
        let restored = db.find_by_source("anilist", "21").unwrap().unwrap();
        assert_eq!(restored.id, original_id, "row id must be preserved on restore");
        assert_eq!(restored.followed_at, 100, "original followed_at preserved");
        assert!(restored.dropped_at.is_none());
    }

    #[test]
    fn drop_is_idempotent_and_does_not_overwrite_original_dropped_at() {
        let mut db = fresh();
        db.add_follow("anilist", "21", "anime", "One Piece", 100).unwrap();
        assert!(db.drop_follow("anilist", "21", 150).unwrap());
        assert!(db.drop_follow("anilist", "21", 999).unwrap());
        let row = db.find_by_source("anilist", "21").unwrap().unwrap();
        assert_eq!(row.dropped_at, Some(150), "COALESCE keeps original timestamp");
    }

    #[test]
    fn drop_or_unfollow_nonexistent_is_safe() {
        let db = fresh();
        assert!(!db.drop_follow("anilist", "999999", 100).unwrap());
        assert!(!db.unfollow("anilist", "999999").unwrap());
    }

    #[test]
    fn list_filters_partition_active_and_dropped() {
        let mut db = fresh();
        db.add_follow("anilist", "1", "anime", "A", 1).unwrap();
        db.add_follow("anilist", "2", "anime", "B", 2).unwrap();
        db.add_follow("anilist", "3", "anime", "C", 3).unwrap();
        db.drop_follow("anilist", "2", 10).unwrap();

        let active = db.list_follows(ListFilter::Active).unwrap();
        let dropped = db.list_follows(ListFilter::Dropped).unwrap();
        let all = db.list_follows(ListFilter::All).unwrap();
        assert_eq!(active.len(), 2);
        assert_eq!(dropped.len(), 1);
        assert_eq!(all.len(), 3);
        assert_eq!(dropped[0].source_id, "2");
    }

    #[test]
    fn list_active_orders_by_followed_at_desc() {
        let mut db = fresh();
        db.add_follow("anilist", "1", "anime", "Old", 100).unwrap();
        db.add_follow("anilist", "2", "anime", "Newer", 200).unwrap();
        db.add_follow("anilist", "3", "anime", "Newest", 300).unwrap();
        let active = db.list_follows(ListFilter::Active).unwrap();
        let order: Vec<&str> = active.iter().map(|i| i.display_title.as_str()).collect();
        assert_eq!(order, ["Newest", "Newer", "Old"]);
    }

    // ------------------------------------------------------------------
    // Property tests — the marvel-correctness contract.
    //
    // Generate arbitrary sequences of (follow, drop, unfollow) ops and
    // assert that the DB state remains internally consistent and
    // matches an in-memory model.
    // ------------------------------------------------------------------

    #[derive(Debug, Clone)]
    enum Op {
        Follow(u8),   // small id so we get collisions
        Drop(u8),
        Unfollow(u8),
    }

    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    enum State {
        Active,
        Dropped,
    }

    fn arb_op() -> impl Strategy<Value = Op> {
        prop_oneof![
            (0u8..16).prop_map(Op::Follow),
            (0u8..16).prop_map(Op::Drop),
            (0u8..16).prop_map(Op::Unfollow),
        ]
    }

    proptest! {
        #![proptest_config(ProptestConfig {
            cases: 64,
            .. ProptestConfig::default()
        })]
        #[test]
        fn db_state_matches_in_memory_model(ops in prop::collection::vec(arb_op(), 0..80)) {
            let mut db = fresh();
            let mut model: HashMap<u8, State> = HashMap::new();
            let mut clock: i64 = 0;
            for op in &ops {
                clock += 1;
                match *op {
                    Op::Follow(id) => {
                        let sid = id.to_string();
                        let out = db.add_follow("anilist", &sid, "anime", "T", clock).unwrap();
                        let prev = model.get(&id).copied();
                        match prev {
                            None => prop_assert_eq!(out, FollowOutcome::NewlyFollowed),
                            Some(State::Active) => prop_assert_eq!(out, FollowOutcome::AlreadyFollowing),
                            Some(State::Dropped) => prop_assert_eq!(out, FollowOutcome::RestoredFromDrop),
                        }
                        model.insert(id, State::Active);
                    }
                    Op::Drop(id) => {
                        let sid = id.to_string();
                        let touched = db.drop_follow("anilist", &sid, clock).unwrap();
                        match model.get(&id).copied() {
                            None => prop_assert!(!touched),
                            Some(_) => {
                                prop_assert!(touched);
                                model.insert(id, State::Dropped);
                            }
                        }
                    }
                    Op::Unfollow(id) => {
                        let sid = id.to_string();
                        let removed = db.unfollow("anilist", &sid).unwrap();
                        match model.remove(&id) {
                            None => prop_assert!(!removed),
                            Some(_) => prop_assert!(removed),
                        }
                    }
                }
            }
            // Invariants:
            // 1. Counts agree with model.
            let expected_active = model.values().filter(|s| **s == State::Active).count() as i64;
            let expected_dropped = model.values().filter(|s| **s == State::Dropped).count() as i64;
            prop_assert_eq!(db.count_active().unwrap(), expected_active);
            prop_assert_eq!(db.count_dropped().unwrap(), expected_dropped);
            // 2. No duplicate (source, source_id) — should be impossible
            //    given the unique index, but verify.
            let unique: i64 = db.conn().query_row(
                "SELECT COUNT(*) FROM (SELECT DISTINCT source, source_id FROM tracked_item)",
                [],
                |r| r.get(0),
            ).unwrap();
            let total: i64 = db.conn().query_row(
                "SELECT COUNT(*) FROM tracked_item",
                [],
                |r| r.get(0),
            ).unwrap();
            prop_assert_eq!(unique, total);
            // 3. State of each present id matches model.
            for (id, expected) in &model {
                let row = db.find_by_source("anilist", &id.to_string()).unwrap().unwrap();
                let actual = if row.dropped_at.is_some() { State::Dropped } else { State::Active };
                prop_assert_eq!(actual, *expected);
            }
        }
    }
}
