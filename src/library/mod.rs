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
use crate::store::{
    CacheEntry, CanonicalFollowOutcome, CanonicalListFilter, CanonicalRelease, Db, Engagement,
    EngagementEvent, SourceRef,
};
use crate::time::Clock;

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
        db.list_canonical(CanonicalListFilter::Active)
    }

    /// Soft-dropped canonicals, newest-first by dropped_at.
    pub fn dropped(&self) -> Result<Vec<CanonicalRelease>> {
        let db = self.lock_db()?;
        db.list_canonical(CanonicalListFilter::Dropped)
    }

    /// Every canonical including created-not-followed, newest-first
    /// by created_at.
    pub fn all_canonical(&self) -> Result<Vec<CanonicalRelease>> {
        let db = self.lock_db()?;
        db.list_canonical(CanonicalListFilter::All)
    }

    pub fn find_canonical(&self, id: &CanonicalId) -> Result<Option<CanonicalRelease>> {
        let db = self.lock_db()?;
        db.find_canonical(id)
    }

    pub fn find_canonical_by_source(
        &self,
        source: &str,
        source_id: &str,
    ) -> Result<Option<CanonicalId>> {
        let db = self.lock_db()?;
        db.find_canonical_by_source(source, source_id)
    }

    pub fn count_followed(&self) -> Result<i64> {
        let db = self.lock_db()?;
        db.count_followed_canonical()
    }

    // ------------------------------------------------------------------
    // Engagement
    // ------------------------------------------------------------------

    /// Append an engagement event. `meta` must be JSON if provided.
    pub fn engage(
        &self,
        canonical_id: &CanonicalId,
        event: EngagementEvent,
        meta: Option<&str>,
    ) -> Result<()> {
        let db = self.lock_db()?;
        db.append_engagement(canonical_id, event, self.now(), meta)?;
        Ok(())
    }

    /// Last engagement of a given event kind for one canonical.
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

    /// Global engagement, newest-first. `since` is unix seconds.
    /// `limit` = 0 means no cap.
    pub fn recent_engagement(&self, since: i64, limit: u32) -> Result<Vec<Engagement>> {
        let db = self.lock_db()?;
        db.recent_engagement(since, limit)
    }

    // ------------------------------------------------------------------
    // Source refs and metadata cache (used by the sync engine).
    // ------------------------------------------------------------------

    pub fn source_refs_for(&self, canonical_id: &CanonicalId) -> Result<Vec<SourceRef>> {
        let db = self.lock_db()?;
        db.source_refs_for_canonical(canonical_id)
    }

    pub fn get_cache(&self, source: &str, source_id: &str) -> Result<Option<CacheEntry>> {
        let db = self.lock_db()?;
        db.get_cache(source, source_id)
    }

    pub fn upsert_cache(&self, entry: &CacheEntry) -> Result<()> {
        let db = self.lock_db()?;
        db.upsert_cache(entry)
    }

    /// Persist the rendered ASCII cover for a canonical. Wraps the
    /// existing store-level setter so the TUI's follow path doesn't
    /// have to reach into `store::canonical_release` directly.
    pub fn set_canonical_cover(
        &self,
        id: &CanonicalId,
        ascii: &str,
        color: Option<&str>,
    ) -> Result<()> {
        let db = self.lock_db()?;
        db.set_canonical_cover(id, ascii, color)
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

    // ------------------------------------------------------------------
    // Canonicalization cache (used by the canonical service)
    // ------------------------------------------------------------------

    pub fn cached_canonical_for(
        &self,
        source: &str,
        source_id: &str,
    ) -> Result<Option<CanonicalId>> {
        let db = self.lock_db()?;
        db.cached_canonical_for(source, source_id)
    }

    /// Record a canonicalization decision. `decided_by` is a free-form
    /// provenance tag — "llm:claude-opus-4-7", "alias-match", "manual".
    pub fn cache_canonicalization(
        &self,
        source: &str,
        source_id: &str,
        canonical_id: &CanonicalId,
        decided_by: &str,
    ) -> Result<()> {
        let db = self.lock_db()?;
        db.cache_canonicalization(source, source_id, canonical_id, decided_by, self.now())
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

        let mapped = lib.find_canonical_by_source("tmdb", "95396").unwrap().unwrap();
        assert_eq!(mapped, cid);
    }

    #[test]
    fn second_source_ref_for_same_canonical_extends_mapping() {
        let lib = lib_at(1_000);
        let cid = id("severance");
        lib.follow_with_source(&cid, ReleaseKind::Tv, "Severance", "tmdb", "95396", None, 0.95)
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
        assert_eq!(
            lib.find_canonical_by_source("tmdb", "95396").unwrap().unwrap(),
            cid
        );
        assert_eq!(
            lib.find_canonical_by_source("tvmaze", "44060").unwrap().unwrap(),
            cid
        );
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
        assert_eq!(lib.dropped().unwrap().len(), 1);
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
            lib.follow_with_source(
                &id(slug),
                ReleaseKind::Tv,
                slug,
                "tmdb",
                slug,
                None,
                1.0,
            )
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
        lib.engage(&cid, EngagementEvent::Completed, Some(r#"{"seen":1}"#))
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
    fn recent_engagement_respects_since_and_limit() {
        let clock = AdvanceableClock::new(1_000);
        let lib = Library::open_in_memory(Arc::new(clock.clone())).unwrap();
        let cid = id("x");
        lib.follow_with_source(&cid, ReleaseKind::Tv, "X", "tmdb", "1", None, 1.0).unwrap();
        for t in [1_500, 2_000, 2_500, 3_000] {
            clock.set(t);
            lib.engage(&cid, EngagementEvent::Opened, None).unwrap();
        }
        let recent = lib.recent_engagement(2_000, 0).unwrap();
        assert_eq!(recent.len(), 3); // 2_000, 2_500, 3_000
        assert_eq!(recent[0].occurred_at, 3_000);
        let limited = lib.recent_engagement(0, 2).unwrap();
        assert_eq!(limited.len(), 2);
    }

    #[test]
    fn canonicalization_cache_round_trip_through_library() {
        let lib = lib_at(1_000);
        let cid = id("severance");
        lib.follow_with_source(&cid, ReleaseKind::Tv, "Severance", "tmdb", "1", None, 1.0)
            .unwrap();
        assert!(lib.cached_canonical_for("tmdb", "1").unwrap().is_none());
        lib.cache_canonicalization("tmdb", "1", &cid, "llm:claude-opus-4-7")
            .unwrap();
        assert_eq!(
            lib.cached_canonical_for("tmdb", "1").unwrap().unwrap(),
            cid
        );
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
            lib.follow_with_source(
                &id(slug),
                ReleaseKind::Tv,
                slug,
                "tmdb",
                slug,
                None,
                1.0,
            )
            .unwrap();
        }
        assert_eq!(lib.count_followed().unwrap(), 3);
        lib.drop_canonical(&id("b")).unwrap();
        assert_eq!(lib.count_followed().unwrap(), 2);
    }
}
