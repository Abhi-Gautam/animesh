//! Append-only event log over `engagement`.
//!
//! Each row is one event in a canonical's life. The DB stores meta as
//! opaque TEXT (JSON), but at the API boundary we expose a typed
//! [`EngagementMeta`] so callers never reach into raw JSON.
//!
//! Query patterns the rest of the codebase needs:
//!   * last_engagement(canonical, event) — dedupe for the notifier and
//!     the upcoming-drop computer.
//!   * engagement_for_canonical(canonical) — detail pane.

use std::fmt;
use std::str::FromStr;

use anyhow::{anyhow, Context, Result};
#[cfg(test)]
use rusqlite::OptionalExtension;
use rusqlite::{params, Row};
use serde::{Deserialize, Serialize};

use crate::ids::CanonicalId;

use super::Db;

/// Engagement event kind. Mirrors the V0004 CHECK constraint on
/// `engagement.event`. Keep these in sync.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(crate) enum EngagementEvent {
    /// User followed a deep link to play the title.
    Opened,
    /// User completed (or "seen up to") a title.
    Completed,
    /// User paused an open session explicitly.
    Paused,
    /// User assigned a rating.
    Rated,
    /// User snoozed a notification.
    Snoozed,
    /// System: streamer link verified playable for the user's region.
    Verified,
}

impl EngagementEvent {
    pub(crate) fn as_str(&self) -> &'static str {
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

/// Typed payload for events that carry meta. Events without payload
/// (`Opened`, `Paused`, `Snoozed`) store `None` at the DB level.
///
/// Serialized as plain JSON object (no tag) — the DB's separate `event`
/// column already encodes the discriminator, and a tag-free shape is
/// what existing rows already have.
#[derive(Debug, Clone, PartialEq)]
pub(crate) enum EngagementMeta {
    /// User watched up to episode `seen`.
    Completed { seen: i64 },
    /// Streamer link confirmed playable on a subscribed streamer.
    /// `url` accepts the legacy `deep_link` key on decode so historical
    /// rows from earlier sync-engine writes still round-trip.
    Verified { streamer: String, url: String },
    /// Rating on a 0..=`max` scale.
    Rated { score: i64, max: i64 },
}

impl EngagementMeta {
    /// Decode raw meta JSON given the event kind. Returns `None` when:
    /// - the raw is None (events without payload),
    /// - the event kind has no associated payload variant, or
    /// - the JSON fails to decode (corrupt row — surfaced silently so
    ///   one bad row does not crash the read path).
    pub(crate) fn decode(event: EngagementEvent, raw: Option<&str>) -> Option<Self> {
        let raw = raw?;
        match event {
            EngagementEvent::Completed => serde_json::from_str::<CompletedJson>(raw)
                .ok()
                .map(|m| Self::Completed { seen: m.seen }),
            EngagementEvent::Verified => {
                serde_json::from_str::<VerifiedJson>(raw)
                    .ok()
                    .map(|m| Self::Verified {
                        streamer: m.streamer,
                        url: m.url,
                    })
            }
            EngagementEvent::Rated => {
                serde_json::from_str::<RatedJson>(raw)
                    .ok()
                    .map(|m| Self::Rated {
                        score: m.score,
                        max: m.max,
                    })
            }
            EngagementEvent::Opened | EngagementEvent::Paused | EngagementEvent::Snoozed => None,
        }
    }

    /// Serialize to a JSON string for DB storage.
    pub(crate) fn encode(&self) -> Result<String> {
        match self {
            Self::Completed { seen } => serde_json::to_string(&CompletedJson { seen: *seen }),
            Self::Verified { streamer, url } => serde_json::to_string(&VerifiedJson {
                streamer: streamer.clone(),
                url: url.clone(),
            }),
            Self::Rated { score, max } => serde_json::to_string(&RatedJson {
                score: *score,
                max: *max,
            }),
        }
        .context("serialize EngagementMeta")
    }

    /// Render back to a `serde_json::Value` for export layers (the LLM
    /// context builder) that want a generic JSON tree.
    pub(crate) fn to_json_value(&self) -> serde_json::Value {
        match self {
            Self::Completed { seen } => serde_json::json!({ "seen": seen }),
            Self::Verified { streamer, url } => {
                serde_json::json!({ "streamer": streamer, "url": url })
            }
            Self::Rated { score, max } => serde_json::json!({ "score": score, "max": max }),
        }
    }
}

#[derive(Debug, Serialize, Deserialize)]
struct CompletedJson {
    seen: i64,
}

#[derive(Debug, Serialize, Deserialize)]
struct VerifiedJson {
    streamer: String,
    #[serde(alias = "deep_link")]
    url: String,
}

#[derive(Debug, Serialize, Deserialize)]
struct RatedJson {
    score: i64,
    max: i64,
}

/// Origin of an `Engagement` row. The TUI synthesizes one in-memory
/// `Completed` whenever the user marks watched, so we can react before
/// the durable write returns. Distinguishing the two prevents callers
/// from treating the synthesized row as authoritative.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum EngagementSource {
    /// Row read from the DB; carries its rowid.
    Persisted(i64),
    /// Synthesized in-memory; not yet (or never) persisted.
    InMemory,
}

