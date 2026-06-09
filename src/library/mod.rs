//! Library — the single entry point for business operations.
//!
//! Everything above this module (TUI handlers, CLI commands, sync
//! loop) is thin marshalling: parse input, call Library, render output.
//! Everything below (store, sources, llm) does only what Library asks.
//!
//! The architectural rule, in one sentence:
//!
//! > There is exactly one place to ask "is this followed?" and exactly
//! > one place that answers it.
//!
//! Library is that place.
//!
//! ## Layering
//!
//! Library does NOT canonicalize titles. The canonicalization service
//! (see `crate::canonical`) decides which canonical_id a source row
//! maps to and calls into Library to read/write. Library exposes
//! primitives like [`Library::follow_with_source`] that take a
//! pre-decided [`CanonicalId`] and stitch the canonical_release +
//! source_ref + followed_at flip together atomically (within one db
//! lock).
//!
//! ## Clock injection
//!
//! Every "now-ish" timestamp comes from an injected [`Clock`]. Tests
//! use [`crate::time::FixedClock`] to fix the timeline; production uses
//! [`crate::time::SystemClock`]. Library never calls `SystemTime::now`
//! directly.

use std::path::Path;
use std::sync::{Arc, Mutex};

use anyhow::{Context, Result};

use crate::ids::{CanonicalId, ReleaseKind};
use crate::ingest::{RawSourcePayload, SourceObservation};
use crate::search::source_candidate::SourceCandidateResult;
use crate::store::{
    CacheEntry, CanonicalFollowOutcome, CanonicalRelease, Db, Engagement, EngagementEvent,
    EngagementMeta, EngagementSource, SourceParseError, SourceRef,
};
use crate::time::Clock;

/// One fully-resolved canonical the TUI can render directly. Produced
/// by [`Library::load_resolved`] in a single query — no per-row joins
/// from the caller.
#[derive(Debug, Clone)]
pub struct ResolvedRelease {
    pub canonical: CanonicalRelease,
    /// Highest-confidence attached source_ref. Invariant: a followed
    /// canonical has at least one attached ref (enforced at follow
    /// time), so this is never absent for the rows `load_resolved`
    /// returns.
    pub primary_source: SourceRef,
    pub cache: Option<CacheEntry>,
    pub last_completed: Option<Engagement>,
    pub last_verified: Option<Engagement>,
}

/// The domain facade. Hold one of these per process; share via
/// `Arc<Library>` to async tasks and the TUI.
pub struct Library {
    db: Mutex<Db>,
    clock: Arc<dyn Clock>,
}

impl Library {
    /// Open a Library backed by the on-disk DB at `path`. Creates the
    /// file (and parent dirs) and runs migrations on first open.
    pub fn open(path: &Path, clock: Arc<dyn Clock>) -> Result<Self> {
        let db = Db::open(path).context("open Library DB")?;
        Ok(Self {
            db: Mutex::new(db),
            clock,
        })
    }

    /// In-memory Library for tests. Migrations always run.
    #[cfg(test)]
    pub fn open_in_memory(clock: Arc<dyn Clock>) -> Result<Self> {
        let db = Db::open_in_memory().context("open in-memory Library DB")?;
        Ok(Self {
            db: Mutex::new(db),
            clock,
        })
    }

    fn now(&self) -> i64 {
        self.clock.now()
    }

    // ------------------------------------------------------------------
    // Follow / drop
    // ------------------------------------------------------------------

    /// Atomically ensure the canonical exists, attach this source_ref,
    /// and flip followed_at if not already followed. The three writes
    /// happen under one mutex hold; a crash between them leaves the
    /// system in a state any retry will idempotently complete.
    ///
    /// `confidence` is the canonicalizer's stated likelihood that this
    /// source row maps to the given canonical. Legacy / user-confirmed
    /// rows use 1.0.
    #[allow(clippy::too_many_arguments)]
    pub fn follow_with_source(
        &self,
        canonical_id: &CanonicalId,
        kind: ReleaseKind,
        display_title: &str,
        source: &str,
        source_id: &str,
        raw_title: Option<&str>,
        confidence: f64,
    ) -> Result<CanonicalFollowOutcome> {
        let mut db = self.lock_db()?;
        let now = self.clock.now();
        db.upsert_canonical(canonical_id, kind, display_title, now)?;
        db.attach_source_ref(canonical_id, source, source_id, raw_title, confidence)?;
        db.follow_canonical(canonical_id, now)
    }

