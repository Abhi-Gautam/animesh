//! Append-only event log over `engagement`.
//!
//! Replaces the v0.4 `watch_progress` table's "count of episodes seen"
//! shape with a typed event log. Each row is one event in a canonical's
//! life — opened, completed, paused, rated, snoozed, or system-verified.
//! The shape is deliberately denormalized: meta is opaque JSON,
//! decoded by whichever call site cares.
//!
//! Query patterns the rest of the codebase needs:
//!   * recent_engagement(since) — feed the LLM context export.
//!   * last_engagement(canonical, event) — dedupe for the notifier and
//!     the upcoming-drop computer.
//!   * engagement_for_canonical(canonical) — detail pane.

use std::fmt;
use std::str::FromStr;

use anyhow::{anyhow, Context, Result};
use rusqlite::{params, OptionalExtension, Row};

use crate::ids::CanonicalId;

use super::Db;

/// Engagement event kind. Mirrors the V0004 CHECK constraint on
/// `engagement.event`. Keep these in sync.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum EngagementEvent {
    /// User followed a deep link to play the title.
    Opened,
    /// User completed (or, for legacy backfill, "seen up to") a title.
    Completed,
    /// User paused an open session explicitly.
    Paused,
    /// User assigned a rating. Meta carries {score, max}.
    Rated,
    /// User snoozed a notification.
    Snoozed,
    /// System: streamer link verified playable for the user's region.
    Verified,
}

impl EngagementEvent {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Opened => "opened",
            Self::Completed => "completed",
            Self::Paused => "paused",
            Self::Rated => "rated",
            Self::Snoozed => "snoozed",
            Self::Verified => "verified",
        }
    }
}

impl fmt::Display for EngagementEvent {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

impl FromStr for EngagementEvent {
    type Err = anyhow::Error;
    fn from_str(s: &str) -> Result<Self> {
        match s {
            "opened" => Ok(Self::Opened),
            "completed" => Ok(Self::Completed),
            "paused" => Ok(Self::Paused),
            "rated" => Ok(Self::Rated),
            "snoozed" => Ok(Self::Snoozed),
            "verified" => Ok(Self::Verified),
            other => Err(anyhow!("unknown EngagementEvent {other:?}")),
        }
    }
}

/// One row of `engagement`. Opaque `meta` — decoded at the call site.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Engagement {
    pub id: i64,
    pub canonical_id: CanonicalId,
    pub event: EngagementEvent,
    pub occurred_at: i64,
    pub meta: Option<String>,
}

impl Engagement {
    fn from_row(row: &Row<'_>) -> rusqlite::Result<Self> {
        let event_str: String = row.get("event")?;
        let event = EngagementEvent::from_str(&event_str).map_err(|e| {
            rusqlite::Error::FromSqlConversionFailure(
                2,
                rusqlite::types::Type::Text,
                Box::new(std::io::Error::new(std::io::ErrorKind::InvalidData, e.to_string())),
            )
        })?;
        Ok(Self {
            id: row.get("id")?,
            canonical_id: row.get("canonical_id")?,
            event,
            occurred_at: row.get("occurred_at")?,
            meta: row.get("meta")?,
        })
    }
}

impl Db {
    /// Append an engagement event. Returns the new row's primary key.
    ///
    /// `meta` must be a JSON document if provided; it is stored as TEXT
    /// without validation — call sites that produce it should use
    /// serde_json::to_string. The reason for storing raw: the read
    /// path is type-erased and deserialization belongs at the boundary
    /// that knows the schema.
    pub fn append_engagement(
        &self,
        canonical_id: &CanonicalId,
        event: EngagementEvent,
        occurred_at: i64,
        meta: Option<&str>,
    ) -> Result<i64> {
        let conn = self.conn();
        conn.execute(
            "INSERT INTO engagement (canonical_id, event, occurred_at, meta) \
             VALUES (?1, ?2, ?3, ?4)",
            params![canonical_id, event.as_str(), occurred_at, meta],
        )
        .context("append_engagement")?;
        Ok(conn.last_insert_rowid())
    }

