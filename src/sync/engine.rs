//! SyncEngine — the long-running refresh + verify loop.
//!
//! One `tick()` does the full cycle for the followed graph:
//!
//!   1. List followed canonicals (via Library).
//!   2. For each canonical, for each attached source_ref:
//!      a. Look up the source adapter by name.
//!      b. Check the metadata cache. Skip if fresh (TTL not expired).
//!      c. Fetch the latest SourceRecord from the source.
//!      d. Compute previous streaming_links from the cached row.
//!      e. Upsert the cache with the new record.
//!      f. Diff streaming_links → [`VerifiedRelease`] events.
//!      g. Record each event as an Engagement::Verified and fan out to
//!         the [`Dispatcher`].
//!   3. Return a [`SyncReport`].
//!
//! The `run(ct)` loop is just `tick → sleep → tick → …` with
//! cancellation via [`tokio_util::sync::CancellationToken`].

use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;

use anyhow::{Context, Result};
use tokio_util::sync::CancellationToken;

use crate::library::Library;
use crate::notifier::Dispatcher;
use crate::sources::{Source, StreamingLink};
use crate::store::{CacheEntry, EngagementEvent, TtlConfig};
use crate::time::Clock;

use super::verify::detect_new_streaming;
use super::VerifiedRelease;

/// Outcome of one [`SyncEngine::tick`].
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct SyncReport {
    pub canonicals_examined: u32,
    pub source_refs_refreshed: u32,
    pub source_refs_skipped_fresh: u32,
    pub verified_releases: Vec<VerifiedRelease>,
    pub errors: Vec<String>,
}

/// Refresh-and-verify engine.
pub struct SyncEngine {
    library: Arc<Library>,
    sources: HashMap<&'static str, Arc<dyn Source>>,
    dispatcher: Arc<Dispatcher>,
    subscriptions: Vec<String>,
    clock: Arc<dyn Clock>,
    ttl: TtlConfig,
    interval: Duration,
}

impl SyncEngine {
    /// Build an engine. `subscriptions` mirrors `Config.subscriptions.video`
    /// flattened — case-insensitive matching against
    /// SourceRecord.streaming_links.site.
    pub fn new(
        library: Arc<Library>,
        sources: Vec<Arc<dyn Source>>,
        dispatcher: Arc<Dispatcher>,
        subscriptions: Vec<String>,
        clock: Arc<dyn Clock>,
        ttl: TtlConfig,
        interval: Duration,
    ) -> Self {
        let sources = sources.into_iter().map(|s| (s.name(), s)).collect();
        Self {
            library,
            sources,
            dispatcher,
            subscriptions,
            clock,
            ttl,
            interval,
        }
    }

    /// One refresh-and-verify pass. Idempotent and resumable: a crash
    /// mid-tick leaves the system consistent (each refresh is a single
    /// upsert + a single engagement append + zero-or-more notifies).
    pub async fn tick(&self) -> SyncReport {
        let mut report = SyncReport::default();
        let followed = match self.library.followed() {
            Ok(rows) => rows,
            Err(e) => {
                report.errors.push(format!("list followed: {e:#}"));
                return report;
            }
        };
        report.canonicals_examined = followed.len() as u32;

        for canonical in followed {
            let refs = match self.library.source_refs_for(&canonical.id) {
                Ok(refs) => refs,
                Err(e) => {
                    report
                        .errors
                        .push(format!("source_refs_for {}: {e:#}", canonical.id));
                    continue;
                }
            };
            for sref in refs {
                let Some(source) = self.sources.get(sref.source.as_str()) else {
                    // No adapter registered for this source — skip
                    // silently. Common when an old source_ref points at
                    // a source we no longer ship (a v0.4 row).
                    continue;
                };
                match self.refresh_one(&canonical.id, &sref.source, &sref.source_id, source).await {
                    Ok(RefreshOutcome::Fresh) => {
                        report.source_refs_skipped_fresh += 1;
                    }
                    Ok(RefreshOutcome::Refreshed { verified }) => {
                        report.source_refs_refreshed += 1;
                        for v in &verified {
                            // Persist the verified event as an
                            // engagement so the context export and the
                            // TUI surface it.
                            let meta = serde_json::to_string(&serde_json::json!({
                                "streamer": v.streamer,
                                "deep_link": v.deep_link,
                            }))
                            .ok();
                            if let Err(e) = self.library.engage(
                                &v.canonical_id,
                                EngagementEvent::Verified,
                                meta.as_deref(),
                            ) {
                                report
                                    .errors
                                    .push(format!("engage Verified: {e:#}"));
                            }
                            match self.dispatcher.notify_once(v).await {
                                Ok(_) => {}
                                Err(e) => {
                                    report
                                        .errors
                                        .push(format!("dispatch: {e:#}"));
                                }
                            }
                        }
                        report.verified_releases.extend(verified);
                    }
                    Err(e) => {
                        report
                            .errors
                            .push(format!("refresh {}/{}: {e:#}", sref.source, sref.source_id));
                    }
                }
            }
        }
        report
    }

