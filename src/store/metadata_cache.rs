//! Ephemeral cache of source metadata. TTL-bounded; losing it never
//! loses user state.
//!
//! TTL is per-status (releasing shows refresh fast; finished shows
//! barely ever). The store layer only persists the precomputed
//! `expires_at`; the policy lives in [`TtlConfig`] so it stays out of
//! SQL.

use anyhow::{Context, Result};
use rusqlite::params;
#[cfg(test)]
use rusqlite::{OptionalExtension, Row};

use super::Db;

const HOUR: i64 = 3_600;
const DAY: i64 = 86_400;

/// Parsed view of the `status` column. Unknown strings (or NULL) fall
/// into [`CacheStatus::Unknown`] so the policy still has a defined TTL.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum CacheStatus {
    Releasing,
    NotYetReleased,
    Finished,
    Unknown,
}

impl CacheStatus {
    /// Accepts AniList's canonical `RELEASING`, `NOT_YET_RELEASED`,
    /// `FINISHED` plus a few common spellings. Anything else maps to
    /// `Unknown`.
    pub(crate) fn parse(s: Option<&str>) -> Self {
        match s.unwrap_or("").to_ascii_uppercase().as_str() {
            "RELEASING" | "CURRENTLY_AIRING" => Self::Releasing,
            "NOT_YET_RELEASED" => Self::NotYetReleased,
            "FINISHED" => Self::Finished,
            _ => Self::Unknown,
        }
    }
}

/// Per-status TTLs in seconds. Defaults come from spec §5.5; env vars
/// override (testing + power users).
#[derive(Debug, Clone, Copy)]
pub(crate) struct TtlConfig {
    pub releasing: i64,
    pub not_yet_released: i64,
    pub finished: i64,
    pub unknown: i64,
}

impl TtlConfig {
    pub(crate) const DEFAULT: Self = Self {
        releasing: 6 * HOUR,
        not_yet_released: 48 * HOUR,
        finished: 30 * DAY,
        unknown: 24 * HOUR,
    };

    pub(crate) fn ttl_for(&self, status: CacheStatus) -> i64 {
        match status {
            CacheStatus::Releasing => self.releasing,
            CacheStatus::NotYetReleased => self.not_yet_released,
            CacheStatus::Finished => self.finished,
            CacheStatus::Unknown => self.unknown,
        }
    }

    /// `expires_at` for a row fetched now with the given status.
    pub(crate) fn expires_at(&self, status: CacheStatus, fetched_at: i64) -> i64 {
        fetched_at + self.ttl_for(status)
    }
}

/// One row of metadata_cache. Mirrors V0001 + V0002 columns.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct CacheEntry {
    pub source: String,
    pub source_id: String,
    pub display_title: Option<String>,
    pub title_english: Option<String>,
    pub title_native: Option<String>,
    pub status: Option<String>,
    pub total_episodes: Option<i64>,
    pub format: Option<String>,
    pub next_episode_number: Option<i64>,
    pub next_episode_airs_at: Option<i64>,
    pub fetched_at: i64,
    pub expires_at: i64,
    // V0002 — extended TUI detail-pane fields.
    pub cover_image_url: Option<String>,
    pub description: Option<String>,
    pub score: Option<f64>,
    pub studios: Option<String>,
    pub streaming_links_json: Option<String>,
}

impl CacheEntry {
    /// Picks the best alternate title — English first, then native. The
    /// canonical's own `display_title` is what users see by default;
    /// this is the "second language" pass for hero subheaders and the
    /// LLM context export.
    pub(crate) fn title_priority(&self) -> Option<&str> {
        self.title_english
            .as_deref()
            .or(self.title_native.as_deref())
    }

    pub(crate) fn status(&self) -> Option<&str> {
        self.status.as_deref()
    }

    pub(crate) fn format(&self) -> Option<&str> {
        self.format.as_deref()
    }

    pub(crate) fn description(&self) -> Option<&str> {
        self.description.as_deref()
    }

    pub(crate) fn studios(&self) -> Option<&str> {
        self.studios.as_deref()
    }

    pub(crate) fn score(&self) -> Option<f64> {
        self.score
    }

    pub(crate) fn total_episodes(&self) -> Option<i64> {
        self.total_episodes
    }

    pub(crate) fn next_episode(&self) -> Option<i64> {
        self.next_episode_number
    }

    pub(crate) fn next_episode_airs_at(&self) -> Option<i64> {
        self.next_episode_airs_at
    }

    #[cfg(test)]
    fn from_row(row: &Row<'_>) -> rusqlite::Result<Self> {
        Ok(Self {
            source: row.get("source")?,
            source_id: row.get("source_id")?,
            display_title: row.get("display_title")?,
            title_english: row.get("title_english")?,
            title_native: row.get("title_native")?,
            status: row.get("status")?,
            total_episodes: row.get("total_episodes")?,
            format: row.get("format")?,
            next_episode_number: row.get("next_episode_number")?,
            next_episode_airs_at: row.get("next_episode_airs_at")?,
            fetched_at: row.get("fetched_at")?,
            expires_at: row.get("expires_at")?,
            cover_image_url: row.get("cover_image_url")?,
            description: row.get("description")?,
            score: row.get("score")?,
            studios: row.get("studios")?,
            streaming_links_json: row.get("streaming_links_json")?,
        })
    }
}