    /// Most recent engagement event globally, since the given
    /// occurred_at threshold (inclusive). Used by the context export
    /// to give an LLM the user's recent taste signal.
    ///
    /// `limit` caps the number of rows returned; 0 means "no limit"
    /// because SQLite treats LIMIT -1 as no cap and we want a typed
    /// "no cap" without exposing that.
    pub fn recent_engagement(&self, since: i64, limit: u32) -> Result<Vec<Engagement>> {
        let sql = if limit == 0 {
            "SELECT * FROM engagement WHERE occurred_at >= ?1 ORDER BY occurred_at DESC".to_string()
        } else {
            format!(
                "SELECT * FROM engagement WHERE occurred_at >= ?1 ORDER BY occurred_at DESC LIMIT {limit}"
            )
        };
        let conn = self.conn();
        let mut stmt = conn.prepare(&sql).context("prepare recent_engagement")?;
        let rows = stmt
            .query_map(params![since], Engagement::from_row)
            .context("query recent_engagement")?;
        rows.collect::<rusqlite::Result<Vec<_>>>()
            .context("collect recent_engagement")
    }

    /// Last engagement of the given kind for a canonical. The notifier
    /// uses this to dedupe (don't re-notify if we've already verified
    /// the same drop window) and the upcoming-drop computer uses it to
    /// avoid re-prompting an already-completed episode.
    pub fn last_engagement(
        &self,
        canonical_id: &CanonicalId,
        event: EngagementEvent,
    ) -> Result<Option<Engagement>> {
        self.conn()
            .query_row(
                "SELECT * FROM engagement \
                 WHERE canonical_id = ?1 AND event = ?2 \
                 ORDER BY occurred_at DESC LIMIT 1",
                params![canonical_id, event.as_str()],
                Engagement::from_row,
            )
            .optional()
            .context("last_engagement")
    }

