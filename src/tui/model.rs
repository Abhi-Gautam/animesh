//! In-memory view-model the TUI renders from.
//!
//! `Shelf::load(facade, now, windows)` reads every active follow from
//! the [`Library`] facade, joins each with its attached source_ref +
//! cache + latest "completed" engagement, and bucketizes per pane.
//!
//! Renamed from the v0.4 `Library` to disambiguate from the durable
//! [`crate::library::Library`] facade. This shelf is rebuilt every
//! time the durable state mutates (follow, drop, watched, sync).

use anyhow::Result;
use serde::Deserialize;

use crate::ids::CanonicalId;
use crate::library::Library as Facade;
use crate::store::{
    CacheEntry, CanonicalRelease, Engagement, EngagementEvent, EngagementMeta, EngagementSource,
    SourceRef,
};
use crate::tui::pane::{bucket, BucketInputs, Pane, Windows};

#[derive(Debug, Clone)]
pub struct Show {
    /// Durable canonical row. Replaces the v0.4 `TrackedItem`.
    pub canonical: CanonicalRelease,
    /// First attached source_ref (highest confidence). Used for cache
    /// lookups and the legacy display of `(source, source_id)` pairs.
    /// Required: a followed canonical always has at least one
    /// source_ref by `follow_with_source`'s invariant.
    pub primary_source: SourceRef,
    pub cache: Option<CacheEntry>,
    /// Most recent `Completed` engagement event for this canonical.
    /// `seen` derives from its `meta.seen` JSON.
    pub last_completed: Option<Engagement>,
    /// Most recent `EngagementEvent::Verified`, if any. Meta JSON shape:
    /// `{"streamer": "Netflix", "url": "..."}` (written by sync engine).
    pub last_verified: Option<Engagement>,
    /// True when `last_verified.streamer` is in user's subs.
    pub subscribed_match: bool,
    pub pane: Option<Pane>,
    /// Parsed streaming links (if cached).
    pub streaming: Vec<StreamingLink>,
}

/// Tolerant streaming-link shape. Serde silently ignores extra fields,
/// so legacy `MediaExternalLink` JSON (which carried `color` and `type`)
/// still deserializes against this lean shape.
#[derive(Debug, Clone, Deserialize)]
pub struct StreamingLink {
    #[serde(default)]
    pub site: Option<String>,
    #[serde(default)]
    pub url: Option<String>,
}

impl Show {
    /// Project a single resolved row into the TUI's display model.
    /// Parses streaming_links_json once, computes the subs match once,
    /// and runs the pane bucketing once — all O(1) per show with no
    /// further DB hits.
    fn from_resolved(
        r: crate::library::ResolvedRelease,
        now: i64,
        windows: Windows,
        subs: &crate::tui::subs::Subs,
    ) -> Self {
        let streaming: Vec<StreamingLink> = r
            .cache
            .as_ref()
            .and_then(|c| c.streaming_links_json.as_deref())
            .and_then(|j| serde_json::from_str::<Vec<StreamingLink>>(j).ok())
            .unwrap_or_default();
        let subscribed_match = r
            .last_verified
            .as_ref()
            .and_then(|e| e.streamer())
            .map(|s| subs.matches(s))
            .unwrap_or(false);
        let mut show = Self {
            canonical: r.canonical,
            primary_source: r.primary_source,
            cache: r.cache,
            last_completed: r.last_completed,
            last_verified: r.last_verified,
            subscribed_match,
            pane: None,
            streaming,
        };
        show.pane = bucket(show_inputs(&show), now, windows);
        show
    }

    pub fn seen(&self) -> i64 {
        self.last_completed
            .as_ref()
            .and_then(|e| e.seen())
            .unwrap_or(0)
    }

    pub fn total(&self) -> Option<i64> {
        self.cache.as_ref().and_then(|c| c.total_episodes())
    }

    pub fn next_episode(&self) -> Option<i64> {
        self.cache.as_ref().and_then(|c| c.next_episode())
    }

