//! Idempotency log for the canonicalization service.
//!
//! Before calling the LLM to decide whether a (source, source_id) maps
//! to an existing canonical or needs a new one, the canonical service
//! checks this cache. A hit short-circuits the LLM call. A miss
//! triggers the full alias-match → LLM pipeline; the result is then
//! recorded here.
//!
//! The cache is intentionally tolerant: when a canonical_release is
//! deleted, the cache row's `canonical_id` becomes NULL (FK
//! ON DELETE SET NULL). [`Db::cached_canonical_for`] treats that as a
//! miss — re-canonicalize next time we see the source row.
//!
//! `decided_by` carries provenance: "llm:claude-opus-4-7",
//! "alias-match", "manual", "legacy-backfill". The shape is a
//! free-form colon-namespaced string so we can group + analyze later
//! without a schema change.

use anyhow::{Context, Result};
use rusqlite::{params, OptionalExtension};

use crate::ids::CanonicalId;

use super::Db;

impl Db {
    /// Record a canonicalization decision. Last write wins —
    /// re-canonicalization is allowed and the audit history is in
    /// `decided_at` + `decided_by`.
    pub fn cache_canonicalization(
        &self,
        source: &str,
        source_id: &str,
        canonical_id: &CanonicalId,
        decided_by: &str,
        decided_at: i64,
    ) -> Result<()> {
        self.conn()
            .execute(
                "INSERT INTO canonicalization_cache \
                 (source, source_id, canonical_id, decided_at, decided_by) \
                 VALUES (?1, ?2, ?3, ?4, ?5) \
                 ON CONFLICT(source, source_id) DO UPDATE SET \
                    canonical_id = excluded.canonical_id, \
                    decided_at   = excluded.decided_at, \
                    decided_by   = excluded.decided_by",
                params![source, source_id, canonical_id, decided_at, decided_by],
            )
            .context("cache_canonicalization")?;
        Ok(())
    }

    /// Look up a previously decided canonical id. Returns None for
    /// cache miss or when the cached canonical was deleted (FK SET
    /// NULL). The caller redoes the decision in either case.
    pub fn cached_canonical_for(
        &self,
        source: &str,
        source_id: &str,
    ) -> Result<Option<CanonicalId>> {
        self.conn()
            .query_row(
                "SELECT canonical_id FROM canonicalization_cache \
                 WHERE source = ?1 AND source_id = ?2 AND canonical_id IS NOT NULL",
                params![source, source_id],
                |r| r.get(0),
            )
            .optional()
            .context("cached_canonical_for")
    }

    /// Read the full audit row. Used by the doctor command + telemetry,
    /// not the hot path. Returns (canonical_id, decided_at, decided_by).
    pub fn canonicalization_audit(
        &self,
        source: &str,
        source_id: &str,
    ) -> Result<Option<(Option<CanonicalId>, i64, String)>> {
        self.conn()
            .query_row(
                "SELECT canonical_id, decided_at, decided_by \
                 FROM canonicalization_cache \
                 WHERE source = ?1 AND source_id = ?2",
                params![source, source_id],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
            )
            .optional()
            .context("canonicalization_audit")
    }