    /// Every engagement event for one canonical, newest-first. Used by
    /// the detail pane and the LLM export.
    pub fn engagement_for_canonical(
        &self,
        canonical_id: &CanonicalId,
    ) -> Result<Vec<Engagement>> {
        let conn = self.conn();
        let mut stmt = conn
            .prepare(
                "SELECT * FROM engagement WHERE canonical_id = ?1 \
                 ORDER BY occurred_at DESC, id DESC",
            )
            .context("prepare engagement_for_canonical")?;
        let rows = stmt
            .query_map(params![canonical_id], Engagement::from_row)
            .context("query engagement_for_canonical")?;
        rows.collect::<rusqlite::Result<Vec<_>>>()
            .context("collect engagement_for_canonical")
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

    #[test]
    fn event_str_round_trip_covers_every_variant() {
        for ev in [
            EngagementEvent::Opened,
            EngagementEvent::Completed,
            EngagementEvent::Paused,
            EngagementEvent::Rated,
            EngagementEvent::Snoozed,
            EngagementEvent::Verified,
        ] {
            assert_eq!(EngagementEvent::from_str(ev.as_str()).unwrap(), ev);
        }
        assert!(EngagementEvent::from_str("bogus").is_err());
    }

    #[test]
    fn append_returns_new_row_id_and_persists() {
        let db = fresh();
        let cid = id("foo");
        with_canonical(&db, &cid);
        let id1 = db
            .append_engagement(&cid, EngagementEvent::Opened, 100, None)
            .unwrap();
        let id2 = db
            .append_engagement(
                &cid,
                EngagementEvent::Completed,
                200,
                Some(r#"{"seen":1}"#),
            )
            .unwrap();
        assert!(id2 > id1, "ids are monotonic");
        let all = db.engagement_for_canonical(&cid).unwrap();
        assert_eq!(all.len(), 2);
        // Newest first.
        assert_eq!(all[0].event, EngagementEvent::Completed);
        assert_eq!(all[0].meta.as_deref(), Some(r#"{"seen":1}"#));
        assert_eq!(all[1].event, EngagementEvent::Opened);
    }

    #[test]
    fn append_with_unknown_canonical_fails_with_fk_error() {
        let db = fresh();
        // Note: ghost canonical never upserted. The FK ON DELETE CASCADE
        // also blocks orphan inserts (FK is enforced at write time).
        let err = db
            .append_engagement(&id("ghost"), EngagementEvent::Opened, 100, None)
            .unwrap_err();
        assert!(format!("{err:#}").to_uppercase().contains("FOREIGN KEY"));
    }

    #[test]
    fn recent_engagement_respects_threshold_and_limit() {
        let db = fresh();
        let cid = id("foo");
        with_canonical(&db, &cid);
        for t in [100, 200, 300, 400, 500] {
            db.append_engagement(&cid, EngagementEvent::Opened, t, None)
                .unwrap();
        }
        // since=300 → 300, 400, 500
        let rows = db.recent_engagement(300, 0).unwrap();
        assert_eq!(rows.len(), 3);
        assert_eq!(rows[0].occurred_at, 500);
        assert_eq!(rows[2].occurred_at, 300);
        // limit=2 → 500, 400
        let rows = db.recent_engagement(0, 2).unwrap();
        assert_eq!(rows.len(), 2);
        assert_eq!(rows[0].occurred_at, 500);
        assert_eq!(rows[1].occurred_at, 400);
    }

    #[test]
    fn last_engagement_returns_only_matching_event() {
        let db = fresh();
        let cid = id("foo");
        with_canonical(&db, &cid);
        db.append_engagement(&cid, EngagementEvent::Opened, 100, None).unwrap();
        db.append_engagement(&cid, EngagementEvent::Verified, 200, None).unwrap();
        db.append_engagement(&cid, EngagementEvent::Opened, 300, None).unwrap();
        let last_open = db
            .last_engagement(&cid, EngagementEvent::Opened)
            .unwrap()
            .unwrap();
        assert_eq!(last_open.occurred_at, 300);
        let last_verified = db
            .last_engagement(&cid, EngagementEvent::Verified)
            .unwrap()
            .unwrap();
        assert_eq!(last_verified.occurred_at, 200);
        assert!(db
            .last_engagement(&cid, EngagementEvent::Rated)
            .unwrap()
            .is_none());
    }

    #[test]
    fn engagement_for_canonical_orders_newest_first_with_stable_tie_break() {
        let db = fresh();
        let cid = id("foo");
        with_canonical(&db, &cid);
        // Two events at the same occurred_at — the secondary ORDER BY
        // id DESC guarantees the most recently inserted ties win.
        let earlier = db
            .append_engagement(&cid, EngagementEvent::Opened, 100, Some("first"))
            .unwrap();
        let later = db
            .append_engagement(&cid, EngagementEvent::Opened, 100, Some("second"))
            .unwrap();
        assert!(later > earlier);
        let rows = db.engagement_for_canonical(&cid).unwrap();
        assert_eq!(rows.len(), 2);
        assert_eq!(rows[0].id, later);
        assert_eq!(rows[1].id, earlier);
    }

    #[test]
    fn append_then_read_round_trips_unicode_meta() {
        let db = fresh();
        let cid = id("foo");
        with_canonical(&db, &cid);
        let meta = r#"{"note":"ワンピース 🏴‍☠️","seen":7}"#;
        db.append_engagement(&cid, EngagementEvent::Rated, 100, Some(meta))
            .unwrap();
        let rows = db.engagement_for_canonical(&cid).unwrap();
        assert_eq!(rows[0].meta.as_deref(), Some(meta));
    }

    #[test]
    fn cascade_delete_removes_engagement_rows_when_canonical_deleted() {
        let db = fresh();
        let cid = id("foo");
        with_canonical(&db, &cid);
        for ev in [EngagementEvent::Opened, EngagementEvent::Completed] {
            db.append_engagement(&cid, ev, 100, None).unwrap();
        }
        assert!(db.delete_canonical(&cid).unwrap());
        let leftover: i64 = db
            .conn()
            .query_row("SELECT COUNT(*) FROM engagement", [], |r| r.get(0))
            .unwrap();
        assert_eq!(leftover, 0);
    }

    #[test]
    fn recent_engagement_is_global_across_canonicals() {
        let db = fresh();
        let a = id("a");
        let b = id("b");
        with_canonical(&db, &a);
        with_canonical(&db, &b);
        db.append_engagement(&a, EngagementEvent::Opened, 100, None).unwrap();
        db.append_engagement(&b, EngagementEvent::Verified, 200, None).unwrap();
        let rows = db.recent_engagement(0, 0).unwrap();
        assert_eq!(rows.len(), 2);
        // Newest first.
        assert_eq!(rows[0].canonical_id, b);
        assert_eq!(rows[1].canonical_id, a);
    }
}
