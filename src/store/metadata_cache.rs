//! Ephemeral cache of source metadata. TTL-bounded; losing it never
//! loses user state.
//!
//! TTL is per-status (releasing shows refresh fast; finished shows
//! barely ever). The store layer only persists the precomputed
//! `expires_at`; the policy lives in [`TtlConfig`] so it stays out of
//! SQL.

use std::env;

use anyhow::{Context, Result};
use rusqlite::{params, OptionalExtension, Row};

use super::Db;

const HOUR: i64 = 3_600;
const DAY: i64 = 86_400;

/// Parsed view of the `status` column. Unknown strings (or NULL) fall
/// into [`CacheStatus::Unknown`] so the policy still has a defined TTL.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CacheStatus {
    Releasing,
    NotYetReleased,
    Finished,
    Unknown,
}

impl CacheStatus {
    /// Accepts AniList's canonical `RELEASING`, `NOT_YET_RELEASED`,
    /// `FINISHED` plus a few common spellings. Anything else maps to
    /// `Unknown`.
    pub fn parse(s: Option<&str>) -> Self {
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
pub struct TtlConfig {
    pub releasing: i64,
    pub not_yet_released: i64,
    pub finished: i64,
    pub unknown: i64,
}

impl TtlConfig {
    pub const DEFAULT: Self = Self {
        releasing: 6 * HOUR,
        not_yet_released: 48 * HOUR,
        finished: 30 * DAY,
        unknown: 24 * HOUR,
    };

    pub fn from_env() -> Self {
        let d = Self::DEFAULT;
        Self {
            releasing: env_i64("ANIMESH_TTL_RELEASING").unwrap_or(d.releasing),
            not_yet_released: env_i64("ANIMESH_TTL_NOT_YET_RELEASED")
                .unwrap_or(d.not_yet_released),
            finished: env_i64("ANIMESH_TTL_FINISHED").unwrap_or(d.finished),
            unknown: env_i64("ANIMESH_TTL_UNKNOWN").unwrap_or(d.unknown),
        }
    }

    pub fn ttl_for(&self, status: CacheStatus) -> i64 {
        match status {
            CacheStatus::Releasing => self.releasing,
            CacheStatus::NotYetReleased => self.not_yet_released,
            CacheStatus::Finished => self.finished,
            CacheStatus::Unknown => self.unknown,
        }
    }

    /// `expires_at` for a row fetched now with the given status.
    pub fn expires_at(&self, status: CacheStatus, fetched_at: i64) -> i64 {
        fetched_at + self.ttl_for(status)
    }
}

fn env_i64(key: &str) -> Option<i64> {
    env::var(key).ok().and_then(|v| v.parse().ok())
}

/// One row of metadata_cache. Mirrors the schema in V0001.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CacheEntry {
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
}

impl CacheEntry {
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
        })
    }
}

/// Summary used by `doctor` and tests.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CacheStats {
    pub total: i64,
    pub fresh: i64,
    pub expired: i64,
    pub oldest_fetched_at: Option<i64>,
    pub newest_fetched_at: Option<i64>,
}

