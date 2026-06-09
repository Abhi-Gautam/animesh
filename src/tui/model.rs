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
    CacheEntry, CanonicalRelease, Engagement, EngagementEvent, SourceRef,
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
    pub fn seen(&self) -> i64 {
        let Some(ev) = &self.last_completed else {
            return 0;
        };
        let Some(meta) = &ev.meta else {
            return 0;
        };
        // meta is JSON `{"seen": N}`. Anything else → 0.
        serde_json::from_str::<serde_json::Value>(meta)
            .ok()
            .and_then(|v| v.get("seen").and_then(|s| s.as_i64()))
            .unwrap_or(0)
    }

    pub fn total(&self) -> Option<i64> {
        self.cache.as_ref().and_then(|c| c.total_episodes)
    }

    pub fn next_episode(&self) -> Option<i64> {
        self.cache.as_ref().and_then(|c| c.next_episode_number)
    }

    pub fn airs_at(&self) -> Option<i64> {
        self.cache.as_ref().and_then(|c| c.next_episode_airs_at)
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
        self.cache
            .as_ref()
            .and_then(|c| c.title_english.as_deref().or(c.title_native.as_deref()))
    }

    pub fn studios(&self) -> Option<&str> {
        self.cache.as_ref().and_then(|c| c.studios.as_deref())
    }

    pub fn score(&self) -> Option<f64> {
        self.cache.as_ref().and_then(|c| c.score)
    }

    pub fn status(&self) -> Option<&str> {
        self.cache.as_ref().and_then(|c| c.status.as_deref())
    }

    pub fn description(&self) -> Option<&str> {
        self.cache.as_ref().and_then(|c| c.description.as_deref())
    }

    pub fn format(&self) -> Option<&str> {
        self.cache.as_ref().and_then(|c| c.format.as_deref())
    }

    pub fn verified_streamer(&self) -> Option<String> {
        let meta = self.last_verified.as_ref()?.meta.as_deref()?;
        serde_json::from_str::<serde_json::Value>(meta)
            .ok()
            .and_then(|v| v.get("streamer")?.as_str().map(str::to_string))
    }

    pub fn verified_url(&self) -> Option<String> {
        let meta = self.last_verified.as_ref()?.meta.as_deref()?;
        serde_json::from_str::<serde_json::Value>(meta)
            .ok()
            .and_then(|v| v.get("url")?.as_str().map(str::to_string))
    }

    pub fn verified_at(&self) -> Option<i64> {
        Some(self.last_verified.as_ref()?.occurred_at)
    }

    /// First future air/release time across cache fields. Today only
    /// `next_episode_airs_at` is populated; this is the seam where music
    /// and film release-date fields will plug in later without changing
    /// callers.
    pub fn next_drop_at(&self) -> Option<i64> {
        self.cache.as_ref().and_then(|c| c.next_episode_airs_at)
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
    /// Build the view-model from the durable state. Canonicals with no
    /// attached source_ref are silently skipped — they shouldn't exist
    /// in practice (every followed canonical attaches one at follow
    /// time) but we don't want a misshapen row to crash the TUI.
    pub fn load(
        facade: &Facade,
        now: i64,
        windows: Windows,
        subs: &crate::tui::subs::Subs,
    ) -> Result<Self> {
        let canonicals = facade.followed()?;
        let mut shows = Vec::with_capacity(canonicals.len());
        for canonical in canonicals {
            let refs = facade.source_refs_for(&canonical.id)?;
            let Some(primary_source) = refs.into_iter().next() else {
                continue;
            };
            let cache = facade.get_cache(&primary_source.source, &primary_source.source_id)?;
            let last_completed =
                facade.last_engagement(&canonical.id, EngagementEvent::Completed)?;
            let last_verified =
                facade.last_engagement(&canonical.id, EngagementEvent::Verified)?;
            let streaming: Vec<StreamingLink> = cache
                .as_ref()
                .and_then(|c| c.streaming_links_json.as_deref())
                .and_then(|j| serde_json::from_str::<Vec<StreamingLink>>(j).ok())
                .unwrap_or_default();
            let mut show = Show {
                canonical,
                primary_source,
                cache,
                last_completed,
                last_verified,
                subscribed_match: false,
                pane: None,
                streaming,
            };
            show.subscribed_match = show
                .verified_streamer()
                .as_deref()
                .map(|s| subs.matches(s))
                .unwrap_or(false);
            show.pane = bucket(show_inputs(&show), now, windows);
            shows.push(show);
        }
        Ok(Self { shows })
    }

    /// Re-derive each show's `pane` from current state. Call after
    /// the user mutates progress (e.g. `w` key) or on tick.
    pub fn recompute_panes(
        &mut self,
        now: i64,
        windows: Windows,
        subs: &crate::tui::subs::Subs,
    ) {
        for s in &mut self.shows {
            s.subscribed_match = s
                .verified_streamer()
                .as_deref()
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
                    // id=0 is the sentinel for "in-memory, never
                    // persisted with this id" — only set_progress
                    // creates these. A reload via Shelf::load
                    // overwrites with the real row.
                    id: 0,
                    canonical_id: canonical_id.clone(),
                    event: EngagementEvent::Completed,
                    occurred_at: now,
                    meta: Some(format!("{{\"seen\":{seen}}}")),
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