    pub fn airs_at(&self) -> Option<i64> {
        self.cache.as_ref().and_then(|c| c.next_episode_airs_at())
    }

    pub fn display_title(&self) -> &str {
        &self.canonical.display_title
    }

    pub fn canonical_id(&self) -> &CanonicalId {
        &self.canonical.id
    }

    pub fn source_id(&self) -> &str {
        &self.primary_source.source_id
    }

    pub fn romaji(&self) -> Option<&str> {
        self.cache.as_ref().and_then(|c| c.title_priority())
    }

    pub fn studios(&self) -> Option<&str> {
        self.cache.as_ref().and_then(|c| c.studios())
    }

    pub fn score(&self) -> Option<f64> {
        self.cache.as_ref().and_then(|c| c.score())
    }

    pub fn status(&self) -> Option<&str> {
        self.cache.as_ref().and_then(|c| c.status())
    }

    pub fn description(&self) -> Option<&str> {
        self.cache.as_ref().and_then(|c| c.description())
    }

    pub fn format(&self) -> Option<&str> {
        self.cache.as_ref().and_then(|c| c.format())
    }

    pub fn verified_streamer(&self) -> Option<&str> {
        self.last_verified.as_ref()?.streamer()
    }

    pub fn verified_url(&self) -> Option<&str> {
        self.last_verified.as_ref()?.verified_url()
    }

    pub fn verified_at(&self) -> Option<i64> {
        Some(self.last_verified.as_ref()?.occurred_at)
    }

    /// First future air/release time across cache fields. Today only
    /// `next_episode_airs_at` is populated; this is the seam where music
    /// and film release-date fields will plug in later without changing
    /// callers.
    pub fn next_drop_at(&self) -> Option<i64> {
        self.cache.as_ref().and_then(|c| c.next_episode_airs_at())
    }

    pub fn fully_done(&self) -> bool {
        match (self.total(), Some(self.seen())) {
            (Some(t), Some(s)) => s >= t,
            _ => false,
        }
    }
}

pub struct Shelf {
    pub shows: Vec<Show>,
}

impl Shelf {
    /// Build the view-model from the durable state. One round trip to
    /// the DB regardless of follow count — see
    /// [`Facade::load_resolved`].
    pub fn load(
        facade: &Facade,
        now: i64,
        windows: Windows,
        subs: &crate::tui::subs::Subs,
    ) -> Result<Self> {
        let resolved = facade.load_resolved()?;
        let shows = resolved
            .into_iter()
            .map(|r| Show::from_resolved(r, now, windows, subs))
            .collect();
        Ok(Self { shows })
    }

    /// Re-derive each show's `pane` from current state. Call after
    /// the user mutates progress (e.g. `w` key) or on tick.
    pub fn recompute_panes(&mut self, now: i64, windows: Windows, subs: &crate::tui::subs::Subs) {
        for s in &mut self.shows {
            s.subscribed_match = s
                .verified_streamer()
                .map(|n| subs.matches(n))
                .unwrap_or(false);
            s.pane = bucket(show_inputs(s), now, windows);
        }
    }

    /// Replace progress for one show in-memory (mirror of a durable
    /// `engage(Completed, …)` write). Synthesizes an in-memory
    /// engagement so `seen()` reflects the new value without a reload.
    pub fn set_progress(&mut self, canonical_id: &CanonicalId, seen: i64, now: i64) {
        for s in &mut self.shows {
            if &s.canonical.id == canonical_id {
                s.last_completed = Some(Engagement {
                    source: EngagementSource::InMemory,
                    canonical_id: canonical_id.clone(),
                    event: EngagementEvent::Completed,
                    occurred_at: now,
                    meta: Some(EngagementMeta::Completed { seen }),
                });
            }
        }
    }
}

fn show_inputs(s: &Show) -> BucketInputs {
    BucketInputs {
        next_drop_at: s.next_drop_at(),
        verified_playable_at: s.verified_at(),
        subscribed: s.subscribed_match,
        fully_done: s.fully_done(),
    }
}
