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

use anyhow::{anyhow, Context, Result};

use crate::ids::{CanonicalId, ReleaseKind};
use crate::ingest::{RawSourcePayload, SourceObservation};
use crate::search::source_candidate::SourceCandidateResult;
pub(crate) use crate::store::ResolvedRelease;
use crate::store::{
    CacheEntry, CanonicalFollowOutcome, CanonicalRelease, CanonicalScheduleEvent, Db, Engagement,
    EngagementEvent, EngagementMeta, SourceParseError, SourceRef, SourceRefRefreshState,
    SourceSearchCacheEntry,
};
use crate::time::Clock;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct SourceIngestSuccess {
    pub projected_events: usize,
}

/// The domain facade. Hold one of these per process; share via
/// `Arc<Library>` to async tasks and the TUI.
pub(crate) struct Library {
    db: Mutex<Db>,
    clock: Arc<dyn Clock>,
}

impl Library {
    /// Open a Library backed by the on-disk DB at `path`. Creates the
    /// file (and parent dirs) and runs migrations on first open.
    pub(crate) fn open(path: &Path, clock: Arc<dyn Clock>) -> Result<Self> {
        let db = Db::open(path).context("open Library DB")?;
        Ok(Self {
            db: Mutex::new(db),
            clock,
        })
    }