/// One engagement row, typed end-to-end.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct Engagement {
    pub source: EngagementSource,
    pub canonical_id: CanonicalId,
    pub event: EngagementEvent,
    pub occurred_at: i64,
    pub meta: Option<EngagementMeta>,
}

impl Engagement {
    /// `seen` count if this is a `Completed` event with payload.
    pub(crate) fn seen(&self) -> Option<i64> {
        match &self.meta {
            Some(EngagementMeta::Completed { seen }) => Some(*seen),
            _ => None,
        }
    }

    /// Verified-streamer name if this is a `Verified` event.
    pub(crate) fn streamer(&self) -> Option<&str> {
        match &self.meta {
            Some(EngagementMeta::Verified { streamer, .. }) => Some(streamer),
            _ => None,
        }
    }

    /// Verified-link URL if this is a `Verified` event.
    pub(crate) fn verified_url(&self) -> Option<&str> {
        match &self.meta {
            Some(EngagementMeta::Verified { url, .. }) => Some(url),
            _ => None,
        }
    }

    fn from_row(row: &Row<'_>) -> rusqlite::Result<Self> {
        let event_str: String = row.get("event")?;
        let event = EngagementEvent::from_str(&event_str).map_err(|e| {
            rusqlite::Error::FromSqlConversionFailure(
                2,
                rusqlite::types::Type::Text,
                Box::new(std::io::Error::new(
                    std::io::ErrorKind::InvalidData,
                    e.to_string(),
                )),
            )
        })?;
        let raw_meta: Option<String> = row.get("meta")?;
        let meta = EngagementMeta::decode(event, raw_meta.as_deref());
        let id: i64 = row.get("id")?;
        Ok(Self {
            source: EngagementSource::Persisted(id),
            canonical_id: row.get("canonical_id")?,
            event,
            occurred_at: row.get("occurred_at")?,
            meta,
        })
    }
}

impl Db {
    /// Append an engagement event. Returns the new row's primary key.
    pub(crate) fn append_engagement(
        &self,
        canonical_id: &CanonicalId,
        event: EngagementEvent,
        occurred_at: i64,
        meta: Option<&EngagementMeta>,
    ) -> Result<i64> {
        let meta_json = meta.map(|m| m.encode()).transpose()?;
        let conn = self.conn();
        conn.execute(
            "INSERT INTO engagement (canonical_id, event, occurred_at, meta) \
             VALUES (?1, ?2, ?3, ?4)",
            params![canonical_id, event.as_str(), occurred_at, meta_json],
        )
        .context("append_engagement")?;
        Ok(conn.last_insert_rowid())
    }