    /// Soft-drop a canonical. Idempotent. Returns whether a row was
    /// touched (false = canonical not present or never followed).
    pub fn drop_canonical(&self, canonical_id: &CanonicalId) -> Result<bool> {
        let db = self.lock_db()?;
        db.drop_canonical(canonical_id, self.now())
    }

    // ------------------------------------------------------------------
    // Reads
    // ------------------------------------------------------------------

    /// Active follows, newest-first by followed_at.
    pub fn followed(&self) -> Result<Vec<CanonicalRelease>> {
        let db = self.lock_db()?;
        db.list_active_canonical()
    }

    /// Active follows with their primary source_ref, cache row, and
    /// last `Completed` + `Verified` engagement events — all in one
    /// prepared, cached query. Replaces the N+1 the TUI's `Shelf::load`
    /// used to do.
    pub fn load_resolved(&self) -> Result<Vec<ResolvedRelease>> {
        let db = self.lock_db()?;
        load_resolved_rows(&db)
    }

    #[cfg(test)]
    pub fn find_canonical(&self, id: &CanonicalId) -> Result<Option<CanonicalRelease>> {
        let db = self.lock_db()?;
        db.find_canonical(id)
    }

    pub fn count_followed(&self) -> Result<i64> {
        let db = self.lock_db()?;
        db.count_followed_canonical()
    }

    // ------------------------------------------------------------------
    // Engagement
    // ------------------------------------------------------------------

    /// Append an engagement event. The payload (if any) is typed by
    /// [`EngagementMeta`]; the store encodes it to JSON for the column.
    pub fn engage(
        &self,
        canonical_id: &CanonicalId,
        event: EngagementEvent,
        meta: Option<EngagementMeta>,
    ) -> Result<()> {
        let db = self.lock_db()?;
        db.append_engagement(canonical_id, event, self.now(), meta.as_ref())?;
        Ok(())
    }

    /// Last engagement of a given event kind for one canonical.
    /// Tests use this directly; production reads go through
    /// [`Library::load_resolved`] which folds the same lookup into a
    /// single join.
    #[cfg(test)]
    pub fn last_engagement(
        &self,
        canonical_id: &CanonicalId,
        event: EngagementEvent,
    ) -> Result<Option<Engagement>> {
        let db = self.lock_db()?;
        db.last_engagement(canonical_id, event)
    }

    /// Engagement events for one canonical, newest-first.
    pub fn engagement_for(&self, canonical_id: &CanonicalId) -> Result<Vec<Engagement>> {
        let db = self.lock_db()?;
        db.engagement_for_canonical(canonical_id)
    }

    // ------------------------------------------------------------------
    // Source refs and metadata cache (used by the sync engine).
    // ------------------------------------------------------------------

    pub fn source_refs_for(&self, canonical_id: &CanonicalId) -> Result<Vec<SourceRef>> {
        let db = self.lock_db()?;
        db.source_refs_for_canonical(canonical_id)
    }

    /// Test-only — production reads source the cache through
    /// [`Library::load_resolved`].
    #[cfg(test)]
    pub fn get_cache(&self, source: &str, source_id: &str) -> Result<Option<CacheEntry>> {
        let db = self.lock_db()?;
        db.get_cache(source, source_id)
    }

    pub fn upsert_cache(&self, entry: &CacheEntry) -> Result<()> {
        let db = self.lock_db()?;
        db.upsert_cache(entry)
    }

    // ------------------------------------------------------------------
    // Source ingestion: raw payloads -> observations -> candidates.
    // ------------------------------------------------------------------

    pub fn store_raw_source_payload(&self, payload: &RawSourcePayload) -> Result<()> {
        let db = self.lock_db()?;
        db.upsert_raw_source_payload(payload)
    }

    pub fn store_source_observation(&self, observation: &SourceObservation) -> Result<()> {
        let mut db = self.lock_db()?;
        db.upsert_source_observation(observation)
    }

