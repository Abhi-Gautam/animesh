//! In-memory view model the TUI renders from.
//!
//! `Library::load(db, now, windows)` reads every active follow + its
//! cache + its watch progress and produces one `Show` per item. Panes
//! are derived (not persisted) by calling `bucket()` per show.

use anyhow::Result;
use serde::Deserialize;

use crate::store::{CacheEntry, Db, ListFilter, TrackedItem, WatchProgress};
use crate::tui::pane::{bucket, BucketInputs, Pane, Windows};

#[derive(Debug, Clone)]
pub struct Show {
    pub item: TrackedItem,
    pub cache: Option<CacheEntry>,
    pub progress: Option<WatchProgress>,
    pub pane: Option<Pane>,
    /// Parsed streaming links (if cached).
    pub streaming: Vec<StreamingLink>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct StreamingLink {
    pub site: Option<String>,
    pub url: Option<String>,
    pub color: Option<String>,
    #[serde(default, rename = "type")]
    pub link_type: Option<String>,
}

impl Show {
    pub fn seen(&self) -> i64 {
        self.progress.as_ref().map(|p| p.seen).unwrap_or(0)
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
        &self.item.display_title
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
}

pub struct Library {
    pub shows: Vec<Show>,
}

impl Library {
    pub fn load(db: &Db, now: i64, windows: Windows) -> Result<Self> {
        let items = db.list_follows(ListFilter::Active)?;
        let mut shows = Vec::with_capacity(items.len());
        for item in items {
            let cache = db.get_cache(&item.source, &item.source_id)?;
            let progress = db.get_watch(&item.source, &item.source_id)?;
            let streaming: Vec<StreamingLink> = cache
                .as_ref()
                .and_then(|c| c.streaming_links_json.as_deref())
                .and_then(|j| serde_json::from_str::<Vec<StreamingLink>>(j).ok())
                .unwrap_or_default();
            let mut show = Show {
                item,
                cache,
                progress,
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

    /// Replace progress for one show in-memory (mirror of a store mutation).
    pub fn set_progress(&mut self, source: &str, source_id: &str, seen: i64, now: i64) {
        for s in &mut self.shows {
            if s.item.source == source && s.item.source_id == source_id {
                s.progress = Some(WatchProgress {
                    seen,
                    updated_at: now,
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
