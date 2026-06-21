//! External-source adapters.
//!
//! Port/adapter boundary:
//! - callers see [`SourceAdapter`] + [`SourceRegistry`]
//! - each source module owns its HTTP client, request shaping, raw payload
//!   construction, and parser
//! - `reqwest::` remains confined to source modules

use std::future::Future;
use std::pin::Pin;

use anyhow::Result;

use crate::ingest::{RawSourcePayload, SourceParser};

pub(crate) mod anilist;
pub(crate) mod itunes;
pub(crate) mod jikan;
pub(crate) mod kitsu;
pub(crate) mod musicbrainz;
pub(crate) mod tvmaze;

pub(crate) type SourceFuture<'a, T> = Pin<Box<dyn Future<Output = Result<T>> + Send + 'a>>;

/// Source port used by ingestion/search orchestration. A source exposes only:
/// 1. search: query remote source when the user explicitly asks online search
/// 2. ingest: fetch source-owned information for a selected/followed source id
pub(crate) trait SourceAdapter: Send + Sync {
    fn source(&self) -> &'static str;

    fn parser(&self) -> &dyn SourceParser;

    fn search<'a>(
        &'a self,
        query: &'a str,
        limit: u32,
        now: i64,
    ) -> SourceFuture<'a, Vec<RawSourcePayload>>;

    #[allow(dead_code)]
    fn ingest<'a>(
        &'a self,
        source_id: &'a str,
        now: i64,
    ) -> SourceFuture<'a, Option<RawSourcePayload>>;
}

pub(crate) struct SourceRegistry {
    adapters: Vec<Box<dyn SourceAdapter>>,
}

impl SourceRegistry {
    pub(crate) fn new(adapters: Vec<Box<dyn SourceAdapter>>) -> Self {
        Self { adapters }
    }

    #[allow(dead_code)]
    pub(crate) fn empty() -> Self {
        Self {
            adapters: Vec::new(),
        }
    }

    pub(crate) fn production() -> Self {
        Self::new(vec![
            Box::new(anilist::AniListSource::new()),
            Box::new(jikan::JikanSource::new()),
            Box::new(kitsu::KitsuSource::new()),
            Box::new(tvmaze::TvMazeSource::new()),
            Box::new(musicbrainz::MusicBrainzSource::new()),
            Box::new(itunes::ItunesSource::new()),
        ])
    }

    #[allow(dead_code)]
    pub(crate) fn adapters(&self) -> &[Box<dyn SourceAdapter>] {
        &self.adapters
    }

    pub(crate) fn adapter(&self, source: &str) -> Option<&dyn SourceAdapter> {
        self.adapters
            .iter()
            .map(|adapter| adapter.as_ref())
            .find(|adapter| adapter.source() == source)
    }

    pub(crate) fn search_adapters(&self) -> Vec<&dyn SourceAdapter> {
        self.adapters
            .iter()
            .map(|adapter| adapter.as_ref())
            .collect()
    }
}

pub(crate) fn stable_hash(input: &str) -> String {
    // FNV-1a 64-bit. Deterministic identity for request/response payloads
    // without adding a crypto dependency.
    let mut hash = 0xcbf29ce484222325_u64;
    for byte in input.as_bytes() {
        hash ^= u64::from(*byte);
        hash = hash.wrapping_mul(0x100000001b3);
    }
    format!("{hash:016x}")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn production_discovery_searches_all_enabled_adapters() {
        let registry = SourceRegistry::production();
        let sources: Vec<&str> = registry
            .search_adapters()
            .into_iter()
            .map(|adapter| adapter.source())
            .collect();
        assert_eq!(
            sources,
            vec![
                "anilist",
                "jikan",
                "kitsu",
                "tvmaze",
                "musicbrainz",
                "itunes"
            ]
        );
    }

    #[test]
    fn registry_can_lookup_selected_source_adapter() {
        let registry = SourceRegistry::production();
        assert_eq!(registry.adapter("jikan").unwrap().source(), "jikan");
        assert_eq!(registry.adapter("kitsu").unwrap().source(), "kitsu");
        assert_eq!(registry.adapter("tvmaze").unwrap().source(), "tvmaze");
        assert_eq!(
            registry.adapter("musicbrainz").unwrap().source(),
            "musicbrainz"
        );
        assert_eq!(registry.adapter("itunes").unwrap().source(), "itunes");
    }

    #[test]
    fn production_discovery_budget_matches_enabled_searchable_adapters() {
        let registry = SourceRegistry::production();
        assert_eq!(registry.search_adapters().len(), 6);
    }
}