    #[allow(dead_code)]
    pub fn record_source_parse_error(&self, err: &SourceParseError) -> Result<()> {
        let db = self.lock_db()?;
        db.insert_source_parse_error(err)
    }

    pub fn search_source_candidates(
        &self,
        query: &str,
        limit: u32,
    ) -> Result<Vec<SourceCandidateResult>> {
        let db = self.lock_db()?;
        db.search_source_candidates(query, limit)
    }

    // ------------------------------------------------------------------
    // Small kv store (used by the notifier for dedupe markers,
    // by sync for last-tick state).
    // ------------------------------------------------------------------

    pub fn kv_get(&self, key: &str) -> Result<Option<String>> {
        let db = self.lock_db()?;
        Ok(db.kv_get(key)?.map(|(value, _)| value))
    }

    pub fn kv_set(&self, key: &str, value: &str) -> Result<()> {
        let db = self.lock_db()?;
        db.kv_set(key, value, self.now())
    }

    /// Remove a key. No-op if missing.
    pub fn kv_delete(&self, key: &str) -> Result<()> {
        let db = self.lock_db()?;
        db.kv_delete(key)
    }

    /// Persisted streamer subscriptions (canonical-lowercase strings).
    /// Backed by kv key `subs.streaming` (JSON array). Empty when unset.
    pub fn subscribed_streamers(&self) -> Result<Vec<String>> {
        let Some(raw) = self.kv_get("subs.streaming")? else {
            return Ok(Vec::new());
        };
        serde_json::from_str(&raw).with_context(|| format!("decode subs.streaming: {raw:?}"))
    }

    /// Overwrite the streamer subscription list. Empty input clears the key.
    pub fn set_subscribed_streamers(&self, streamers: &[String]) -> Result<()> {
        if streamers.is_empty() {
            return self.kv_delete("subs.streaming");
        }
        let json = serde_json::to_string(streamers).context("encode subs")?;
        self.kv_set("subs.streaming", &json)
    }

    // ------------------------------------------------------------------
    // Internals
    // ------------------------------------------------------------------

    fn lock_db(&self) -> Result<std::sync::MutexGuard<'_, Db>> {
        self.db
            .lock()
            .map_err(|_| anyhow::anyhow!("Library DB mutex poisoned"))
    }
}

/// One prepared-and-cached statement runs every per-frame resolved
/// load. The window-function picks the highest-confidence source_ref
/// per canonical and the newest `Completed`/`Verified` event per
/// canonical — all in a single round trip.
const LOAD_RESOLVED_SQL: &str = "\
WITH primary_sr AS (\
    SELECT canonical_id, source, source_id, raw_title, confidence \
    FROM (\
        SELECT canonical_id, source, source_id, raw_title, confidence, \
               ROW_NUMBER() OVER ( \
                   PARTITION BY canonical_id \
                   ORDER BY confidence DESC, source ASC \
               ) AS rn \
        FROM source_ref\
    ) WHERE rn = 1\
), \
last_engagement_by_event AS (\
    SELECT canonical_id, event, id, occurred_at, meta \
    FROM (\
        SELECT canonical_id, event, id, occurred_at, meta, \
               ROW_NUMBER() OVER ( \
                   PARTITION BY canonical_id, event \
                   ORDER BY occurred_at DESC, id DESC \
               ) AS rn \
        FROM engagement \
        WHERE event IN ('completed', 'verified')\
    ) WHERE rn = 1\
) \
SELECT \
    cr.id, cr.kind, cr.display_title, cr.cover_ascii, cr.cover_color, \
    cr.followed_at, cr.dropped_at, cr.user_note, cr.created_at, \
    psr.source, psr.source_id, psr.raw_title, psr.confidence, \
    mc.display_title, mc.title_english, mc.title_native, mc.status, \
    mc.total_episodes, mc.format, mc.next_episode_number, mc.next_episode_airs_at, \
    mc.fetched_at, mc.expires_at, mc.cover_image_url, mc.description, \
    mc.score, mc.studios, mc.streaming_links_json, \
    lc.id, lc.occurred_at, lc.meta, \
    lv.id, lv.occurred_at, lv.meta \
FROM canonical_release cr \
JOIN primary_sr psr ON psr.canonical_id = cr.id \
LEFT JOIN metadata_cache mc \
    ON mc.source = psr.source AND mc.source_id = psr.source_id \
LEFT JOIN last_engagement_by_event lc \
    ON lc.canonical_id = cr.id AND lc.event = 'completed' \
LEFT JOIN last_engagement_by_event lv \
    ON lv.canonical_id = cr.id AND lv.event = 'verified' \
WHERE cr.followed_at IS NOT NULL AND cr.dropped_at IS NULL \
ORDER BY cr.followed_at DESC";

