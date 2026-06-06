//! `animesh schedule`.
//!
//! Default: shows airing of your followed library in the next N days,
//! served from `metadata_cache`. Stale or missing entries trigger a
//! per-show `by_id` refresh. Empty library hints at `animesh follow`.
//!
//! `--all`: classic global AniList schedule. Side-effects every result
//! into `metadata_cache`, so the picker's corpus warms through normal
//! usage (spec §7.3).
//!
//! `--past`: implies `--all` in v0.3 — followed-only past episodes
//! require historical episode data we don't store. Followed-only past
//! views ship in SP-3.

use anyhow::Result;
use async_trait::async_trait;
use chrono::{FixedOffset, Utc};

use crate::{
    anilist::AniListClient,
    commands::Command,
    renderer::{render_schedule, ScheduleRow},
    store::{
        resolve_db_path, CacheEntry, CacheStatus, Db, ListFilter, TtlConfig,
    },
    utils::{get_user_timezone, match_timezone},
};

pub struct ScheduleCommand {
    interval: u32,
    timezone: Option<String>,
    past: bool,
    all: bool,
}

impl ScheduleCommand {
    pub fn new(interval: u32, timezone: Option<String>, past: bool, all: bool) -> Self {
        Self {
            interval,
            timezone,
            past,
            all,
        }
    }

    fn resolved_timezone(&self) -> FixedOffset {
        if let Some(tz) = &self.timezone {
            match_timezone(tz).unwrap_or_else(|| {
                eprintln!("Invalid timezone: {tz}. Using default timezone.");
                get_user_timezone()
            })
        } else {
            get_user_timezone()
        }
    }

    fn tz_label(&self, tz: FixedOffset) -> String {
        if let Some(s) = &self.timezone {
            s.to_uppercase()
        } else {
            let offset = tz.local_minus_utc();
            let sign = if offset >= 0 { "+" } else { "-" };
            let hours = offset.abs() / 3600;
            let minutes = (offset.abs() % 3600) / 60;
            format!("UTC{sign}{hours:02}:{minutes:02}")
        }
    }

    fn time_range(&self, now: i64) -> (i64, i64) {
        let span = (self.interval as i64) * 24 * 3600;
        if self.past {
            (now - span, now)
        } else {
            (now, now + span)
        }
    }
}

#[async_trait(?Send)]
impl Command for ScheduleCommand {
    async fn execute(&self) -> Result<()> {
        let path = resolve_db_path()?;
        let mut db = Db::open(&path)?;
        let client = AniListClient::new();
        let now = Utc::now().timestamp();
        let tz = self.resolved_timezone();
        let tz_label = self.tz_label(tz);
        let (start, end) = self.time_range(now);

        // --past implies --all for v0.3.
        let mode_all = self.all || self.past;

        let rows = if mode_all {
            schedule_all(&client, &db, start, end, now).await?
        } else {
            if db.count_active()? == 0 {
                println!(
                    "Your library is empty — try `animesh follow --id N` to start, \
                     or `animesh schedule --all` to browse the global AniList schedule."
                );
                return Ok(());
            }
            schedule_followed(&mut db, &client, start, end, now).await?
        };

        let out = render_schedule(&rows, tz, &tz_label, now, true);
        print!("{out}");
        Ok(())
    }
}