    /// In-memory Library for tests. Migrations always run.
    #[cfg(test)]
    pub(crate) fn open_in_memory(clock: Arc<dyn Clock>) -> Result<Self> {
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
    pub(crate) fn follow_with_source(
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
    pub(crate) fn drop_canonical(&self, canonical_id: &CanonicalId) -> Result<bool> {
        let db = self.lock_db()?;
        db.drop_canonical(canonical_id, self.now())
    }

    pub(crate) fn canonical_id_for_source_candidate(
        candidate: &SourceCandidateResult,
    ) -> CanonicalId {
        CanonicalId::legacy_from_source(candidate.kind, &candidate.source, &candidate.source_id)
    }

    pub(crate) fn canonical_id_for_source_ref(
        &self,
        source: &str,
        source_id: &str,
    ) -> Result<Option<CanonicalId>> {
        let db = self.lock_db()?;
        Ok(db
            .find_source_ref(source, source_id)?
            .map(|source_ref| source_ref.canonical_id))
    }

    /// Follow an indexed source candidate. This is intentionally source-agnostic:
    /// callers provide the selected candidate, and Library owns the canonical
    /// row + source_ref + followed_at mutation.
    pub(crate) fn follow_source_candidate(
        &self,
        candidate: &SourceCandidateResult,
    ) -> Result<CanonicalFollowOutcome> {
        let canonical_id = Self::canonical_id_for_source_candidate(candidate);
        self.follow_with_source(
            &canonical_id,
            candidate.kind,
            &candidate.display_title,
            &candidate.source,
            &candidate.source_id,
            Some(&candidate.display_title),
            1.0,
        )
    }

    // ------------------------------------------------------------------
    // Reads
    // ------------------------------------------------------------------

    /// Active follows, newest-first by followed_at.
    pub(crate) fn followed(&self) -> Result<Vec<CanonicalRelease>> {
        let db = self.lock_db()?;
        db.list_active_canonical()
    }

    /// Active follows with their primary source_ref, cache row, and
    /// last `Completed` + `Verified` engagement events — all in one
    /// prepared, cached query. Replaces the N+1 the TUI's `Shelf::load`
    /// used to do.
    pub(crate) fn load_resolved(&self) -> Result<Vec<ResolvedRelease>> {
        let now = self.now();
        let db = self.lock_db()?;
        db.load_resolved(now)
    }

    #[cfg(test)]
    pub(crate) fn find_canonical(&self, id: &CanonicalId) -> Result<Option<CanonicalRelease>> {
        let db = self.lock_db()?;
        db.find_canonical(id)
    }

    pub(crate) fn count_followed(&self) -> Result<i64> {
        let db = self.lock_db()?;
        db.count_followed_canonical()
    }

    // ------------------------------------------------------------------
    // Engagement
    // ------------------------------------------------------------------

    /// Append an engagement event. The payload (if any) is typed by
    /// [`EngagementMeta`]; the store encodes it to JSON for the column.
    pub(crate) fn engage(
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
    pub(crate) fn last_engagement(
        &self,
        canonical_id: &CanonicalId,
        event: EngagementEvent,
    ) -> Result<Option<Engagement>> {
        let db = self.lock_db()?;
        db.last_engagement(canonical_id, event)
    }

    /// Engagement events for one canonical, newest-first.
    pub(crate) fn engagement_for(&self, canonical_id: &CanonicalId) -> Result<Vec<Engagement>> {
        let db = self.lock_db()?;
        db.engagement_for_canonical(canonical_id)
    }

    // ------------------------------------------------------------------
    // Source refs and metadata cache (used by the sync engine).
    // ------------------------------------------------------------------

    pub(crate) fn source_refs_for(&self, canonical_id: &CanonicalId) -> Result<Vec<SourceRef>> {
        let db = self.lock_db()?;
        db.source_refs_for_canonical(canonical_id)
    }

    pub(crate) fn upsert_cache(&self, entry: &CacheEntry) -> Result<()> {
        let db = self.lock_db()?;
        db.upsert_cache(entry)
    }

    // ------------------------------------------------------------------
    // Source ingestion: raw payloads -> observations -> candidates.
    // ------------------------------------------------------------------

    pub(crate) fn store_raw_source_payload(&self, payload: &RawSourcePayload) -> Result<()> {
        let db = self.lock_db()?;
        db.upsert_raw_source_payload(payload)
    }

    pub(crate) fn store_source_observation(&self, observation: &SourceObservation) -> Result<()> {
        let mut db = self.lock_db()?;
        db.upsert_source_observation(observation)
    }

    pub(crate) fn record_source_ingest_success(
        &self,
        canonical_id: &CanonicalId,
        raw_payload: &RawSourcePayload,
        observation: &SourceObservation,
        next_due_at: i64,
    ) -> Result<SourceIngestSuccess> {
        if raw_payload.source != observation.source {
            return Err(anyhow!(
                "raw payload source {:?} does not match observation source {:?}",
                raw_payload.source,
                observation.source
            ));
        }
        if raw_payload.id != observation.raw_payload_id {
            return Err(anyhow!(
                "raw payload id {:?} does not match observation raw_payload_id {:?}",
                raw_payload.id,
                observation.raw_payload_id
            ));
        }

        let now = self.now();
        let projected_events = observation.release_events.len();
        let mut db = self.lock_db()?;
        db.upsert_raw_source_payload(raw_payload)?;
        db.upsert_source_observation(observation)?;
        db.upsert_canonical_schedule_events(
            canonical_id,
            &observation.source,
            &observation.release_events,
        )?;
        db.upsert_source_ref_refresh_state(&SourceRefRefreshState {
            source: observation.source.clone(),
            source_id: observation.source_id.clone(),
            last_attempt_at: Some(now),
            last_success_at: Some(now),
            last_error: None,
            next_due_at: Some(next_due_at),
            failure_count: 0,
        })?;
        Ok(SourceIngestSuccess { projected_events })
    }

    pub(crate) fn record_source_ingest_failure(
        &self,
        source: &str,
        source_id: &str,
        error: &str,
        next_due_at: i64,
    ) -> Result<()> {
        let now = self.now();
        let db = self.lock_db()?;
        let existing = db.get_source_ref_refresh_state(source, source_id)?;
        let failure_count = existing
            .as_ref()
            .map(|state| state.failure_count + 1)
            .unwrap_or(1);
        db.upsert_source_ref_refresh_state(&SourceRefRefreshState {
            source: source.to_string(),
            source_id: source_id.to_string(),
            last_attempt_at: Some(now),
            last_success_at: existing.as_ref().and_then(|state| state.last_success_at),
            last_error: Some(error.to_string()),
            next_due_at: Some(next_due_at),
            failure_count,
        })
    }

    #[allow(dead_code)]
    pub(crate) fn record_source_parse_error(&self, err: &SourceParseError) -> Result<()> {
        let db = self.lock_db()?;
        db.insert_source_parse_error(err)
    }

    pub(crate) fn search_source_candidates(
        &self,
        query: &str,
        limit: u32,
    ) -> Result<Vec<SourceCandidateResult>> {
        let db = self.lock_db()?;
        db.search_source_candidates(query, limit)
    }

    pub(crate) fn upsert_source_search_cache(&self, entry: &SourceSearchCacheEntry) -> Result<()> {
        let db = self.lock_db()?;
        db.upsert_source_search_cache(entry)
    }

    pub(crate) fn get_source_search_cache(
        &self,
        source: &str,
        query_key: &str,
    ) -> Result<Option<SourceSearchCacheEntry>> {
        let db = self.lock_db()?;
        db.get_source_search_cache(source, query_key)
    }

    #[allow(dead_code)]
    pub(crate) fn upsert_source_ref_refresh_state(
        &self,
        state: &SourceRefRefreshState,
    ) -> Result<()> {
        let db = self.lock_db()?;
        db.upsert_source_ref_refresh_state(state)
    }

    pub(crate) fn get_source_ref_refresh_state(
        &self,
        source: &str,
        source_id: &str,
    ) -> Result<Option<SourceRefRefreshState>> {
        let db = self.lock_db()?;
        db.get_source_ref_refresh_state(source, source_id)
    }

    pub(crate) fn due_source_ref_refresh_states(
        &self,
        limit: u32,
    ) -> Result<Vec<SourceRefRefreshState>> {
        let db = self.lock_db()?;
        db.due_source_ref_refresh_states(self.now(), limit)
    }

    #[allow(dead_code)]
    pub(crate) fn project_canonical_schedule_events(
        &self,
        canonical_id: &CanonicalId,
        source: &str,
        observation: &SourceObservation,
    ) -> Result<()> {
        let db = self.lock_db()?;
        db.upsert_canonical_schedule_events(canonical_id, source, &observation.release_events)
    }

    #[allow(dead_code)]
    pub(crate) fn schedule_events_for_canonical(
        &self,
        canonical_id: &CanonicalId,
    ) -> Result<Vec<CanonicalScheduleEvent>> {
        let db = self.lock_db()?;
        db.schedule_events_for_canonical(canonical_id)
    }

    // ------------------------------------------------------------------
    // Small kv store (used by the notifier for dedupe markers,
    // by sync for last-tick state).
    // ------------------------------------------------------------------

    pub(crate) fn kv_get(&self, key: &str) -> Result<Option<String>> {
        let db = self.lock_db()?;
        Ok(db.kv_get(key)?.map(|(value, _)| value))
    }

    pub(crate) fn kv_set(&self, key: &str, value: &str) -> Result<()> {
        let db = self.lock_db()?;
        db.kv_set(key, value, self.now())
    }

    /// Remove a key. No-op if missing.
    pub(crate) fn kv_delete(&self, key: &str) -> Result<()> {
        let db = self.lock_db()?;
        db.kv_delete(key)
    }

    /// Persisted streamer subscriptions (canonical-lowercase strings).
    /// Backed by kv key `subs.streaming` (JSON array). Empty when unset.
    pub(crate) fn subscribed_streamers(&self) -> Result<Vec<String>> {
        let Some(raw) = self.kv_get("subs.streaming")? else {
            return Ok(Vec::new());
        };
        serde_json::from_str(&raw).with_context(|| format!("decode subs.streaming: {raw:?}"))
    }

    /// Overwrite the streamer subscription list. Empty input clears the key.
    pub(crate) fn set_subscribed_streamers(&self, streamers: &[String]) -> Result<()> {
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ingest::{
        HttpMethod, RawSourcePayload, ReleaseEventObservation, SourceObservation, TimePrecision,
    };
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
    fn source_parsers_materialize_searchable_candidates() {
        use crate::ingest::{HttpMethod, RawSourcePayload, SourceParser};
        use crate::sources::{
            anilist::AniListParser, itunes::ItunesParser, jikan::JikanParser, kitsu::KitsuParser,
            musicbrainz::MusicBrainzParser,
        };

        struct Case {
            source: &'static str,
            endpoint: &'static str,
            body: &'static str,
            parser: Box<dyn SourceParser>,
            search_query: &'static str,
            expected_source_id: &'static str,
        }

        let cases = vec![
            Case {
                source: "anilist",
                endpoint: "search_media",
                parser: Box::new(AniListParser),
                search_query: "one piece",
                expected_source_id: "21",
                body: r#"{
                    "data": {"Page": {"media": [{
                        "id": 21,
                        "title": {"romaji": "ONE PIECE", "english": "One Piece", "native": "ワンピース"},
                        "status": "RELEASING",
                        "episodes": null,
                        "format": "TV",
                        "nextAiringEpisode": {"episode": 1100, "airingAt": 1700000000}
                    }]}}
                }"#,
            },
            Case {
                source: "musicbrainz",
                endpoint: "artist_search",
                parser: Box::new(MusicBrainzParser),
                search_query: "radiohead",
                expected_source_id: "a74b1b7f-71a5-4011-9441-d0b5e4122711",
                body: r#"{
                    "artists": [{
                        "id": "a74b1b7f-71a5-4011-9441-d0b5e4122711",
                        "name": "Radiohead",
                        "sort-name": "Radiohead",
                        "type": "Group",
                        "country": "GB",
                        "aliases": [{"name": "On A Friday"}],
                        "tags": [{"name": "alternative rock"}]
                    }]
                }"#,
            },
            Case {
                source: "itunes",
                endpoint: "search",
                parser: Box::new(ItunesParser),
                search_query: "dune",
                expected_source_id: "track:123",
                body: r#"{
                    "resultCount": 1,
                    "results": [{
                        "wrapperType": "track",
                        "kind": "feature-movie",
                        "trackId": 123,
                        "trackName": "Dune: Part Two",
                        "artistName": "Denis Villeneuve",
                        "releaseDate": "2024-03-01T08:00:00Z"
                    }]
                }"#,
            },
            Case {
                source: "kitsu",
                endpoint: "anime_search",
                parser: Box::new(KitsuParser),
                search_query: "bebop",
                expected_source_id: "1",
                body: r#"{
                    "data": [{
                        "id": "1",
                        "type": "anime",
                        "attributes": {
                            "canonicalTitle": "Cowboy Bebop",
                            "titles": {"ja_jp": "カウボーイビバップ"},
                            "status": "finished",
                            "startDate": "1998-04-03"
                        }
                    }]
                }"#,
            },
            Case {
                source: "jikan",
                endpoint: "anime_search",
                parser: Box::new(JikanParser),
                search_query: "fullmetal",
                expected_source_id: "5114",
                body: r#"{
                    "data": [{
                        "mal_id": 5114,
                        "title": "Fullmetal Alchemist: Brotherhood",
                        "title_japanese": "鋼の錬金術師 FULLMETAL ALCHEMIST",
                        "status": "Finished Airing"
                    }]
                }"#,
            },
        ];