fn load_resolved_rows(db: &Db) -> Result<Vec<ResolvedRelease>> {
    let conn = db.conn();
    let mut stmt = conn
        .prepare_cached(LOAD_RESOLVED_SQL)
        .context("prepare load_resolved")?;
    let rows = stmt
        .query_map([], map_resolved_row)
        .context("query load_resolved")?;
    rows.collect::<rusqlite::Result<Vec<_>>>()
        .context("collect load_resolved rows")
}

fn map_resolved_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<ResolvedRelease> {
    // canonical_release block (cols 0..9).
    let cr_id: CanonicalId = row.get(0)?;
    let kind_str: String = row.get(1)?;
    let kind = kind_str.parse::<ReleaseKind>().map_err(|e| {
        rusqlite::Error::FromSqlConversionFailure(
            1,
            rusqlite::types::Type::Text,
            Box::new(std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                e.to_string(),
            )),
        )
    })?;
    let canonical = CanonicalRelease {
        id: cr_id.clone(),
        kind,
        display_title: row.get(2)?,
        cover_ascii: row.get(3)?,
        cover_color: row.get(4)?,
        followed_at: row.get(5)?,
        dropped_at: row.get(6)?,
        user_note: row.get(7)?,
        created_at: row.get(8)?,
    };

    // primary source_ref (cols 9..13).
    let primary_source = SourceRef {
        canonical_id: cr_id.clone(),
        source: row.get(9)?,
        source_id: row.get(10)?,
        raw_title: row.get(11)?,
        confidence: row.get(12)?,
    };

    // metadata_cache (cols 13..28). `fetched_at` is NOT NULL when the
    // LEFT JOIN matched; we use it as the "row exists" signal.
    let mc_fetched_at: Option<i64> = row.get(21)?;
    let cache = if let Some(fetched_at) = mc_fetched_at {
        Some(CacheEntry {
            source: primary_source.source.clone(),
            source_id: primary_source.source_id.clone(),
            display_title: row.get(13)?,
            title_english: row.get(14)?,
            title_native: row.get(15)?,
            status: row.get(16)?,
            total_episodes: row.get(17)?,
            format: row.get(18)?,
            next_episode_number: row.get(19)?,
            next_episode_airs_at: row.get(20)?,
            fetched_at,
            expires_at: row.get(22)?,
            cover_image_url: row.get(23)?,
            description: row.get(24)?,
            score: row.get(25)?,
            studios: row.get(26)?,
            streaming_links_json: row.get(27)?,
        })
    } else {
        None
    };

    // last_completed engagement (cols 28..31).
    let last_completed = engagement_from_join(row, 28, EngagementEvent::Completed, &cr_id)?;

    // last_verified engagement (cols 31..34).
    let last_verified = engagement_from_join(row, 31, EngagementEvent::Verified, &cr_id)?;

    Ok(ResolvedRelease {
        canonical,
        primary_source,
        cache,
        last_completed,
        last_verified,
    })
}

