//! CRUD over `source_ref`.
//!
//! A `source_ref` is the (source, source_id) → canonical_release link
//! that lets many noisy sources fan into one canonical row. The
//! [`Db::attach_source_ref`] semantics are deliberately strict:
//!
//!   * (source, source_id) absent → insert.
//!   * (source, source_id) present and points at the same canonical
//!     → update raw_title + confidence (idempotent re-canonicalize).
//!   * (source, source_id) present and points at a DIFFERENT canonical
//!     → refuse. The caller must explicitly drop + re-follow to remap.
//!
//! This is the marvel-correctness bar: silent overwrites are the
//! easiest way to corrupt a canonicalization graph.

use anyhow::{anyhow, Context, Result};
use rusqlite::{params, OptionalExtension, Row};

use crate::ids::CanonicalId;

use super::Db;

/// One row of `source_ref`. Used by the export layer and by the
/// canonicalization cache for lookups.
#[derive(Debug, Clone, PartialEq)]
pub struct SourceRef {
    pub canonical_id: CanonicalId,
    pub source: String,
    pub source_id: String,
    pub raw_title: Option<String>,
    pub confidence: f64,
}

impl SourceRef {
    fn from_row(row: &Row<'_>) -> rusqlite::Result<Self> {
        Ok(Self {
            canonical_id: row.get("canonical_id")?,
            source: row.get("source")?,
            source_id: row.get("source_id")?,
            raw_title: row.get("raw_title")?,
            confidence: row.get("confidence")?,
        })
    }
}

/// Outcome of [`Db::attach_source_ref`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AttachSourceRefOutcome {
    /// First time we've seen this (source, source_id) pair.
    Inserted,
    /// (source, source_id) already mapped to the SAME canonical_id;
    /// raw_title / confidence were refreshed.
    RefreshedSameCanonical,
}

impl Db {
    /// Attach a (source, source_id) to a canonical_id. Idempotent
    /// against the same canonical; refuses to silently remap.
    ///
    /// Errors:
    ///   * confidence out of [0, 1] — surface a typed error so callers
    ///     can tell the LLM "produce a valid confidence" without a
    ///     CHECK-constraint trace.
    ///   * (source, source_id) already mapped to a different canonical
    ///     → returns `Err`. Use [`Db::remap_source_ref`] to override.
    pub fn attach_source_ref(
        &mut self,
        canonical_id: &CanonicalId,
        source: &str,
        source_id: &str,
        raw_title: Option<&str>,
        confidence: f64,
    ) -> Result<AttachSourceRefOutcome> {
        if !(0.0..=1.0).contains(&confidence) {
            return Err(anyhow!(
                "source_ref confidence must be in [0, 1], got {confidence}"
            ));
        }
        let tx = self.conn_mut().transaction().context("attach_source_ref tx")?;
        let existing: Option<CanonicalId> = tx
            .query_row(
                "SELECT canonical_id FROM source_ref WHERE source = ?1 AND source_id = ?2",
                params![source, source_id],
                |r| r.get(0),
            )
            .optional()
            .context("attach_source_ref lookup")?;
        let outcome = match existing {
            None => {
                tx.execute(
                    "INSERT INTO source_ref \
                     (canonical_id, source, source_id, raw_title, confidence) \
                     VALUES (?1, ?2, ?3, ?4, ?5)",
                    params![canonical_id, source, source_id, raw_title, confidence],
                )
                .context("insert source_ref")?;
                AttachSourceRefOutcome::Inserted
            }
            Some(ref current) if current == canonical_id => {
                tx.execute(
                    "UPDATE source_ref \
                     SET raw_title = ?3, confidence = ?4 \
                     WHERE source = ?1 AND source_id = ?2",
                    params![source, source_id, raw_title, confidence],
                )
                .context("refresh source_ref")?;
                AttachSourceRefOutcome::RefreshedSameCanonical
            }
            Some(other) => {
                return Err(anyhow!(
                    "source_ref collision: ({source}, {source_id}) is already mapped to {other}, \
                     refusing silent remap to {canonical_id}; drop the old follow first"
                ));
            }
        };
        tx.commit().context("attach_source_ref commit")?;
        Ok(outcome)
    }