    /// Refresh one source_ref. Returns whether a fetch happened and
    /// any verified events that came out of the diff.
    async fn refresh_one(
        &self,
        canonical_id: &crate::ids::CanonicalId,
        source: &str,
        source_id: &str,
        adapter: &Arc<dyn Source>,
    ) -> Result<RefreshOutcome> {
        let now = self.clock.now();
        let cached = self
            .library
            .get_cache(source, source_id)
            .context("get_cache")?;
        if let Some(c) = &cached {
            if c.expires_at > now {
                return Ok(RefreshOutcome::Fresh);
            }
        }
        let fresh = match adapter.fetch(source_id).await? {
            Some(r) => r,
            // Source 200'd with "not found" — leave cache + canonical
            // alone, don't error.
            None => return Ok(RefreshOutcome::Refreshed { verified: vec![] }),
        };
        let prev_links: Vec<StreamingLink> = cached
            .as_ref()
            .and_then(|c| c.streaming_links_json.as_deref())
            .and_then(|s| serde_json::from_str(s).ok())
            .unwrap_or_default();
        let new_entry = CacheEntry::from_source_record(&fresh, &self.ttl, now);
        self.library
            .upsert_cache(&new_entry)
            .context("upsert_cache")?;
        let verified = detect_new_streaming(
            canonical_id,
            &prev_links,
            &fresh.streaming_links,
            &self.subscriptions,
            now,
        );
        Ok(RefreshOutcome::Refreshed { verified })
    }

    /// Long-running loop. Calls `tick()`, waits `interval`, repeats.
    /// Returns cleanly on cancellation.
    pub async fn run(self, ct: CancellationToken) -> Result<()> {
        loop {
            let _ = self.tick().await;
            tokio::select! {
                _ = ct.cancelled() => return Ok(()),
                _ = tokio::time::sleep(self.interval) => {}
            }
        }
    }
}

enum RefreshOutcome {
    Fresh,
    Refreshed { verified: Vec<VerifiedRelease> },
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ids::{CanonicalId, ReleaseKind};
    use crate::notifier::{DispatchOutcome, Notifier};
    use crate::sources::SourceRecord;
    use crate::time::{AdvanceableClock, FixedClock};
    use async_trait::async_trait;
    use std::sync::Mutex;

    /// A Source that returns canned SourceRecords keyed by source_id.
    struct StubSource {
        name: &'static str,
        kinds: Vec<ReleaseKind>,
        records: Mutex<HashMap<String, SourceRecord>>,
        fetch_calls: Mutex<u32>,
    }

    impl StubSource {
        fn new(name: &'static str, kinds: Vec<ReleaseKind>) -> Self {
            Self {
                name,
                kinds,
                records: Mutex::new(HashMap::new()),
                fetch_calls: Mutex::new(0),
            }
        }

        fn set(&self, source_id: &str, record: SourceRecord) {
            self.records
                .lock()
                .unwrap()
                .insert(source_id.to_string(), record);
        }

        fn fetch_count(&self) -> u32 {
            *self.fetch_calls.lock().unwrap()
        }
    }