        for (idx, case) in cases.into_iter().enumerate() {
            let lib = lib_at(1_000 + idx as i64);
            let raw = RawSourcePayload {
                id: format!("raw:{}:{idx}", case.source),
                source: case.source.into(),
                endpoint: case.endpoint.into(),
                method: HttpMethod::Get,
                request_key: format!("{}:{}:{idx}", case.source, case.endpoint),
                request_hash: format!("req-{idx}"),
                request_json: None,
                http_status: 200,
                response_hash: format!("resp-{idx}"),
                response_json: case.body.into(),
                fetched_at: 1_000 + idx as i64,
                expires_at: None,
                created_at: 1_000 + idx as i64,
            };

            lib.store_raw_source_payload(&raw).unwrap();
            let observations = case.parser.parse_search(&raw).unwrap();
            assert!(
                !observations.is_empty(),
                "{} should parse observations",
                case.source
            );
            for observation in observations {
                lib.store_source_observation(&observation).unwrap();
            }

            let hits = lib.search_source_candidates(case.search_query, 10).unwrap();
            assert!(
                hits.iter().any(|hit| {
                    hit.source == case.source && hit.source_id == case.expected_source_id
                }),
                "{} candidate should be searchable for {:?}; hits: {:?}",
                case.source,
                case.search_query,
                hits
            );
        }
    }

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

    use crate::store::metadata_cache::TtlConfig;
    use crate::store::{CacheEntry, EngagementMeta};

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

    #[test]
    fn load_resolved_returns_projected_canonical_schedule_event() {
        let clock = AdvanceableClock::new(1_000);
        let lib = Library::open_in_memory(Arc::new(clock.clone())).unwrap();
        let cid = id("scheduled");
        lib.follow_with_source(
            &cid,
            ReleaseKind::Tv,
            "Scheduled",
            "tvmaze",
            "42",
            None,
            1.0,
        )
        .unwrap();

        let raw = RawSourcePayload {
            id: "raw:tvmaze:42".into(),
            source: "tvmaze".into(),
            endpoint: "show".into(),
            method: HttpMethod::Get,
            request_key: "tvmaze:show:42".into(),
            request_hash: "req".into(),
            request_json: None,
            http_status: 200,
            response_hash: "resp".into(),
            response_json: "{}".into(),
            fetched_at: 1_000,
            expires_at: None,
            created_at: 1_000,
        };
        lib.store_raw_source_payload(&raw).unwrap();

        let observation = SourceObservation {
            source: "tvmaze".into(),
            source_id: "42".into(),
            raw_payload_id: raw.id.clone(),
            kind: ReleaseKind::Tv,
            display_title: "Scheduled".into(),
            raw_title: None,
            description: None,
            status: Some("Running".into()),
            observed_at: 1_000,
            source_updated_at: None,
            aliases: vec![],
            external_ids: vec![],
            release_events: vec![
                ReleaseEventObservation {
                    id: "tvmaze:42:past".into(),
                    event_kind: "episode".into(),
                    title: Some("Past".into()),
                    season: Some(1),
                    episode: Some(1),
                    local_date: None,
                    local_time: None,
                    source_timezone: Some("UTC".into()),
                    scheduled_at: Some(900),
                    precision: TimePrecision::Instant,
                    confidence: 0.9,
                    observed_at: 1_000,
                },
                ReleaseEventObservation {
                    id: "tvmaze:42:soon".into(),
                    event_kind: "episode".into(),
                    title: Some("Soon".into()),
                    season: Some(1),
                    episode: Some(2),
                    local_date: None,
                    local_time: None,
                    source_timezone: Some("UTC".into()),
                    scheduled_at: Some(1_500),
                    precision: TimePrecision::Instant,
                    confidence: 0.9,
                    observed_at: 1_000,
                },
                ReleaseEventObservation {
                    id: "tvmaze:42:later".into(),
                    event_kind: "episode".into(),
                    title: Some("Later".into()),
                    season: Some(1),
                    episode: Some(3),
                    local_date: None,
                    local_time: None,
                    source_timezone: Some("UTC".into()),
                    scheduled_at: Some(3_000),
                    precision: TimePrecision::Instant,
                    confidence: 0.9,
                    observed_at: 1_000,
                },
            ],
            links: vec![],
            images: vec![],
        };
        lib.store_source_observation(&observation).unwrap();
        lib.project_canonical_schedule_events(&cid, "tvmaze", &observation)
            .unwrap();

        let resolved = lib.load_resolved().unwrap();
        assert_eq!(resolved.len(), 1);
        let ev = resolved[0]
            .next_schedule_event
            .as_ref()
            .expect("next schedule event");
        assert_eq!(ev.source, "tvmaze");
        assert_eq!(ev.event_kind, "episode");
        assert_eq!(ev.title.as_deref(), Some("Soon"));
        assert_eq!(ev.season, Some(1));
        assert_eq!(ev.episode, Some(2));
        assert_eq!(ev.scheduled_at, Some(1_500));

        clock.set(4_000);
        let resolved = lib.load_resolved().unwrap();
        let ev = resolved[0].next_schedule_event.as_ref().unwrap();
        assert_eq!(ev.title.as_deref(), Some("Later"));
        assert_eq!(ev.episode, Some(3));
        assert_eq!(ev.scheduled_at, Some(3_000));
    }
}