    /// List every (source, source_id) attached to a canonical. Useful
    /// for the LLM context export and for the canonical detail pane.
    pub fn source_refs_for_canonical(
        &self,
        canonical_id: &CanonicalId,
    ) -> Result<Vec<SourceRef>> {
        let conn = self.conn();
        let mut stmt = conn
            .prepare_cached(
                "SELECT * FROM source_ref WHERE canonical_id = ?1 \
                 ORDER BY confidence DESC, source ASC",
            )
            .context("prepare source_refs_for_canonical")?;
        let rows = stmt
            .query_map(params![canonical_id], SourceRef::from_row)
            .context("query source_refs_for_canonical")?;
        rows.collect::<rusqlite::Result<Vec<_>>>()
            .context("collect source_refs_for_canonical")
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ids::ReleaseKind;

    fn fresh() -> Db {
        Db::open_in_memory().unwrap()
    }

    fn id(slug: &str) -> CanonicalId {
        CanonicalId::new(ReleaseKind::Tv, slug).unwrap()
    }

    fn with_canonical(db: &Db, id: &CanonicalId) {
        db.upsert_canonical(id, ReleaseKind::Tv, "X", 1).unwrap();
    }

    fn first_ref_for(db: &Db, cid: &CanonicalId) -> SourceRef {
        db.source_refs_for_canonical(cid)
            .unwrap()
            .into_iter()
            .next()
            .expect("expected at least one source_ref")
    }

    #[test]
    fn attach_inserts_when_pair_is_new() {
        let mut db = fresh();
        let cid = id("foo");
        with_canonical(&db, &cid);
        let out = db
            .attach_source_ref(&cid, "anilist", "21", Some("Foo"), 0.9)
            .unwrap();
        assert_eq!(out, AttachSourceRefOutcome::Inserted);
        let sr = first_ref_for(&db, &cid);
        assert_eq!(sr.source, "anilist");
        assert_eq!(sr.source_id, "21");
        assert_eq!(sr.raw_title.as_deref(), Some("Foo"));
        assert!((sr.confidence - 0.9).abs() < f64::EPSILON);
    }

    #[test]
    fn attach_same_canonical_refreshes_raw_title_and_confidence() {
        let mut db = fresh();
        let cid = id("foo");
        with_canonical(&db, &cid);
        db.attach_source_ref(&cid, "anilist", "21", Some("Foo"), 0.7).unwrap();
        let out = db
            .attach_source_ref(&cid, "anilist", "21", Some("Foo (updated)"), 0.95)
            .unwrap();
        assert_eq!(out, AttachSourceRefOutcome::RefreshedSameCanonical);
        let sr = first_ref_for(&db, &cid);
        assert_eq!(sr.raw_title.as_deref(), Some("Foo (updated)"));
        assert!((sr.confidence - 0.95).abs() < f64::EPSILON);
    }

    #[test]
    fn attach_different_canonical_refuses_silent_remap() {
        let mut db = fresh();
        let a = id("foo");
        let b = id("bar");
        with_canonical(&db, &a);
        with_canonical(&db, &b);
        db.attach_source_ref(&a, "anilist", "21", Some("Foo"), 0.9).unwrap();
        let err = db
            .attach_source_ref(&b, "anilist", "21", Some("Foo"), 0.9)
            .unwrap_err();
        let msg = format!("{err}");
        assert!(msg.contains("collision"), "got: {msg}");
        // The original mapping is untouched.
        let sr = first_ref_for(&db, &a);
        assert_eq!(sr.canonical_id, a);
        assert!(db.source_refs_for_canonical(&b).unwrap().is_empty());
    }

    #[test]
    fn attach_rejects_confidence_out_of_bounds() {
        let mut db = fresh();
        let cid = id("foo");
        with_canonical(&db, &cid);
        let too_high = db
            .attach_source_ref(&cid, "anilist", "21", None, 1.5)
            .unwrap_err();
        assert!(format!("{too_high}").contains("confidence"));
        let too_low = db
            .attach_source_ref(&cid, "anilist", "22", None, -0.1)
            .unwrap_err();
        assert!(format!("{too_low}").contains("confidence"));
    }

    #[test]
    fn source_refs_for_canonical_returns_all_attached() {
        let mut db = fresh();
        let cid = id("foo");
        with_canonical(&db, &cid);
        db.attach_source_ref(&cid, "anilist", "21", Some("AL"), 0.95).unwrap();
        db.attach_source_ref(&cid, "tmdb", "33", Some("TMDB"), 0.85).unwrap();
        let refs = db.source_refs_for_canonical(&cid).unwrap();
        assert_eq!(refs.len(), 2);
        // ORDER BY confidence DESC — AniList (0.95) before TMDB (0.85).
        assert_eq!(refs[0].source, "anilist");
        assert_eq!(refs[1].source, "tmdb");
    }

    #[test]
    fn attach_with_no_matching_canonical_fails_with_fk_error() {
        let mut db = fresh();
        let cid = id("ghost"); // never upserted
        let err = db
            .attach_source_ref(&cid, "anilist", "21", None, 0.9)
            .unwrap_err();
        let msg = format!("{err:#}");
        // sqlite returns "FOREIGN KEY constraint failed" — accept any
        // mention of FK.
        assert!(
            msg.to_uppercase().contains("FOREIGN KEY"),
            "expected FK error, got: {msg}"
        );
    }
}