/// Followed-only path: build rows from `metadata_cache`, refreshing
/// stale entries per show.
async fn schedule_followed(
    db: &mut Db,
    client: &AniListClient,
    start: i64,
    end: i64,
    now: i64,
) -> Result<Vec<ScheduleRow>> {
    let follows = db.list_follows(ListFilter::Active)?;
    let ttl = TtlConfig::from_env();
    let mut rows: Vec<ScheduleRow> = Vec::new();

    for item in &follows {
        let cached = db.get_cache(&item.source, &item.source_id)?;
        let fresh = match &cached {
            Some(c) => c.expires_at > now,
            None => false,
        };
        let (airs_at, episode) = if !fresh && item.source == "anilist" {
            let id_n: i64 = item.source_id.parse().unwrap_or(0);
            match client.by_id(id_n).await {
                Ok(Some(media)) => {
                    let status = CacheStatus::parse(media.status.as_deref());
                    let entry = CacheEntry {
                        source: "anilist".into(),
                        source_id: item.source_id.clone(),
                        display_title: Some(media.display_title().to_string()),
                        title_english: media.title.english.clone(),
                        title_native: media.title.native.clone(),
                        status: media.status.clone(),
                        total_episodes: media.episodes,
                        format: media.format.clone(),
                        next_episode_number: media.next_airing_episode.map(|n| n.episode),
                        next_episode_airs_at: media.next_airing_episode.map(|n| n.airing_at),
                        fetched_at: now,
                        expires_at: ttl.expires_at(status, now),
                    };
                    let pair = (entry.next_episode_airs_at, entry.next_episode_number);
                    db.upsert_cache(&entry)?;
                    pair
                }
                _ => cached
                    .as_ref()
                    .map(|c| (c.next_episode_airs_at, c.next_episode_number))
                    .unwrap_or((None, None)),
            }
        } else {
            cached
                .as_ref()
                .map(|c| (c.next_episode_airs_at, c.next_episode_number))
                .unwrap_or((None, None))
        };

        if let (Some(at), Some(ep)) = (airs_at, episode) {
            if at >= start && at <= end {
                rows.push(ScheduleRow {
                    title: item.display_title.clone(),
                    episode: ep,
                    airing_at: at,
                });
            }
        }
    }
    rows.sort_by_key(|r| r.airing_at);
    Ok(rows)
}

