//! `animesh doctor` — the EXPLAIN of animesh.
//!
//! Read-only, no-network, sub-10ms. Surfaces enough state for a user
//! to file a meaningful bug report and for an upgrade-debugging
//! session to find its footing.

use std::path::Path;

use anyhow::Result;
use chrono::{TimeZone, Utc};

use crate::store::{Db, MAX_KNOWN_VERSION};

const KV_LAST_ATTEMPT: &str = "sync.last_attempt_at";
const KV_LAST_SUCCESS: &str = "sync.last_success_at";
const KV_LAST_ERROR: &str = "sync.last_error";
const KV_RATELIMIT_REMAINING: &str = "anilist.ratelimit.remaining";
const KV_RATELIMIT_RESET: &str = "anilist.ratelimit.reset_at";

/// Snapshot returned by `report` — used directly by tests so we don't
/// have to parse stdout.
#[derive(Debug, Clone)]
pub struct DoctorReport {
    pub db_path: String,
    pub schema_version: u32,
    pub binary_known_version: u32,
    pub binary_version: &'static str,
    pub active_follows: i64,
    pub dropped_follows: i64,
    pub cache_total: i64,
    pub cache_fresh: i64,
    pub cache_expired: i64,
    pub cache_oldest_iso: Option<String>,
    pub cache_newest_iso: Option<String>,
    pub last_sync_attempt_iso: Option<String>,
    pub last_sync_success_iso: Option<String>,
    pub last_sync_error: Option<String>,
    pub ratelimit_remaining: Option<String>,
    pub ratelimit_reset_iso: Option<String>,
}

pub fn report(db: &Db, db_path: &Path, now: i64) -> Result<DoctorReport> {
    let schema_version = db.schema_version()?;
    let active_follows = db.count_active()?;
    let dropped_follows = db.count_dropped()?;
    let stats = db.cache_stats(now)?;
    let iso = |secs: i64| {
        Utc.timestamp_opt(secs, 0)
            .single()
            .map(|t| t.format("%Y-%m-%d %H:%M:%S UTC").to_string())
    };
    let kv_as_iso = |k: &str| {
        db.kv_get(k)
            .ok()
            .flatten()
            .and_then(|(v, _)| v.parse::<i64>().ok())
            .and_then(iso)
    };
    Ok(DoctorReport {
        db_path: db_path.display().to_string(),
        schema_version,
        binary_known_version: MAX_KNOWN_VERSION,
        binary_version: env!("CARGO_PKG_VERSION"),
        active_follows,
        dropped_follows,
        cache_total: stats.total,
        cache_fresh: stats.fresh,
        cache_expired: stats.expired,
        cache_oldest_iso: stats.oldest_fetched_at.and_then(iso),
        cache_newest_iso: stats.newest_fetched_at.and_then(iso),
        last_sync_attempt_iso: kv_as_iso(KV_LAST_ATTEMPT),
        last_sync_success_iso: kv_as_iso(KV_LAST_SUCCESS),
        last_sync_error: db.kv_get(KV_LAST_ERROR).ok().flatten().map(|(v, _)| v),
        ratelimit_remaining: db
            .kv_get(KV_RATELIMIT_REMAINING)
            .ok()
            .flatten()
            .map(|(v, _)| v),
        ratelimit_reset_iso: kv_as_iso(KV_RATELIMIT_RESET),
    })
}