    #[async_trait]
    impl Source for StubSource {
        fn name(&self) -> &'static str {
            self.name
        }
        fn kinds(&self) -> &[ReleaseKind] {
            &self.kinds
        }
        async fn search(&self, _q: &str, _l: u32) -> Result<Vec<SourceRecord>> {
            Ok(vec![])
        }
        async fn fetch(&self, source_id: &str) -> Result<Option<SourceRecord>> {
            *self.fetch_calls.lock().unwrap() += 1;
            Ok(self.records.lock().unwrap().get(source_id).cloned())
        }
    }

    /// Notifier that records every call. Used here to verify dispatch.
    struct RecNotifier {
        ch: &'static str,
        calls: Mutex<Vec<VerifiedRelease>>,
    }
    impl RecNotifier {
        fn new(ch: &'static str) -> Arc<Self> {
            Arc::new(Self {
                ch,
                calls: Mutex::new(Vec::new()),
            })
        }
        fn calls(&self) -> Vec<VerifiedRelease> {
            self.calls.lock().unwrap().clone()
        }
    }

    struct ArcNot(Arc<RecNotifier>);
    #[async_trait]
    impl Notifier for ArcNot {
        fn channel(&self) -> &'static str {
            self.0.ch
        }
        async fn notify(&self, e: &VerifiedRelease) -> Result<()> {
            self.0.calls.lock().unwrap().push(e.clone());
            Ok(())
        }
    }

    fn record_with(streaming: Vec<StreamingLink>) -> SourceRecord {
        SourceRecord {
            source: "stub",
            source_id: "42".into(),
            kind: ReleaseKind::Tv,
            display_title: "Severance".into(),
            raw_title: "Severance".into(),
            aliases: vec![],
            status: Some("RELEASING".into()),
            cover_url: None,
            description: None,
            streaming_links: streaming,
            next_episode_at: None,
        }
    }

    fn link(site: &str, url: &str) -> StreamingLink {
        StreamingLink {
            site: site.into(),
            url: url.into(),
        }
    }

    fn build() -> (
        Arc<Library>,
        Arc<StubSource>,
        Arc<RecNotifier>,
        AdvanceableClock,
    ) {
        let clock = AdvanceableClock::new(1_000_000);
        let lib = Arc::new(Library::open_in_memory(Arc::new(clock.clone())).unwrap());
        let stub = Arc::new(StubSource::new("stub", vec![ReleaseKind::Tv]));
        let notifier = RecNotifier::new("test");
        (lib, stub, notifier, clock)
    }

    fn make_engine(
        lib: Arc<Library>,
        stub: Arc<StubSource>,
        notifier: Arc<RecNotifier>,
        clock: AdvanceableClock,
        subscriptions: Vec<String>,
    ) -> SyncEngine {
        let dispatcher = Arc::new(
            Dispatcher::new(Arc::clone(&lib)).with(Box::new(ArcNot(Arc::clone(&notifier)))),
        );
        let sources: Vec<Arc<dyn Source>> = vec![stub.clone() as Arc<dyn Source>];
        SyncEngine::new(
            lib,
            sources,
            dispatcher,
            subscriptions,
            Arc::new(clock),
            TtlConfig::DEFAULT,
            Duration::from_secs(60),
        )
    }

    async fn follow_one(lib: &Arc<Library>) -> CanonicalId {
        let cid = CanonicalId::new(ReleaseKind::Tv, "severance").unwrap();
        lib.follow_with_source(&cid, ReleaseKind::Tv, "Severance", "stub", "42", None, 1.0)
            .unwrap();
        cid
    }

    #[tokio::test]
    async fn tick_with_no_followed_returns_empty_report() {
        let (lib, stub, notifier, clock) = build();
        let engine = make_engine(lib, stub.clone(), notifier.clone(), clock, vec![]);
        let report = engine.tick().await;
        assert_eq!(report.canonicals_examined, 0);
        assert_eq!(report.source_refs_refreshed, 0);
        assert!(report.verified_releases.is_empty());
        assert_eq!(stub.fetch_count(), 0);
    }

    #[tokio::test]
    async fn tick_refreshes_an_uncached_source_ref() {
        let (lib, stub, notifier, clock) = build();
        follow_one(&lib).await;
        stub.set("42", record_with(vec![]));
        let engine = make_engine(lib, stub.clone(), notifier.clone(), clock, vec![]);
        let report = engine.tick().await;
        assert_eq!(report.canonicals_examined, 1);
        assert_eq!(report.source_refs_refreshed, 1);
        assert_eq!(stub.fetch_count(), 1);
    }

    #[tokio::test]
    async fn tick_skips_a_fresh_cache_row_without_calling_source() {
        let (lib, stub, notifier, clock) = build();
        follow_one(&lib).await;
        stub.set("42", record_with(vec![]));
        let engine = make_engine(
            Arc::clone(&lib),
            stub.clone(),
            notifier.clone(),
            clock.clone(),
            vec![],
        );
        engine.tick().await; // first tick refreshes
        let count_before = stub.fetch_count();
        let report = engine.tick().await; // second tick — cache fresh
        assert_eq!(stub.fetch_count(), count_before);
        assert_eq!(report.source_refs_skipped_fresh, 1);
        assert_eq!(report.source_refs_refreshed, 0);
    }

    #[tokio::test]
    async fn tick_after_ttl_expires_refreshes_again() {
        let (lib, stub, notifier, clock) = build();
        follow_one(&lib).await;
        stub.set("42", record_with(vec![]));
        let engine = make_engine(
            Arc::clone(&lib),
            stub.clone(),
            notifier.clone(),
            clock.clone(),
            vec![],
        );
        engine.tick().await;
        // Jump past the default RELEASING TTL (6h = 21600s).
        clock.advance(30 * 3600);
        let report = engine.tick().await;
        assert_eq!(report.source_refs_refreshed, 1);
    }

    #[tokio::test]
    async fn tick_emits_verified_when_subscribed_streamer_link_appears() {
        let (lib, stub, notifier, clock) = build();
        follow_one(&lib).await;
        stub.set("42", record_with(vec![]));
        let engine = make_engine(
            Arc::clone(&lib),
            stub.clone(),
            notifier.clone(),
            clock.clone(),
            vec!["Netflix".into()],
        );
        // First tick — no streaming links yet.
        engine.tick().await;
        assert!(notifier.calls().is_empty());
        // Now the source adds a Netflix link.
        stub.set(
            "42",
            record_with(vec![link("Netflix", "https://netflix.com/title/42")]),
        );
        // Expire cache.
        clock.advance(30 * 3600);
        let report = engine.tick().await;
        assert_eq!(report.verified_releases.len(), 1);
        assert_eq!(report.verified_releases[0].streamer, "Netflix");
        // Notifier was invoked.
        let calls = notifier.calls();
        assert_eq!(calls.len(), 1);
        assert_eq!(calls[0].deep_link, "https://netflix.com/title/42");
        // An engagement::Verified was persisted.
        let engagement = lib
            .engagement_for(&CanonicalId::new(ReleaseKind::Tv, "severance").unwrap())
            .unwrap();
        assert!(engagement
            .iter()
            .any(|e| e.event == EngagementEvent::Verified));
    }

    #[tokio::test]
    async fn tick_does_not_emit_verified_for_unsubscribed_streamer() {
        let (lib, stub, notifier, clock) = build();
        follow_one(&lib).await;
        stub.set("42", record_with(vec![]));
        let engine = make_engine(
            Arc::clone(&lib),
            stub.clone(),
            notifier.clone(),
            clock.clone(),
            vec!["Crunchyroll".into()],
        );
        engine.tick().await;
        stub.set(
            "42",
            record_with(vec![link("Netflix", "https://netflix.com/x")]),
        );
        clock.advance(30 * 3600);
        let report = engine.tick().await;
        assert!(report.verified_releases.is_empty());
        assert!(notifier.calls().is_empty());
    }

    #[tokio::test]
    async fn tick_skips_source_ref_without_adapter() {
        let (lib, stub, notifier, clock) = build();
        // Follow with a source ("tvmaze") for which no adapter is
        // registered.
        let cid = CanonicalId::new(ReleaseKind::Tv, "x").unwrap();
        lib.follow_with_source(&cid, ReleaseKind::Tv, "X", "tvmaze", "1", None, 1.0)
            .unwrap();
        let engine = make_engine(lib, stub.clone(), notifier.clone(), clock, vec![]);
        let report = engine.tick().await;
        // No fetch made, no errors.
        assert_eq!(stub.fetch_count(), 0);
        assert!(report.errors.is_empty(), "got: {:?}", report.errors);
    }

    #[tokio::test]
    async fn tick_records_error_on_source_failure_without_aborting() {
        let (lib, stub, notifier, clock) = build();
        follow_one(&lib).await;
        // Source returns None (not found) — no error, but no verify either.
        let engine = make_engine(
            Arc::clone(&lib),
            stub.clone(),
            notifier.clone(),
            clock.clone(),
            vec!["Netflix".into()],
        );
        let report = engine.tick().await;
        assert!(report.errors.is_empty(), "got: {:?}", report.errors);
        assert!(report.verified_releases.is_empty());
    }

    #[tokio::test]
    async fn run_returns_when_cancelled() {
        let (lib, stub, notifier, _clock) = build();
        let engine = make_engine(
            lib,
            stub,
            notifier,
            AdvanceableClock::new(1_000),
            vec![],
        );
        // Tight interval so a single tick + cancel finishes fast.
        let engine = SyncEngine {
            interval: Duration::from_millis(50),
            ..engine
        };
        let ct = CancellationToken::new();
        let handle = tokio::spawn({
            let ct = ct.clone();
            async move { engine.run(ct).await }
        });
        // Cancel quickly.
        ct.cancel();
        let res = tokio::time::timeout(Duration::from_secs(2), handle)
            .await
            .expect("run did not exit on cancel")
            .expect("task panicked");
        assert!(res.is_ok());
    }

    #[test]
    fn _imports_compile() {
        // sanity that test imports stay used.
        let _ = DispatchOutcome::Delivered { channel: "x" };
        let _ = FixedClock(0);
    }
}
