//! External-source adapters.
//!
//! Every external API the project talks to lives here, behind the
//! [`Source`] trait. The trait is small on purpose: search a string,
//! fetch a record by id. That's the contract the canonical service
//! and the sync loop call against.
//!
//! Layering rules (enforced by a greppable CI lint, see
//! `.github/workflows/architecture.yml`):
//!
//!   * `reqwest::` may only appear in `src/sources/` and `src/llm/`.
//!   * Adapters return canonical-shaped [`SourceRecord`]s, never raw
//!     source JSON.
//!   * Each adapter owns its own rate-limiting + retry. The sync loop
//!     does not know which source it's hitting.
//!
//! ## File layout
//!
//! One file per source. If a single file ever exceeds ~500 lines,
//! split it then. The original design had a per-source subdirectory
//! (`anilist/mod.rs`, `anilist/client.rs`, `anilist/types.rs`); that
//! got collapsed because the type/client/impl split is unearned at
//! this scale.

pub mod anilist;

use anyhow::{Context, Result};
use async_trait::async_trait;
use serde::{Deserialize, Serialize};

use crate::ids::ReleaseKind;

/// Small HTTP-GET-bytes helper used by callers that need raw bytes
/// from a source URL (cover art, etc) without owning their own
/// reqwest client. Lives here so the layering rule "reqwest only in
/// sources/, llm/, notifier/" stays mechanically true.
pub async fn fetch_bytes(url: &str) -> Result<Vec<u8>> {
    let resp = reqwest::get(url).await.context("HTTP GET")?;
    let status = resp.status();
    if !status.is_success() {
        return Err(anyhow::anyhow!("HTTP {status} fetching {url}"));
    }
    let bytes = resp.bytes().await.context("read response body")?;
    Ok(bytes.to_vec())
}

/// One normalized record from an external source.
///
/// Lightweight projection of whatever the source's native API returns.
/// `search` populates the identity fields; `fetch` fills in the detail
/// fields. Optional fields are `None` until the source supplies them.
#[derive(Debug, Clone, PartialEq)]
pub struct SourceRecord {
    /// Adapter name — "anilist", "tmdb", "tvmaze", "musicbrainz".
    pub source: &'static str,
    /// Native source identifier as a string. The shape varies per
    /// source (numeric AniList ids, GUID-ish MusicBrainz mbids); we
    /// normalize to TEXT so it round-trips through SQLite.
    pub source_id: String,
    pub kind: ReleaseKind,
    /// Human-friendly display title (English-preferred or source's
    /// preferred form).
    pub display_title: String,
    /// The source's primary title field, unchanged. Persisted in
    /// `source_ref.raw_title` for the LLM canonicalizer's prompt
    /// context.
    pub raw_title: String,
    /// Alternative titles, romanizations, native scripts — anything
    /// the source provides that helps the canonicalizer disambiguate.
    pub aliases: Vec<String>,
    /// Source-specific status string ("RELEASING", "FINISHED",
    /// "Ended", "in_production"). Free-form because status shapes
    /// differ per source; the metadata cache parses this into a
    /// normalized enum at read time.
    pub status: Option<String>,
    /// Cover image URL. Used by the cover-ascii renderer.
    pub cover_url: Option<String>,
    /// Long-form description from the source.
    pub description: Option<String>,
    /// Source-reported streaming providers. The sync loop's verify
    /// step diffs these between refreshes to fire VerifiedRelease.
    pub streaming_links: Vec<StreamingLink>,
    /// Unix seconds for the next scheduled drop, if the source knows.
    pub next_episode_at: Option<i64>,
}

/// One streaming provider entry attached to a SourceRecord.
///
/// `site` is the human-readable streamer name ("Netflix",
/// "Crunchyroll"). `url` is the deep link or watch-page URL the source
/// supplies — when one appears for a subscribed `site`, the verify
/// step fires.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct StreamingLink {
    pub site: String,
    pub url: String,
}

/// Adapter contract.
///
/// Every external API the project talks to implements this. The
/// methods are intentionally narrow: anything broader belongs in the
/// service that orchestrates calls (canonical/, sync/), not in the
/// adapter.
#[async_trait]
pub trait Source: Send + Sync {
    /// Stable identifier. Matches the value persisted in
    /// `source_ref.source` and `metadata_cache.source`.
    fn name(&self) -> &'static str;

    /// Which release kinds this source can return. The canonical
    /// service uses this to skip sources that can't help (don't ask
    /// MusicBrainz about TV shows).
    fn kinds(&self) -> &[ReleaseKind];

    /// Free-form text search. `limit` caps the number of results.
    /// Implementations may return fewer; they MUST NOT return more.
    async fn search(&self, query: &str, limit: u32) -> Result<Vec<SourceRecord>>;

    /// Fetch a full record by native source id. Returns `Ok(None)` if
    /// the source 200s with "not found" semantics (e.g. AniList
    /// `Media: null`), and `Err` for transport-level failures.
    async fn fetch(&self, source_id: &str) -> Result<Option<SourceRecord>>;
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Dyn-safety check. If this compiles, `Box<dyn Source>` is
    /// usable as the canonical service's per-source registry value.
    #[test]
    fn source_trait_is_object_safe() {
        fn _accepts(_x: Box<dyn Source>) {}
    }
}
