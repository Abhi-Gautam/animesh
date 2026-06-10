//! CRUD over the durable `canonical_release` table.
//!
//! Mirrors the philosophy of `tracked_item.rs` — re-follow-safe,
//! idempotent drop, hard-delete behind `unfollow` — but keyed on
//! [`CanonicalId`] instead of the old (source, source_id) pair. The
//! canonicalization service decides which canonical_id a given source
//! row maps to (via `source_ref`); this module never invents one.
//!
//! Lifecycle of `followed_at` / `dropped_at`:
//!   * NULL / NULL — created by the canonicalizer; not yet followed.
//!   * Some(t) / NULL — currently followed.
//!   * Some(t) / Some(d) — soft-dropped at d, originally followed at t.
//!     A re-follow clears dropped_at and PRESERVES the original
//!     followed_at, so engagement history reads correctly.

use anyhow::{Context, Result};
use rusqlite::{params, OptionalExtension, Row};

use crate::ids::{CanonicalId, ReleaseKind};

use super::Db;

/// One row of `canonical_release`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CanonicalRelease {
    pub id: CanonicalId,
    pub kind: ReleaseKind,
    pub display_title: String,
    pub cover_ascii: Option<String>,
    pub cover_color: Option<String>,
    pub followed_at: Option<i64>,
    pub dropped_at: Option<i64>,
    pub user_note: Option<String>,
    pub created_at: i64,
}

impl CanonicalRelease {
    fn from_row(row: &Row<'_>) -> rusqlite::Result<Self> {
        let kind_str: String = row.get("kind")?;
        let kind = kind_str.parse().map_err(|e: anyhow::Error| {
            rusqlite::Error::FromSqlConversionFailure(
                1,
                rusqlite::types::Type::Text,
                Box::new(std::io::Error::new(
                    std::io::ErrorKind::InvalidData,
                    e.to_string(),
                )),
            )
        })?;
        Ok(Self {
            id: row.get("id")?,
            kind,
            display_title: row.get("display_title")?,
            cover_ascii: row.get("cover_ascii")?,
            cover_color: row.get("cover_color")?,
            followed_at: row.get("followed_at")?,
            dropped_at: row.get("dropped_at")?,
            user_note: row.get("user_note")?,
            created_at: row.get("created_at")?,
        })
    }
}

/// Outcome of [`Db::follow_canonical`]. Identical shape to the legacy
/// [`super::FollowOutcome`] so callers can swap one for the other.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CanonicalFollowOutcome {
    /// First time this canonical has been followed (was created-not-followed).
    NewlyFollowed,
    /// Was previously dropped; dropped_at cleared.
    RestoredFromDrop,
    /// Already actively followed; nothing changed.
    AlreadyFollowing,
    /// The canonical_release row does not exist; caller must create it
    /// (via the canonicalizer) first.
    NotFound,
}

impl Db {
    /// Create a canonical_release row. The canonicalizer is the only
    /// expected call site; the row is "created but not followed" until
    /// [`Db::follow_canonical`] flips it.
    ///
    /// Returns `true` if a row was inserted, `false` if a row with the
    /// same id already existed (idempotent). On duplicate, the existing
    /// row is left untouched.
    pub fn upsert_canonical(
        &self,
        id: &CanonicalId,
        kind: ReleaseKind,
        display_title: &str,
        created_at: i64,
    ) -> Result<bool> {
        let changed = self
            .conn()
            .execute(
                "INSERT INTO canonical_release (id, kind, display_title, created_at) \
                 VALUES (?1, ?2, ?3, ?4) \
                 ON CONFLICT(id) DO NOTHING",
                params![id, kind.as_str(), display_title, created_at],
            )
            .context("upsert_canonical insert")?;
        Ok(changed > 0)
    }

    /// Lookup by canonical id. None if not found.
    #[cfg(test)]
    pub fn find_canonical(&self, id: &CanonicalId) -> Result<Option<CanonicalRelease>> {
        self.conn()
            .query_row(
                "SELECT * FROM canonical_release WHERE id = ?1",
                params![id],
                CanonicalRelease::from_row,
            )
            .optional()
            .context("find_canonical")
    }