    /// Last engagement of the given kind for a canonical. Production
    /// reads pull this through the [`crate::library::Library::load_resolved`]
    /// join; the standalone form survives for tests.
    #[cfg(test)]
    pub(crate) fn last_engagement(
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
    pub(crate) fn engagement_for_canonical(&self, canonical_id: &CanonicalId) -> Result<Vec<Engagement>> {
        let conn = self.conn();
        let mut stmt = conn
            .prepare_cached(
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
    fn event_str_round_trips() {
        for ev in [
            EngagementEvent::Opened,
            EngagementEvent::Completed,
            EngagementEvent::Paused,
            EngagementEvent::Rated,
            EngagementEvent::Snoozed,
            EngagementEvent::Verified,
        ] {
            let s = ev.as_str();
            assert_eq!(EngagementEvent::from_str(s).unwrap(), ev);
        }
    }

    #[test]
    fn from_str_rejects_unknown() {
        assert!(EngagementEvent::from_str("bogus").is_err());
    }

    #[test]
    fn meta_completed_round_trips_through_encode_decode() {
        let m = EngagementMeta::Completed { seen: 7 };
        let raw = m.encode().unwrap();
        let back = EngagementMeta::decode(EngagementEvent::Completed, Some(&raw)).unwrap();
        assert_eq!(back, m);
    }

    #[test]
    fn meta_verified_round_trips_and_accepts_deep_link_alias() {
        let m = EngagementMeta::Verified {
            streamer: "Netflix".into(),
            url: "https://netflix.com/x".into(),
        };
        let raw = m.encode().unwrap();
        let back = EngagementMeta::decode(EngagementEvent::Verified, Some(&raw)).unwrap();
        assert_eq!(back, m);

        // Legacy row with `deep_link` instead of `url` decodes the same.
        let legacy = r#"{"streamer":"Netflix","deep_link":"https://netflix.com/x"}"#;
        let from_legacy = EngagementMeta::decode(EngagementEvent::Verified, Some(legacy)).unwrap();
        assert_eq!(from_legacy, m);
    }

    #[test]
    fn meta_decode_returns_none_for_payloadless_events() {
        for ev in [
            EngagementEvent::Opened,
            EngagementEvent::Paused,
            EngagementEvent::Snoozed,
        ] {
            assert!(EngagementMeta::decode(ev, Some(r#"{"anything":1}"#)).is_none());
        }
    }

    #[test]
    fn meta_decode_returns_none_for_corrupt_row() {
        assert!(EngagementMeta::decode(EngagementEvent::Completed, Some("not json")).is_none());
    }

    #[test]
    fn append_engagement_with_no_meta_persists_null() {
        let db = fresh();
        let cid = id("foo");
        with_canonical(&db, &cid);
        let row_id = db
            .append_engagement(&cid, EngagementEvent::Opened, 100, None)
            .unwrap();
        assert!(row_id > 0);
        let rows = db.engagement_for_canonical(&cid).unwrap();
        assert_eq!(rows[0].event, EngagementEvent::Opened);
        assert_eq!(rows[0].meta, None);
        assert!(matches!(rows[0].source, EngagementSource::Persisted(id) if id == row_id));
    }

    #[test]
    fn append_then_read_typed_meta() {
        let db = fresh();
        let cid = id("foo");
        with_canonical(&db, &cid);
        let meta = EngagementMeta::Completed { seen: 5 };
        db.append_engagement(&cid, EngagementEvent::Completed, 100, Some(&meta))
            .unwrap();
        let last = db
            .last_engagement(&cid, EngagementEvent::Completed)
            .unwrap()
            .unwrap();
        assert_eq!(last.seen(), Some(5));
    }

    #[test]
    fn append_with_unknown_canonical_fails_with_fk_error() {
        let db = fresh();
        let err = db
            .append_engagement(&id("ghost"), EngagementEvent::Opened, 100, None)
            .unwrap_err();
        assert!(format!("{err:#}").to_uppercase().contains("FOREIGN KEY"));
    }

    #[test]
    fn last_engagement_returns_only_matching_event() {
        let db = fresh();
        let cid = id("foo");
        with_canonical(&db, &cid);
        db.append_engagement(&cid, EngagementEvent::Opened, 100, None)
            .unwrap();
        db.append_engagement(&cid, EngagementEvent::Verified, 200, None)
            .unwrap();
        db.append_engagement(&cid, EngagementEvent::Opened, 300, None)
            .unwrap();
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
        let earlier = db
            .append_engagement(&cid, EngagementEvent::Opened, 100, None)
            .unwrap();
        let later = db
            .append_engagement(&cid, EngagementEvent::Opened, 100, None)
            .unwrap();
        assert!(later > earlier);
        let rows = db.engagement_for_canonical(&cid).unwrap();
        assert_eq!(rows.len(), 2);
        assert!(matches!(rows[0].source, EngagementSource::Persisted(id) if id == later));
        assert!(matches!(rows[1].source, EngagementSource::Persisted(id) if id == earlier));
    }

    #[test]
    fn meta_to_json_value_matches_decoded_input() {
        let raw = r#"{"streamer":"Netflix","url":"https://x"}"#;
        let m = EngagementMeta::decode(EngagementEvent::Verified, Some(raw)).unwrap();
        let v = m.to_json_value();
        assert_eq!(v["streamer"], "Netflix");
        assert_eq!(v["url"], "https://x");
    }

    #[test]
    fn engagement_source_persisted_vs_in_memory() {
        let db = fresh();
        let cid = id("foo");
        with_canonical(&db, &cid);
        db.append_engagement(&cid, EngagementEvent::Opened, 100, None)
            .unwrap();
        let row = db
            .last_engagement(&cid, EngagementEvent::Opened)
            .unwrap()
            .unwrap();
        assert!(matches!(row.source, EngagementSource::Persisted(_)));

        let synth = Engagement {
            source: EngagementSource::InMemory,
            canonical_id: cid,
            event: EngagementEvent::Completed,
            occurred_at: 500,
            meta: Some(EngagementMeta::Completed { seen: 3 }),
        };
        assert!(matches!(synth.source, EngagementSource::InMemory));
        assert_eq!(synth.seen(), Some(3));
    }
}
