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
    pub pane: Option<Pane>,
    /// Parsed streaming links (if cached).
    pub streaming: Vec<StreamingLink>,
}

/// Tolerant streaming-link shape. The cache JSON might come from one of
/// two writers — the legacy v0.4 `MediaExternalLink` (with `color` and
/// `type` fields) or the v0.5 [`crate::sources::StreamingLink`]
/// (just `site` and `url`). Both deserialize cleanly into this shape
/// because every field is optional with serde default.
#[derive(Debug, Clone, Deserialize)]
pub struct StreamingLink {
    #[serde(default)]
    pub site: Option<String>,
    #[serde(default)]
    pub url: Option<String>,
    #[serde(default)]
    pub color: Option<String>,
    #[serde(default, rename = "type")]
    pub link_type: Option<String>,
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

    pub fn source(&self) -> &str {
        &self.primary_source.source
    }

    pub fn source_id(&self) -> &str {
        &self.primary_source.source_id
    }

    pub fn user_note(&self) -> Option<&str> {
        self.canonical.user_note.as_deref()
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

    pub fn cover_url(&self) -> Option<&str> {
        self.cache
            .as_ref()
            .and_then(|c| c.cover_image_url.as_deref())
    }

    pub fn cover_ascii(&self) -> Option<&str> {
        self.canonical.cover_ascii.as_deref()
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
    pub fn load(facade: &Facade, now: i64, windows: Windows) -> Result<Self> {
        let canonicals = facade.followed()?;
        let mut shows = Vec::with_capacity(canonicals.len());
        for canonical in canonicals {
            let refs = facade.source_refs_for(&canonical.id)?;
            let Some(primary_source) = refs.into_iter().next() else {
                continue;
            };
            let cache = facade.get_cache(&primary_source.source, &primary_source.source_id)?;
            let last_completed = facade.last_engagement(&canonical.id, EngagementEvent::Completed)?;
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
                pane: None,
                streaming,
            };
            show.pane = bucket(show_inputs(&show), now, windows);
            shows.push(show);
        }
        Ok(Self { shows })
    }

    /// Re-derive each show's `pane` from current state. Call after
    /// the user mutates progress (e.g. `w` key) or on tick.
    pub fn recompute_panes(&mut self, now: i64, windows: Windows) {
        for s in &mut self.shows {
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
        seen: s.seen(),
        total: s.total(),
        next_episode_number: s.next_episode(),
        next_episode_airs_at: s.airs_at(),
    }
}