    /// Clear cache entries decided before `older_than`. The doctor
    /// command surfaces this when the LLM model upgrades — old
    /// decisions get re-evaluated. No effect on the source_ref
    /// mappings themselves; those persist independently.
    pub fn invalidate_canonicalization_older_than(&self, older_than: i64) -> Result<usize> {
        let n = self
            .conn()
            .execute(
                "DELETE FROM canonicalization_cache WHERE decided_at < ?1",
                params![older_than],
            )
            .context("invalidate_canonicalization_older_than")?;
        Ok(n)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ids::{CanonicalId, ReleaseKind};

    fn fresh() -> Db {
        Db::open_in_memory().unwrap()
    }

    fn id(slug: &str) -> CanonicalId {
        CanonicalId::new(ReleaseKind::Tv, slug).unwrap()
    }

    fn with_canonical(db: &Db, id: &CanonicalId) {
        db.upsert_canonical(id, ReleaseKind::Tv, "X", 1).unwrap();
    }

    #[test]
    fn cache_then_read_round_trips() {
        let db = fresh();
        let cid = id("foo");
        with_canonical(&db, &cid);
        db.cache_canonicalization("anilist", "21", &cid, "llm:claude-opus-4-7", 1000)
            .unwrap();
        let cached = db.cached_canonical_for("anilist", "21").unwrap().unwrap();
        assert_eq!(cached, cid);
    }

    #[test]
    fn second_cache_for_same_pair_overwrites_last_decision() {
        let db = fresh();
        let a = id("a");
        let b = id("b");
        with_canonical(&db, &a);
        with_canonical(&db, &b);
        db.cache_canonicalization("anilist", "21", &a, "alias-match", 1000).unwrap();
        db.cache_canonicalization("anilist", "21", &b, "llm:claude-opus-4-7", 2000).unwrap();
        let cached = db.cached_canonical_for("anilist", "21").unwrap().unwrap();
        assert_eq!(cached, b);
        let audit = db.canonicalization_audit("anilist", "21").unwrap().unwrap();
        assert_eq!(audit.1, 2000);
        assert_eq!(audit.2, "llm:claude-opus-4-7");
    }

    #[test]
    fn cache_miss_returns_none() {
        let db = fresh();
        assert!(db.cached_canonical_for("anilist", "99").unwrap().is_none());
    }

    #[test]
    fn fk_set_null_after_canonical_deleted_makes_lookup_a_miss() {
        let db = fresh();
        let cid = id("foo");
        with_canonical(&db, &cid);
        db.cache_canonicalization("anilist", "21", &cid, "llm", 1000).unwrap();
        // Sanity: lookup hits before deletion.
        assert!(db.cached_canonical_for("anilist", "21").unwrap().is_some());

        db.delete_canonical(&cid).unwrap();

        // Lookup now returns None because canonical_id became NULL.
        assert!(db.cached_canonical_for("anilist", "21").unwrap().is_none());
        // But the audit history is preserved — only canonical_id was
        // nulled out.
        let audit = db.canonicalization_audit("anilist", "21").unwrap().unwrap();
        assert_eq!(audit.0, None);
        assert_eq!(audit.1, 1000);
        assert_eq!(audit.2, "llm");
    }

    #[test]
    fn invalidate_removes_only_rows_older_than_threshold() {
        let db = fresh();
        let cid = id("foo");
        with_canonical(&db, &cid);
        db.cache_canonicalization("anilist", "1", &cid, "llm", 100).unwrap();
        db.cache_canonicalization("anilist", "2", &cid, "llm", 500).unwrap();
        db.cache_canonicalization("anilist", "3", &cid, "llm", 1000).unwrap();
        let removed = db.invalidate_canonicalization_older_than(600).unwrap();
        assert_eq!(removed, 2);
        assert!(db.cached_canonical_for("anilist", "1").unwrap().is_none());
        assert!(db.cached_canonical_for("anilist", "2").unwrap().is_none());
        assert!(db.cached_canonical_for("anilist", "3").unwrap().is_some());
    }

    #[test]
    fn cache_with_missing_canonical_fails_with_fk_error() {
        // FK is enforced even on INSERT to canonicalization_cache —
        // refuses to point at a non-existent canonical_release.
        let db = fresh();
        let cid = id("ghost"); // not upserted
        let err = db
            .cache_canonicalization("anilist", "21", &cid, "llm", 1000)
            .unwrap_err();
        assert!(format!("{err:#}").to_uppercase().contains("FOREIGN KEY"));
    }

    #[test]
    fn audit_returns_none_when_no_cache_row() {
        let db = fresh();
        assert!(db.canonicalization_audit("anilist", "21").unwrap().is_none());
    }
}