/// `--all` path: global AniList schedule. Every result is upserted
/// into the cache so the picker corpus warms.
async fn schedule_all(
    client: &AniListClient,
    db: &Db,
    start: i64,
    end: i64,
    now: i64,
) -> Result<Vec<ScheduleRow>> {
    let entries = client.schedule_window(start, end, 50).await?;
    let ttl = TtlConfig::from_env();
    let mut rows: Vec<ScheduleRow> = Vec::with_capacity(entries.len());
    for e in &entries {
        let status = CacheStatus::parse(e.media.status.as_deref());
        let title = e.media.display_title().to_string();
        let source_id = e.media.id.to_string();
        // Warm the cache for the picker corpus.
        let _ = db.upsert_cache(&CacheEntry {
            source: "anilist".into(),
            source_id: source_id.clone(),
            display_title: Some(title.clone()),
            title_english: e.media.title.english.clone(),
            title_native: e.media.title.native.clone(),
            status: e.media.status.clone(),
            total_episodes: e.media.episodes,
            format: e.media.format.clone(),
            next_episode_number: Some(e.episode),
            next_episode_airs_at: Some(e.airing_at),
            fetched_at: now,
            expires_at: ttl.expires_at(status, now),
        });
        rows.push(ScheduleRow {
            title,
            episode: e.episode,
            airing_at: e.airing_at,
        });
    }
    rows.sort_by_key(|r| r.airing_at);
    Ok(rows)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::store::Db;

    fn body_for_id(id: i64, title: &str, episode: i64, airs_at: i64) -> String {
        format!(
            r#"{{"data": {{ "Media": {{
                "id": {id},
                "title": {{"romaji": "{title}", "english": "{title}", "native": "{title}"}},
                "status": "RELEASING", "episodes": null, "format": "TV",
                "nextAiringEpisode": {{"episode": {episode}, "airingAt": {airs_at}}}
            }} }} }}"#
        )
    }

    #[tokio::test]
    async fn followed_mode_uses_fresh_cache_without_calling_anilist() {
        // Mockito with expect(0) means a network call would be a test failure.
        let mut server = mockito::Server::new_async().await;
        let _m = server
            .mock("POST", "/")
            .with_status(500)
            .expect(0)
            .create_async()
            .await;
        let client = AniListClient::with_base_url(server.url());

        let mut db = Db::open_in_memory().unwrap();
        db.add_follow("anilist", "21", "anime", "One Piece", 100).unwrap();
        // Pre-populate fresh cache (expires_at well in the future).
        db.upsert_cache(&CacheEntry {
            source: "anilist".into(),
            source_id: "21".into(),
            display_title: Some("One Piece".into()),
            title_english: Some("One Piece".into()),
            title_native: Some("ワンピース".into()),
            status: Some("RELEASING".into()),
            total_episodes: None,
            format: Some("TV".into()),
            next_episode_number: Some(1100),
            next_episode_airs_at: Some(1_000 + 3600),
            fetched_at: 1_000,
            expires_at: 1_000_000,
        })
        .unwrap();

        let rows = schedule_followed(&mut db, &client, 0, 2_000_000, 1_000)
            .await
            .unwrap();
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].episode, 1100);
    }

    #[tokio::test]
    async fn followed_mode_refreshes_stale_cache_via_by_id() {
        let mut server = mockito::Server::new_async().await;
        let _m = server
            .mock("POST", "/")
            .with_status(200)
            .with_body(body_for_id(21, "One Piece", 1100, 1_500))
            .expect(1)
            .create_async()
            .await;
        let client = AniListClient::with_base_url(server.url());

        let mut db = Db::open_in_memory().unwrap();
        db.add_follow("anilist", "21", "anime", "One Piece", 100).unwrap();
        // Stale cache: expires_at < now.
        db.upsert_cache(&CacheEntry {
            source: "anilist".into(),
            source_id: "21".into(),
            display_title: Some("One Piece".into()),
            title_english: None,
            title_native: None,
            status: Some("RELEASING".into()),
            total_episodes: None,
            format: None,
            next_episode_number: Some(999),
            next_episode_airs_at: Some(500),
            fetched_at: 0,
            expires_at: 100,
        })
        .unwrap();

        let rows = schedule_followed(&mut db, &client, 0, 2_000, 1_000)
            .await
            .unwrap();
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].episode, 1100, "should use refreshed value");
        let refreshed = db.get_cache("anilist", "21").unwrap().unwrap();
        assert_eq!(refreshed.next_episode_airs_at, Some(1500));
    }

    #[tokio::test]
    async fn followed_mode_filters_to_time_window() {
        let mut server = mockito::Server::new_async().await;
        let _m = server
            .mock("POST", "/")
            .with_status(500)
            .expect(0)
            .create_async()
            .await;
        let client = AniListClient::with_base_url(server.url());

        let mut db = Db::open_in_memory().unwrap();
        db.add_follow("anilist", "21", "anime", "Inside", 100).unwrap();
        db.add_follow("anilist", "22", "anime", "Outside", 100).unwrap();

        db.upsert_cache(&CacheEntry {
            source: "anilist".into(),
            source_id: "21".into(),
            display_title: Some("Inside".into()),
            title_english: None,
            title_native: None,
            status: Some("RELEASING".into()),
            total_episodes: None,
            format: None,
            next_episode_number: Some(5),
            next_episode_airs_at: Some(150),
            fetched_at: 0,
            expires_at: 100_000,
        })
        .unwrap();
        db.upsert_cache(&CacheEntry {
            source: "anilist".into(),
            source_id: "22".into(),
            display_title: Some("Outside".into()),
            title_english: None,
            title_native: None,
            status: Some("RELEASING".into()),
            total_episodes: None,
            format: None,
            next_episode_number: Some(7),
            next_episode_airs_at: Some(900),
            fetched_at: 0,
            expires_at: 100_000,
        })
        .unwrap();

        let rows = schedule_followed(&mut db, &client, 100, 200, 50).await.unwrap();
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].title, "Inside");
    }

    #[tokio::test]
    async fn all_mode_warms_cache_with_global_results() {
        let mut server = mockito::Server::new_async().await;
        let body = r#"{
            "data": {"Page": {"airingSchedules": [
                {"airingAt": 1700, "episode": 1100,
                 "media": {"id": 21, "title": {"romaji": "ONE PIECE", "english": "One Piece", "native": "ワンピース"},
                           "status": "RELEASING", "episodes": null, "format": "TV", "nextAiringEpisode": null}},
                {"airingAt": 1800, "episode": 5,
                 "media": {"id": 99, "title": {"romaji": "X", "english": "X", "native": "X"},
                           "status": "RELEASING", "episodes": null, "format": "TV", "nextAiringEpisode": null}}
            ]}}
        }"#;
        let _m = server
            .mock("POST", "/")
            .with_status(200)
            .with_body(body)
            .create_async()
            .await;
        let client = AniListClient::with_base_url(server.url());

        let db = Db::open_in_memory().unwrap();
        let rows = schedule_all(&client, &db, 0, 10_000, 1_000).await.unwrap();
        assert_eq!(rows.len(), 2);
        // The corpus is now warm — picker should hit the local index.
        let hits = db.search_fuzzy("one", 10).unwrap();
        assert!(hits.iter().any(|h| h.source_id == "21"));
        let hits = db.search_fuzzy("x", 10).unwrap();
        assert!(hits.iter().any(|h| h.source_id == "99"));
    }
}