impl Db {
    /// INSERT OR REPLACE the row. The caller is responsible for
    /// having computed `expires_at` per the TTL policy.
    pub fn upsert_cache(&self, entry: &CacheEntry) -> Result<()> {
        self.conn()
            .execute(
                "INSERT INTO metadata_cache (\
                    source, source_id, display_title, title_english, title_native, \
                    status, total_episodes, format, \
                    next_episode_number, next_episode_airs_at, \
                    fetched_at, expires_at\
                 ) VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11,?12) \
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
                    expires_at = excluded.expires_at",
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
                ],
            )
            .context("upsert_cache")?;
        Ok(())
    }

    /// Get a row regardless of freshness.
    pub fn get_cache(&self, source: &str, source_id: &str) -> Result<Option<CacheEntry>> {
        self.conn()
            .query_row(
                "SELECT * FROM metadata_cache WHERE source = ?1 AND source_id = ?2",
                params![source, source_id],
                CacheEntry::from_row,
            )
            .optional()
            .context("get_cache")
    }

    /// Get a row only if `expires_at > now`. None if missing or stale.
    pub fn get_cache_if_fresh(
        &self,
        source: &str,
        source_id: &str,
        now: i64,
    ) -> Result<Option<CacheEntry>> {
        self.conn()
            .query_row(
                "SELECT * FROM metadata_cache \
                 WHERE source = ?1 AND source_id = ?2 AND expires_at > ?3",
                params![source, source_id, now],
                CacheEntry::from_row,
            )
            .optional()
            .context("get_cache_if_fresh")
    }

    /// Sweep expired rows. Returns the count removed. Triggers will
    /// cascade the delete into search_fts.
    pub fn delete_expired_cache(&self, now: i64) -> Result<usize> {
        let n = self
            .conn()
            .execute(
                "DELETE FROM metadata_cache WHERE expires_at <= ?1",
                params![now],
            )
            .context("delete_expired_cache")?;
        Ok(n)
    }

    /// Aggregate stats for `doctor`.
    pub fn cache_stats(&self, now: i64) -> Result<CacheStats> {
        let conn = self.conn();
        let total: i64 = conn
            .query_row("SELECT COUNT(*) FROM metadata_cache", [], |r| r.get(0))
            .context("cache_stats total")?;
        let fresh: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM metadata_cache WHERE expires_at > ?1",
                params![now],
                |r| r.get(0),
            )
            .context("cache_stats fresh")?;
        let oldest: Option<i64> = conn
            .query_row(
                "SELECT MIN(fetched_at) FROM metadata_cache",
                [],
                |r| r.get(0),
            )
            .ok();
        let newest: Option<i64> = conn
            .query_row(
                "SELECT MAX(fetched_at) FROM metadata_cache",
                [],
                |r| r.get(0),
            )
            .ok();
        Ok(CacheStats {
            total,
            fresh,
            expired: total - fresh,
            oldest_fetched_at: oldest,
            newest_fetched_at: newest,
        })
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
        assert_eq!(CacheStatus::parse(Some("RELEASING")), CacheStatus::Releasing);
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
        db.upsert_cache(&entry("21", "RELEASING", 100, 200)).unwrap();
        let mut updated = entry("21", "FINISHED", 500, 1000);
        updated.display_title = Some("Renamed".into());
        db.upsert_cache(&updated).unwrap();
        let got = db.get_cache("anilist", "21").unwrap().unwrap();
        assert_eq!(got.display_title.as_deref(), Some("Renamed"));
        assert_eq!(got.status.as_deref(), Some("FINISHED"));
        assert_eq!(got.expires_at, 1000);
    }

    #[test]
    fn get_cache_if_fresh_filters_by_expiry() {
        let db = fresh();
        db.upsert_cache(&entry("21", "RELEASING", 0, 100)).unwrap();
        assert!(db.get_cache_if_fresh("anilist", "21", 50).unwrap().is_some());
        assert!(db.get_cache_if_fresh("anilist", "21", 100).unwrap().is_none());
        assert!(db.get_cache_if_fresh("anilist", "21", 200).unwrap().is_none());
        // Stale row is still visible via get_cache.
        assert!(db.get_cache("anilist", "21").unwrap().is_some());
    }

    #[test]
    fn delete_expired_removes_only_expired() {
        let db = fresh();
        db.upsert_cache(&entry("1", "RELEASING", 0, 50)).unwrap();
        db.upsert_cache(&entry("2", "RELEASING", 0, 100)).unwrap();
        db.upsert_cache(&entry("3", "RELEASING", 0, 200)).unwrap();
        let removed = db.delete_expired_cache(100).unwrap();
        assert_eq!(removed, 2, "rows with expires_at <= 100 should be removed");
        assert!(db.get_cache("anilist", "1").unwrap().is_none());
        assert!(db.get_cache("anilist", "2").unwrap().is_none());
        assert!(db.get_cache("anilist", "3").unwrap().is_some());
    }

    #[test]
    fn delete_expired_cascades_into_search_fts() {
        let db = fresh();
        db.upsert_cache(&entry("1", "RELEASING", 0, 50)).unwrap();
        db.upsert_cache(&entry("2", "RELEASING", 0, 200)).unwrap();
        db.delete_expired_cache(100).unwrap();
        // search_fts trigger should have removed row 1.
        let n: i64 = db
            .conn()
            .query_row(
                "SELECT COUNT(*) FROM search_fts WHERE source_id = '1'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(n, 0);
    }

    #[test]
    fn cache_stats_reports_fresh_expired_oldest_newest() {
        let db = fresh();
        db.upsert_cache(&entry("1", "RELEASING", 10, 100)).unwrap();
        db.upsert_cache(&entry("2", "RELEASING", 200, 500)).unwrap();
        db.upsert_cache(&entry("3", "RELEASING", 50, 50)).unwrap();
        let stats = db.cache_stats(150).unwrap();
        assert_eq!(stats.total, 3);
        assert_eq!(stats.fresh, 1, "only row 2 has expires_at > 150");
        assert_eq!(stats.expired, 2);
        assert_eq!(stats.oldest_fetched_at, Some(10));
        assert_eq!(stats.newest_fetched_at, Some(200));
    }
}