    /// Flip a canonical from created-not-followed to actively followed,
    /// or restore from drop. Single transaction so concurrent callers
    /// cannot wedge a half-followed row.
    ///
    /// `followed_at` is the wall-clock seconds at which the act of
    /// following happened. On restoration, the row's original
    /// followed_at is preserved — engagement history reads correctly.
    pub fn follow_canonical(
        &mut self,
        id: &CanonicalId,
        followed_at: i64,
    ) -> Result<CanonicalFollowOutcome> {
        let tx = self
            .conn_mut()
            .transaction()
            .context("follow_canonical tx")?;
        let existing: Option<(Option<i64>, Option<i64>)> = tx
            .query_row(
                "SELECT followed_at, dropped_at FROM canonical_release WHERE id = ?1",
                params![id],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .optional()
            .context("follow_canonical lookup")?;
        let outcome = match existing {
            None => CanonicalFollowOutcome::NotFound,
            Some((Some(_), None)) => CanonicalFollowOutcome::AlreadyFollowing,
            Some((Some(_), Some(_))) => {
                tx.execute(
                    "UPDATE canonical_release SET dropped_at = NULL WHERE id = ?1",
                    params![id],
                )
                .context("restoring dropped canonical_release")?;
                CanonicalFollowOutcome::RestoredFromDrop
            }
            Some((None, _)) => {
                // Created-but-never-followed (followed_at NULL). Set it
                // now. Clear dropped_at defensively even though it's
                // unexpected.
                tx.execute(
                    "UPDATE canonical_release \
                     SET followed_at = ?2, dropped_at = NULL WHERE id = ?1",
                    params![id, followed_at],
                )
                .context("first-time follow update")?;
                CanonicalFollowOutcome::NewlyFollowed
            }
        };
        tx.commit().context("follow_canonical commit")?;
        Ok(outcome)
    }

    /// Soft-drop. Idempotent: dropping an already-dropped canonical is
    /// a no-op; dropping a non-existent id returns `false`.
    pub fn drop_canonical(&self, id: &CanonicalId, dropped_at: i64) -> Result<bool> {
        let updated = self
            .conn()
            .execute(
                "UPDATE canonical_release \
                 SET dropped_at = COALESCE(dropped_at, ?2) \
                 WHERE id = ?1 AND followed_at IS NOT NULL",
                params![id, dropped_at],
            )
            .context("drop_canonical update")?;
        Ok(updated > 0)
    }

    /// Currently followed canonicals, newest-first by followed_at.
    pub fn list_active_canonical(&self) -> Result<Vec<CanonicalRelease>> {
        let conn = self.conn();
        let mut stmt = conn
            .prepare_cached(
                "SELECT * FROM canonical_release \
                 WHERE followed_at IS NOT NULL AND dropped_at IS NULL \
                 ORDER BY followed_at DESC",
            )
            .context("prepare list_active_canonical")?;
        let rows = stmt
            .query_map([], CanonicalRelease::from_row)
            .context("query list_active_canonical")?;
        rows.collect::<rusqlite::Result<Vec<_>>>()
            .context("collect list_active_canonical rows")
    }

    /// Count of actively followed canonicals.
    pub fn count_followed_canonical(&self) -> Result<i64> {
        self.conn()
            .query_row(
                "SELECT COUNT(*) FROM canonical_release \
                 WHERE followed_at IS NOT NULL AND dropped_at IS NULL",
                [],
                |row| row.get(0),
            )
            .context("count_followed_canonical")
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

    fn id(slug: &str) -> CanonicalId {
        CanonicalId::new(ReleaseKind::Tv, slug).unwrap()
    }

    #[test]
    fn upsert_inserts_then_is_idempotent() {
        let db = fresh();
        let id = id("severance");
        assert!(db
            .upsert_canonical(&id, ReleaseKind::Tv, "Severance", 100)
            .unwrap());
        assert!(!db
            .upsert_canonical(&id, ReleaseKind::Tv, "Severance", 200)
            .unwrap());
        // The second upsert did NOT change the existing row.
        let row = db.find_canonical(&id).unwrap().unwrap();
        assert_eq!(row.created_at, 100);
    }

    #[test]
    fn find_returns_none_for_missing_id() {
        let db = fresh();
        assert!(db.find_canonical(&id("nope")).unwrap().is_none());
    }

    #[test]
    fn follow_unknown_id_reports_not_found() {
        let mut db = fresh();
        let out = db.follow_canonical(&id("ghost"), 100).unwrap();
        assert_eq!(out, CanonicalFollowOutcome::NotFound);
    }

    #[test]
    fn follow_lifecycle_newly_then_dropped_then_restored() {
        let mut db = fresh();
        let id = id("severance");
        db.upsert_canonical(&id, ReleaseKind::Tv, "Severance", 1)
            .unwrap();

        let out = db.follow_canonical(&id, 100).unwrap();
        assert_eq!(out, CanonicalFollowOutcome::NewlyFollowed);
        let row = db.find_canonical(&id).unwrap().unwrap();
        assert_eq!(row.followed_at, Some(100));
        assert_eq!(row.dropped_at, None);

        assert!(db.drop_canonical(&id, 200).unwrap());
        let row = db.find_canonical(&id).unwrap().unwrap();
        assert_eq!(
            row.followed_at,
            Some(100),
            "followed_at preserved across drop"
        );
        assert_eq!(row.dropped_at, Some(200));

        let out = db.follow_canonical(&id, 300).unwrap();
        assert_eq!(out, CanonicalFollowOutcome::RestoredFromDrop);
        let row = db.find_canonical(&id).unwrap().unwrap();
        assert_eq!(
            row.followed_at,
            Some(100),
            "followed_at preserved across restore"
        );
        assert_eq!(row.dropped_at, None);

        let out = db.follow_canonical(&id, 400).unwrap();
        assert_eq!(out, CanonicalFollowOutcome::AlreadyFollowing);
    }

    #[test]
    fn drop_is_idempotent_and_preserves_original_dropped_at() {
        let mut db = fresh();
        let id = id("foo");
        db.upsert_canonical(&id, ReleaseKind::Tv, "Foo", 1).unwrap();
        db.follow_canonical(&id, 100).unwrap();
        assert!(db.drop_canonical(&id, 200).unwrap());
        assert!(db.drop_canonical(&id, 999).unwrap());
        let row = db.find_canonical(&id).unwrap().unwrap();
        assert_eq!(
            row.dropped_at,
            Some(200),
            "COALESCE keeps original drop time"
        );
    }

    #[test]
    fn drop_on_created_not_followed_is_a_no_op() {
        // A canonical that was created (by the canonicalizer) but never
        // followed cannot be dropped — there's nothing to drop. The
        // semantics here protect against accidental orphan-drops.
        let db = fresh();
        let id = id("never-followed");
        db.upsert_canonical(&id, ReleaseKind::Tv, "Ghost", 1)
            .unwrap();
        assert!(!db.drop_canonical(&id, 100).unwrap());
        let row = db.find_canonical(&id).unwrap().unwrap();
        assert_eq!(row.followed_at, None);
        assert_eq!(row.dropped_at, None);
    }

    #[test]
    fn list_active_excludes_dropped_rows() {
        let mut db = fresh();
        let a = id("a");
        let b = id("b");
        let c = id("c");
        for (id, t) in [(&a, 1), (&b, 2), (&c, 3)] {
            db.upsert_canonical(id, ReleaseKind::Tv, "X", t).unwrap();
            db.follow_canonical(id, t).unwrap();
        }
        db.drop_canonical(&b, 10).unwrap();

        let active = db.list_active_canonical().unwrap();
        assert_eq!(active.len(), 2);
        assert!(active.iter().all(|r| r.id != b));
    }

    #[test]
    fn list_active_orders_by_followed_at_desc() {
        let mut db = fresh();
        for (slug, t) in [("oldest", 100), ("middle", 200), ("newest", 300)] {
            let i = id(slug);
            db.upsert_canonical(&i, ReleaseKind::Tv, slug, t).unwrap();
            db.follow_canonical(&i, t).unwrap();
        }
        let active = db.list_active_canonical().unwrap();
        let order: Vec<&str> = active.iter().map(|r| r.id.slug()).collect();
        assert_eq!(order, ["newest", "middle", "oldest"]);
    }

    // ------------------------------------------------------------------
    // Property tests — the marvel-correctness contract.
    //
    // The state of a canonical row is one of four:
    //   Created     — followed_at=NULL, dropped_at=NULL
    //   Active      — followed_at=Some, dropped_at=NULL
    //   Dropped     — followed_at=Some, dropped_at=Some
    //   (absent)    — no row at all
    //
    // We generate arbitrary sequences of operations and assert the DB
    // state matches an in-memory state machine.
    // ------------------------------------------------------------------

    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    enum State {
        Created,
        Active,
        Dropped,
    }

    #[derive(Debug, Clone)]
    enum Op {
        Upsert(u8),
        Follow(u8),
        Drop(u8),
    }

    fn arb_op() -> impl Strategy<Value = Op> {
        prop_oneof![
            (0u8..8).prop_map(Op::Upsert),
            (0u8..8).prop_map(Op::Follow),
            (0u8..8).prop_map(Op::Drop),
        ]
    }

    fn slug_for(n: u8) -> String {
        format!("id-{n}")
    }

    proptest! {
        #![proptest_config(ProptestConfig { cases: 64, .. ProptestConfig::default() })]
        #[test]
        fn canonical_state_machine_matches_model(ops in prop::collection::vec(arb_op(), 0..80)) {
            let mut db = fresh();
            let mut model: HashMap<u8, State> = HashMap::new();
            let mut clock: i64 = 0;
            for op in &ops {
                clock += 1;
                match *op {
                    Op::Upsert(n) => {
                        let id = id(&slug_for(n));
                        let inserted = db.upsert_canonical(&id, ReleaseKind::Tv, "X", clock).unwrap();
                        match model.get(&n) {
                            None => {
                                prop_assert!(inserted);
                                model.insert(n, State::Created);
                            }
                            Some(_) => prop_assert!(!inserted, "second upsert is a no-op"),
                        }
                    }
                    Op::Follow(n) => {
                        let id = id(&slug_for(n));
                        let out = db.follow_canonical(&id, clock).unwrap();
                        let expected = match model.get(&n) {
                            None => CanonicalFollowOutcome::NotFound,
                            Some(State::Created) => CanonicalFollowOutcome::NewlyFollowed,
                            Some(State::Active) => CanonicalFollowOutcome::AlreadyFollowing,
                            Some(State::Dropped) => CanonicalFollowOutcome::RestoredFromDrop,
                        };
                        prop_assert_eq!(out, expected);
                        if model.contains_key(&n) {
                            model.insert(n, State::Active);
                        }
                    }
                    Op::Drop(n) => {
                        let id = id(&slug_for(n));
                        let touched = db.drop_canonical(&id, clock).unwrap();
                        match model.get(&n) {
                            None | Some(State::Created) => prop_assert!(!touched),
                            Some(State::Active) => {
                                prop_assert!(touched);
                                model.insert(n, State::Dropped);
                            }
                            Some(State::Dropped) => {
                                prop_assert!(touched, "idempotent drop still touches the row");
                            }
                        }
                    }
                }
            }
            // Invariants.
            let expected_active = model.values().filter(|s| **s == State::Active).count() as i64;
            prop_assert_eq!(db.count_followed_canonical().unwrap(), expected_active);
            // No duplicates.
            let unique: i64 = db.conn().query_row(
                "SELECT COUNT(*) FROM (SELECT DISTINCT id FROM canonical_release)",
                [], |r| r.get(0),
            ).unwrap();
            let total: i64 = db.conn().query_row(
                "SELECT COUNT(*) FROM canonical_release", [], |r| r.get(0),
            ).unwrap();
            prop_assert_eq!(unique, total);
            // State of each present id matches model.
            for (n, expected) in &model {
                let row = db.find_canonical(&id(&slug_for(*n))).unwrap().unwrap();
                let actual = match (row.followed_at, row.dropped_at) {
                    (None, _) => State::Created,
                    (Some(_), None) => State::Active,
                    (Some(_), Some(_)) => State::Dropped,
                };
                prop_assert_eq!(actual, *expected);
            }
        }
    }
}