fn engagement_from_join(
    row: &rusqlite::Row<'_>,
    base: usize,
    event: EngagementEvent,
    canonical_id: &CanonicalId,
) -> rusqlite::Result<Option<Engagement>> {
    let id: Option<i64> = row.get(base)?;
    let Some(id) = id else { return Ok(None) };
    let occurred_at: i64 = row.get(base + 1)?;
    let raw_meta: Option<String> = row.get(base + 2)?;
    Ok(Some(Engagement {
        source: EngagementSource::Persisted(id),
        canonical_id: canonical_id.clone(),
        event,
        occurred_at,
        meta: EngagementMeta::decode(event, raw_meta.as_deref()),
    }))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::time::{AdvanceableClock, FixedClock};

    fn lib_at(t: i64) -> Library {
        Library::open_in_memory(Arc::new(FixedClock(t))).unwrap()
    }

    fn id(slug: &str) -> CanonicalId {
        CanonicalId::new(ReleaseKind::Tv, slug).unwrap()
    }

    #[test]
    fn follow_with_source_creates_canonical_and_attaches_ref() {
        let lib = lib_at(1_000);
        let cid = id("severance");
        let out = lib
            .follow_with_source(
                &cid,
                ReleaseKind::Tv,
                "Severance",
                "tmdb",
                "95396",
                Some("Severance"),
                0.95,
            )
            .unwrap();
        assert_eq!(out, CanonicalFollowOutcome::NewlyFollowed);

        let row = lib.find_canonical(&cid).unwrap().unwrap();
        assert_eq!(row.display_title, "Severance");
        assert_eq!(row.followed_at, Some(1_000));
        assert_eq!(row.created_at, 1_000);

        let refs = lib.source_refs_for(&cid).unwrap();
        assert!(refs
            .iter()
            .any(|r| r.source == "tmdb" && r.source_id == "95396"));
    }

    #[test]
    fn second_source_ref_for_same_canonical_extends_mapping() {
        let lib = lib_at(1_000);
        let cid = id("severance");
        lib.follow_with_source(
            &cid,
            ReleaseKind::Tv,
            "Severance",
            "tmdb",
            "95396",
            None,
            0.95,
        )
        .unwrap();
        // Same canonical, different source — both refs map.
        let out = lib
            .follow_with_source(
                &cid,
                ReleaseKind::Tv,
                "Severance",
                "tvmaze",
                "44060",
                None,
                0.9,
            )
            .unwrap();
        // The canonical was already followed; re-following is a no-op
        // outcome, even when adding a second source ref.
        assert_eq!(out, CanonicalFollowOutcome::AlreadyFollowing);
        let refs = lib.source_refs_for(&cid).unwrap();
        assert!(refs
            .iter()
            .any(|r| r.source == "tmdb" && r.source_id == "95396"));
        assert!(refs
            .iter()
            .any(|r| r.source == "tvmaze" && r.source_id == "44060"));
    }

    #[test]
    fn drop_then_refollow_restores_with_outcome() {
        let clock = AdvanceableClock::new(1_000);
        let lib = Library::open_in_memory(Arc::new(clock.clone())).unwrap();
        let cid = id("severance");
        lib.follow_with_source(&cid, ReleaseKind::Tv, "Severance", "tmdb", "1", None, 1.0)
            .unwrap();
        clock.advance(100);
        assert!(lib.drop_canonical(&cid).unwrap());
        assert_eq!(lib.followed().unwrap().len(), 0);
        clock.advance(100);
        let out = lib
            .follow_with_source(&cid, ReleaseKind::Tv, "Severance", "tmdb", "1", None, 1.0)
            .unwrap();
        assert_eq!(out, CanonicalFollowOutcome::RestoredFromDrop);
        assert_eq!(lib.followed().unwrap().len(), 1);
    }

    #[test]
    fn followed_returns_only_active_rows_newest_first() {
        let clock = AdvanceableClock::new(1_000);
        let lib = Library::open_in_memory(Arc::new(clock.clone())).unwrap();
        for (slug, dt) in [("a", 0), ("b", 100), ("c", 200)] {
            clock.set(1_000 + dt);
            lib.follow_with_source(&id(slug), ReleaseKind::Tv, slug, "tmdb", slug, None, 1.0)
                .unwrap();
        }
        let followed = lib.followed().unwrap();
        let slugs: Vec<&str> = followed.iter().map(|r| r.id.slug()).collect();
        // Newest follow first.
        assert_eq!(slugs, ["c", "b", "a"]);
    }

    #[test]
    fn engage_appends_event_with_clock_now() {
        let clock = AdvanceableClock::new(1_000);
        let lib = Library::open_in_memory(Arc::new(clock.clone())).unwrap();
        let cid = id("severance");
        lib.follow_with_source(&cid, ReleaseKind::Tv, "Severance", "tmdb", "1", None, 1.0)
            .unwrap();
        clock.advance(500);
        lib.engage(&cid, EngagementEvent::Opened, None).unwrap();
        clock.advance(500);
        lib.engage(
            &cid,
            EngagementEvent::Completed,
            Some(EngagementMeta::Completed { seen: 1 }),
        )
        .unwrap();
        let events = lib.engagement_for(&cid).unwrap();
        assert_eq!(events.len(), 2);
        // Newest first.
        assert_eq!(events[0].event, EngagementEvent::Completed);
        assert_eq!(events[0].occurred_at, 2_000);
        assert_eq!(events[1].event, EngagementEvent::Opened);
        assert_eq!(events[1].occurred_at, 1_500);
    }

    #[test]
    fn last_engagement_returns_matching_event() {
        let clock = AdvanceableClock::new(1_000);
        let lib = Library::open_in_memory(Arc::new(clock.clone())).unwrap();
        let cid = id("x");
        lib.follow_with_source(&cid, ReleaseKind::Tv, "X", "tmdb", "1", None, 1.0)
            .unwrap();
        clock.set(2_000);
        lib.engage(&cid, EngagementEvent::Verified, None).unwrap();
        clock.set(3_000);
        lib.engage(&cid, EngagementEvent::Opened, None).unwrap();
        let v = lib
            .last_engagement(&cid, EngagementEvent::Verified)
            .unwrap()
            .unwrap();
        assert_eq!(v.occurred_at, 2_000);
        assert!(lib
            .last_engagement(&cid, EngagementEvent::Rated)
            .unwrap()
            .is_none());
    }

    #[test]
    fn library_is_arc_shareable_across_threads() {
        // The TUI render loop + sync loop hold the same Arc<Library>;
        // confirm the API survives ordinary cross-thread use.
        let lib = Arc::new(lib_at(1_000));
        let cid = id("severance");
        lib.follow_with_source(&cid, ReleaseKind::Tv, "Severance", "tmdb", "1", None, 1.0)
            .unwrap();
        let handles: Vec<_> = (0..4)
            .map(|i| {
                let lib = Arc::clone(&lib);
                let cid = cid.clone();
                std::thread::spawn(move || {
                    let event = if i % 2 == 0 {
                        EngagementEvent::Opened
                    } else {
                        EngagementEvent::Paused
                    };
                    lib.engage(&cid, event, None).unwrap();
                })
            })
            .collect();
        for h in handles {
            h.join().unwrap();
        }
        let events = lib.engagement_for(&cid).unwrap();
        assert_eq!(events.len(), 4);
    }

    #[test]
    fn count_followed_tracks_active_set() {
        let lib = lib_at(1_000);
        assert_eq!(lib.count_followed().unwrap(), 0);
        for slug in ["a", "b", "c"] {
            lib.follow_with_source(&id(slug), ReleaseKind::Tv, slug, "tmdb", slug, None, 1.0)
                .unwrap();
        }
        assert_eq!(lib.count_followed().unwrap(), 3);
        lib.drop_canonical(&id("b")).unwrap();
        assert_eq!(lib.count_followed().unwrap(), 2);
    }

    #[test]
    fn subscribed_streamers_roundtrip() {
        let lib = Library::open_in_memory(Arc::new(FixedClock(1))).unwrap();
        assert!(lib.subscribed_streamers().unwrap().is_empty());
        lib.set_subscribed_streamers(&["Netflix".to_string(), "Crunchyroll".to_string()])
            .unwrap();
        assert_eq!(
            lib.subscribed_streamers().unwrap(),
            vec!["Netflix".to_string(), "Crunchyroll".to_string()]
        );
    }

    #[test]
    fn subscribed_streamers_clears_when_empty() {
        let lib = Library::open_in_memory(Arc::new(FixedClock(1))).unwrap();
        lib.set_subscribed_streamers(&["Netflix".to_string()])
            .unwrap();
        lib.set_subscribed_streamers(&[]).unwrap();
        assert!(lib.subscribed_streamers().unwrap().is_empty());
    }

    #[test]
    fn subscribed_streamers_surfaces_corrupt_json() {
        let lib = Library::open_in_memory(Arc::new(FixedClock(1))).unwrap();
        lib.kv_set("subs.streaming", "not json").unwrap();
        let err = lib.subscribed_streamers().unwrap_err();
        let msg = format!("{err:#}");
        assert!(msg.contains("decode subs.streaming"), "got: {msg}");
    }

    // ------------------------------------------------------------------
    // source ingestion — raw payloads -> observations -> candidates
    // ------------------------------------------------------------------

    #[test]
    fn source_observation_materializes_searchable_candidate() {
        use crate::ingest::{
            AliasObservation, ExternalIdObservation, HttpMethod, RawSourcePayload,
            SourceObservation,
        };

        let lib = lib_at(1_000);
        let raw = RawSourcePayload {
            id: "raw:tvmaze:1".into(),
            source: "tvmaze".into(),
            endpoint: "search_shows".into(),
            method: HttpMethod::Get,
            request_key: "tvmaze:search:severance".into(),
            request_hash: "req-hash".into(),
            request_json: None,
            http_status: 200,
            response_hash: "resp-hash".into(),
            response_json: r#"{"ok":true}"#.into(),
            fetched_at: 1_000,
            expires_at: Some(2_000),
            created_at: 1_000,
        };
        lib.store_raw_source_payload(&raw).unwrap();

        let obs = SourceObservation {
            source: "tvmaze".into(),
            source_id: "44933".into(),
            raw_payload_id: raw.id.clone(),
            kind: ReleaseKind::Tv,
            display_title: "Severance".into(),
            raw_title: None,
            description: Some("workplace sci-fi".into()),
            status: Some("Running".into()),
            observed_at: 1_000,
            source_updated_at: None,
            aliases: vec![AliasObservation {
                alias: "Severance".into(),
                locale: Some("English".into()),
                alias_kind: Some("primary".into()),
                confidence: 1.0,
            }],
            external_ids: vec![ExternalIdObservation {
                id_kind: "imdb".into(),
                id_value: "tt11280740".into(),
                confidence: 1.0,
            }],
            release_events: vec![],
            links: vec![],
            images: vec![],
        };
        lib.store_source_observation(&obs).unwrap();

        let hits = lib.search_source_candidates("severance", 10).unwrap();
        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0].source, "tvmaze");
        assert_eq!(hits[0].source_id, "44933");
        assert_eq!(hits[0].kind, ReleaseKind::Tv);
    }

    // ------------------------------------------------------------------
    // load_resolved — the single-query read path
    // ------------------------------------------------------------------

    use crate::store::{CacheEntry, EngagementMeta, TtlConfig};

    fn cache_for(source_id: &str, status: &str, fetched_at: i64) -> CacheEntry {
        let ttl = TtlConfig::DEFAULT;
        let status_enum = crate::store::metadata_cache::CacheStatus::parse(Some(status));
        CacheEntry {
            source: "tmdb".into(),
            source_id: source_id.into(),
            display_title: Some(format!("Title {source_id}")),
            title_english: Some(format!("English {source_id}")),
            title_native: None,
            status: Some(status.into()),
            total_episodes: Some(12),
            format: Some("TV".into()),
            next_episode_number: Some(3),
            next_episode_airs_at: Some(fetched_at + 3600),
            fetched_at,
            expires_at: ttl.expires_at(status_enum, fetched_at),
            cover_image_url: None,
            description: Some("desc".into()),
            score: Some(82.0),
            studios: Some("Studio X".into()),
            streaming_links_json: Some(r#"[{"site":"Netflix","url":"https://nfx/x"}]"#.into()),
        }
    }

    #[test]
    fn load_resolved_empty_library_returns_empty_vec() {
        let lib = lib_at(1_000);
        let resolved = lib.load_resolved().unwrap();
        assert!(resolved.is_empty());
    }

    #[test]
    fn load_resolved_returns_one_row_per_active_follow_with_cache_and_no_engagements() {
        let lib = lib_at(1_000);
        let cid = id("severance");
        lib.follow_with_source(
            &cid,
            ReleaseKind::Tv,
            "Severance",
            "tmdb",
            "95396",
            None,
            1.0,
        )
        .unwrap();
        lib.upsert_cache(&cache_for("95396", "RELEASING", 1_000))
            .unwrap();

        let resolved = lib.load_resolved().unwrap();
        assert_eq!(resolved.len(), 1);
        let r = &resolved[0];
        assert_eq!(r.canonical.id, cid);
        assert_eq!(r.primary_source.source, "tmdb");
        assert_eq!(r.primary_source.source_id, "95396");
        let cache = r.cache.as_ref().expect("cache row present");
        assert_eq!(cache.status(), Some("RELEASING"));
        assert_eq!(cache.total_episodes(), Some(12));
        assert!(r.last_completed.is_none());
        assert!(r.last_verified.is_none());
    }

    #[test]
    fn load_resolved_omits_cache_when_uncached() {
        let lib = lib_at(1_000);
        let cid = id("uncached");
        lib.follow_with_source(&cid, ReleaseKind::Tv, "U", "tmdb", "0", None, 1.0)
            .unwrap();
        let resolved = lib.load_resolved().unwrap();
        assert_eq!(resolved.len(), 1);
        assert!(resolved[0].cache.is_none());
    }

    #[test]
    fn load_resolved_picks_most_recent_completed_and_verified() {
        let clock = AdvanceableClock::new(1_000);
        let lib = Library::open_in_memory(Arc::new(clock.clone())).unwrap();
        let cid = id("with-events");
        lib.follow_with_source(&cid, ReleaseKind::Tv, "X", "tmdb", "1", None, 1.0)
            .unwrap();
        clock.set(2_000);
        lib.engage(
            &cid,
            EngagementEvent::Completed,
            Some(EngagementMeta::Completed { seen: 1 }),
        )
        .unwrap();
        clock.set(3_000);
        lib.engage(
            &cid,
            EngagementEvent::Completed,
            Some(EngagementMeta::Completed { seen: 2 }),
        )
        .unwrap();
        clock.set(4_000);
        lib.engage(
            &cid,
            EngagementEvent::Verified,
            Some(EngagementMeta::Verified {
                streamer: "Netflix".into(),
                url: "https://netflix.com/x".into(),
            }),
        )
        .unwrap();

        let resolved = lib.load_resolved().unwrap();
        let r = &resolved[0];
        let lc = r.last_completed.as_ref().expect("last_completed present");
        assert_eq!(lc.occurred_at, 3_000);
        assert_eq!(lc.seen(), Some(2));
        let lv = r.last_verified.as_ref().expect("last_verified present");
        assert_eq!(lv.occurred_at, 4_000);
        assert_eq!(lv.streamer(), Some("Netflix"));
        assert_eq!(lv.verified_url(), Some("https://netflix.com/x"));
    }

    #[test]
    fn load_resolved_orders_active_by_followed_at_desc_and_skips_dropped() {
        let clock = AdvanceableClock::new(1_000);
        let lib = Library::open_in_memory(Arc::new(clock.clone())).unwrap();
        for (slug, t) in [("oldest", 1_000), ("middle", 2_000), ("newest", 3_000)] {
            clock.set(t);
            lib.follow_with_source(&id(slug), ReleaseKind::Tv, slug, "tmdb", slug, None, 1.0)
                .unwrap();
        }
        // Drop the middle one — must be excluded from load_resolved.
        lib.drop_canonical(&id("middle")).unwrap();

        let resolved = lib.load_resolved().unwrap();
        let order: Vec<&str> = resolved
            .iter()
            .map(|r| r.canonical.display_title.as_str())
            .collect();
        assert_eq!(order, ["newest", "oldest"]);
    }

    #[test]
    fn load_resolved_picks_highest_confidence_source_ref_as_primary() {
        let lib = lib_at(1_000);
        let cid = id("multi-source");
        // First (and lower confidence) source_ref.
        lib.follow_with_source(&cid, ReleaseKind::Tv, "X", "tmdb", "1", None, 0.7)
            .unwrap();
        // Second source_ref on the same canonical, higher confidence.
        lib.follow_with_source(&cid, ReleaseKind::Tv, "X", "anilist", "21", None, 0.95)
            .unwrap();

        let resolved = lib.load_resolved().unwrap();
        assert_eq!(resolved.len(), 1);
        // The window picks the highest-confidence ref.
        assert_eq!(resolved[0].primary_source.source, "anilist");
        assert!((resolved[0].primary_source.confidence - 0.95).abs() < f64::EPSILON);
    }
}