pub fn format_report(r: &DoctorReport) -> String {
    let mut out = String::new();
    let dash = "—";
    let opt = |s: &Option<String>| s.clone().unwrap_or_else(|| dash.into());
    out.push_str(&format!("animesh {}\n", r.binary_version));
    out.push_str(&format!("  db_path:             {}\n", r.db_path));
    out.push_str(&format!(
        "  schema_version:      V{:04} (binary knows up to V{:04})\n",
        r.schema_version, r.binary_known_version
    ));
    out.push_str("library:\n");
    out.push_str(&format!("  active:              {}\n", r.active_follows));
    out.push_str(&format!("  dropped:             {}\n", r.dropped_follows));
    out.push_str("cache:\n");
    out.push_str(&format!("  total:               {}\n", r.cache_total));
    out.push_str(&format!("  fresh:               {}\n", r.cache_fresh));
    out.push_str(&format!("  expired:             {}\n", r.cache_expired));
    out.push_str(&format!("  oldest entry:        {}\n", opt(&r.cache_oldest_iso)));
    out.push_str(&format!("  newest entry:        {}\n", opt(&r.cache_newest_iso)));
    out.push_str("sync:\n");
    out.push_str(&format!(
        "  last attempt:        {}\n",
        opt(&r.last_sync_attempt_iso)
    ));
    out.push_str(&format!(
        "  last success:        {}\n",
        opt(&r.last_sync_success_iso)
    ));
    out.push_str(&format!("  last error:          {}\n", opt(&r.last_sync_error)));
    out.push_str("anilist rate limit:\n");
    out.push_str(&format!(
        "  remaining:           {}\n",
        opt(&r.ratelimit_remaining)
    ));
    out.push_str(&format!(
        "  resets:              {}\n",
        opt(&r.ratelimit_reset_iso)
    ));
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::store::{CacheEntry, Db};
    use std::path::PathBuf;

    #[test]
    fn fresh_db_report_includes_binary_version_and_zero_counts() {
        let db = Db::open_in_memory().unwrap();
        let r = report(&db, &PathBuf::from("/tmp/animesh.db"), 1_700_000_000).unwrap();
        assert_eq!(r.binary_version, env!("CARGO_PKG_VERSION"));
        assert_eq!(r.schema_version, MAX_KNOWN_VERSION);
        assert_eq!(r.binary_known_version, MAX_KNOWN_VERSION);
        assert_eq!(r.active_follows, 0);
        assert_eq!(r.cache_total, 0);
        assert!(r.last_sync_attempt_iso.is_none());
    }

    #[test]
    fn populated_db_report_reflects_state() {
        let mut db = Db::open_in_memory().unwrap();
        db.add_follow("anilist", "21", "anime", "One Piece", 100)
            .unwrap();
        db.add_follow("anilist", "22", "anime", "Naruto", 100)
            .unwrap();
        db.drop_follow("anilist", "22", 200).unwrap();
        db.upsert_cache(&CacheEntry {
            source: "anilist".into(),
            source_id: "21".into(),
            display_title: Some("One Piece".into()),
            title_english: None,
            title_native: None,
            status: Some("RELEASING".into()),
            total_episodes: None,
            format: None,
            next_episode_number: None,
            next_episode_airs_at: None,
            fetched_at: 1_700_000_000,
            expires_at: 1_700_000_500,
            cover_image_url: None,
            description: None,
            score: None,
            studios: None,
            streaming_links_json: None,
        })
        .unwrap();
        db.kv_set(KV_LAST_ATTEMPT, "1700000000", 1700000000).unwrap();
        db.kv_set(KV_LAST_SUCCESS, "1700000000", 1700000000).unwrap();

        let r = report(&db, &PathBuf::from("/tmp/animesh.db"), 1_700_001_000).unwrap();
        assert_eq!(r.active_follows, 1);
        assert_eq!(r.dropped_follows, 1);
        assert_eq!(r.cache_total, 1);
        assert_eq!(r.cache_fresh, 0, "cache row should be expired by now=1700001000");
        assert_eq!(r.cache_expired, 1);
        assert!(r.last_sync_success_iso.is_some());
    }

    #[test]
    fn format_report_renders_all_sections() {
        let db = Db::open_in_memory().unwrap();
        let r = report(&db, &PathBuf::from("/tmp/animesh.db"), 0).unwrap();
        let s = format_report(&r);
        for needle in [
            "animesh",
            "db_path:",
            "schema_version:",
            "library:",
            "cache:",
            "sync:",
            "anilist rate limit:",
        ] {
            assert!(s.contains(needle), "missing {needle:?} in:\n{s}");
        }
    }
}