impl Db {
    /// INSERT OR REPLACE the row. The caller is responsible for
    /// having computed `expires_at` per the TTL policy.
    pub(crate) fn upsert_cache(&self, entry: &CacheEntry) -> Result<()> {
        self.conn()
            .execute(
                "INSERT INTO metadata_cache (\
                    source, source_id, display_title, title_english, title_native, \
                    status, total_episodes, format, \
                    next_episode_number, next_episode_airs_at, \
                    fetched_at, expires_at, \
                    cover_image_url, description, score, studios, streaming_links_json\
                 ) VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11,?12,?13,?14,?15,?16,?17) \
                 ON CONFLICT(source, source_id) DO UPDATE SET \
                    display_title = excluded.display_title,\
                    title_english = excluded.title_english,\
                    title_native = excluded.title_native,\
                    status = excluded.status,\
                    total_episodes = excluded.total_episodes,\
                    format = excluded.format,\
                    next_episode_number = excluded.next_episode_number,\
                    next_episode_airs_at = excluded.next_episode_airs_at,\
                    fetched_at = excluded.fetched_at,\
                    expires_at = excluded.expires_at,\
                    cover_image_url = excluded.cover_image_url,\
                    description = excluded.description,\
                    score = excluded.score,\
                    studios = excluded.studios,\
                    streaming_links_json = excluded.streaming_links_json",
                params![
                    entry.source,
                    entry.source_id,
                    entry.display_title,
                    entry.title_english,
                    entry.title_native,
                    entry.status,
                    entry.total_episodes,
                    entry.format,
                    entry.next_episode_number,
                    entry.next_episode_airs_at,
                    entry.fetched_at,
                    entry.expires_at,
                    entry.cover_image_url,
                    entry.description,
                    entry.score,
                    entry.studios,
                    entry.streaming_links_json,
                ],
            )
            .context("upsert_cache")?;
        Ok(())
    }

    /// Get a row regardless of freshness. Production callers go
    /// through [`crate::library::Library::load_resolved`]; this stays
    /// for tests.
    #[cfg(test)]
    pub(crate) fn get_cache(&self, source: &str, source_id: &str) -> Result<Option<CacheEntry>> {
        self.conn()
            .query_row(
                "SELECT * FROM metadata_cache WHERE source = ?1 AND source_id = ?2",
                params![source, source_id],
                CacheEntry::from_row,
            )
            .optional()
            .context("get_cache")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fresh() -> Db {
        Db::open_in_memory().unwrap()
    }

    fn entry(source_id: &str, status: &str, fetched_at: i64, expires_at: i64) -> CacheEntry {
        CacheEntry {
            source: "anilist".into(),
            source_id: source_id.into(),
            display_title: Some(format!("Show {source_id}")),
            title_english: Some(format!("Show {source_id} EN")),
            title_native: Some(format!("Show {source_id} JP")),
            status: Some(status.into()),
            total_episodes: Some(12),
            format: Some("TV".into()),
            next_episode_number: Some(3),
            next_episode_airs_at: Some(fetched_at + 3600),
            fetched_at,
            expires_at,
            cover_image_url: None,
            description: None,
            score: None,
            studios: None,
            streaming_links_json: None,
        }
    }

    #[test]
    fn ttl_defaults_match_spec() {
        let d = TtlConfig::DEFAULT;
        assert_eq!(d.ttl_for(CacheStatus::Releasing), 6 * HOUR);
        assert_eq!(d.ttl_for(CacheStatus::NotYetReleased), 48 * HOUR);
        assert_eq!(d.ttl_for(CacheStatus::Finished), 30 * DAY);
        assert!(d.ttl_for(CacheStatus::Unknown) > 0);
    }

    #[test]
    fn parse_status_handles_anilist_strings_and_unknown() {
        assert_eq!(
            CacheStatus::parse(Some("RELEASING")),
            CacheStatus::Releasing
        );
        assert_eq!(
            CacheStatus::parse(Some("currently_airing")),
            CacheStatus::Releasing
        );
        assert_eq!(
            CacheStatus::parse(Some("NOT_YET_RELEASED")),
            CacheStatus::NotYetReleased
        );
        assert_eq!(CacheStatus::parse(Some("FINISHED")), CacheStatus::Finished);
        assert_eq!(CacheStatus::parse(Some("WEIRD")), CacheStatus::Unknown);
        assert_eq!(CacheStatus::parse(None), CacheStatus::Unknown);
    }

    #[test]
    fn expires_at_computes_from_fetched_plus_ttl() {
        let d = TtlConfig::DEFAULT;
        assert_eq!(d.expires_at(CacheStatus::Releasing, 1000), 1000 + 6 * HOUR);
    }

    #[test]
    fn upsert_then_get_round_trips() {
        let db = fresh();
        let e = entry("21", "RELEASING", 100, 200);
        db.upsert_cache(&e).unwrap();
        let got = db.get_cache("anilist", "21").unwrap().unwrap();
        assert_eq!(got, e);
    }

    #[test]
    fn upsert_replaces_existing_row() {
        let db = fresh();
        db.upsert_cache(&entry("21", "RELEASING", 100, 200))
            .unwrap();
        let mut updated = entry("21", "FINISHED", 500, 1000);
        updated.display_title = Some("Renamed".into());
        db.upsert_cache(&updated).unwrap();
        let got = db.get_cache("anilist", "21").unwrap().unwrap();
        assert_eq!(got.display_title.as_deref(), Some("Renamed"));
        assert_eq!(got.status.as_deref(), Some("FINISHED"));
        assert_eq!(got.expires_at, 1000);
    }
}
