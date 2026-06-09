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
    CacheEntry, CanonicalFollowOutcome, CanonicalRelease, Db, Engagement, EngagementEvent,
    EngagementMeta, SourceRef,
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

    pub fn get_cache(&self, source: &str, source_id: &str) -> Result<Option<CacheEntry>> {
        let db = self.lock_db()?;
        db.get_cache(source, source_id)
    }

    pub fn upsert_cache(&self, entry: &CacheEntry) -> Result<()> {
        let db = self.lock_db()?;
        db.upsert_cache(entry)
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
        serde_json::from_str(&raw)
            .with_context(|| format!("decode subs.streaming: {raw:?}"))
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
        assert!(refs.iter().any(|r| r.source == "tmdb" && r.source_id == "95396"));
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
        let refs = lib.source_refs_for(&cid).unwrap();
        assert!(refs.iter().any(|r| r.source == "tmdb" && r.source_id == "95396"));
        assert!(refs.iter().any(|r| r.source == "tvmaze" && r.source_id == "44060"));
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
        lib.set_subscribed_streamers(&["Netflix".to_string()]).unwrap();
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
}
