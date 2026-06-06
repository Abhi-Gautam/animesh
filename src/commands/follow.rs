//! `animesh follow` — adds a show to the durable library.
//!
//! v0.3 ships the scripted `--id N` path. The interactive query
//! flow (`animesh follow <query>`) lands with the picker module
//! (T19–T21).

use anyhow::{anyhow, Context, Result};
use async_trait::async_trait;
use chrono::Utc;

use crate::{
    anilist::{AniListClient, Media},
    commands::Command,
    errors::user_error,
    store::{resolve_db_path, CacheEntry, Db, FollowOutcome, TtlConfig},
};

pub struct FollowCommand {
    id: i64,
}

impl FollowCommand {
    pub fn new(id: i64) -> Self {
        Self { id }
    }
}

/// Result of `follow_inner` — the outcome and the show we resolved.
/// Returned for testability and for the caller to render.
#[derive(Debug)]
pub struct FollowReport {
    pub outcome: FollowOutcome,
    pub media: Media,
}

/// Pure-ish execution path with all I/O dependencies injected. Tests
/// build their own in-memory `Db` and a mockito-pointed `AniListClient`
/// and drive this directly.
pub async fn follow_inner(
    db: &mut Db,
    client: &AniListClient,
    id: i64,
    now: i64,
) -> Result<FollowReport> {
    let media = client
        .by_id(id)
        .await
        .context("AniList by_id")?
        .ok_or_else(|| user_error(anyhow!("no AniList show with id {id}")))?;
    let title = media.display_title().to_string();
    let source_id = media.id.to_string();

    let outcome = db
        .add_follow("anilist", &source_id, "anime", &title, now)
        .context("add_follow")?;

    let ttl = TtlConfig::from_env();
    db.upsert_cache(&CacheEntry::from_media(&media, &ttl, now))
        .context("upsert_cache after follow")?;

    Ok(FollowReport { outcome, media })
}

#[async_trait(?Send)]
impl Command for FollowCommand {
    async fn execute(&self) -> Result<()> {
        let path = resolve_db_path()?;
        let mut db = Db::open(&path)?;
        let client = AniListClient::new();
        let now = Utc::now().timestamp();
        let report = follow_inner(&mut db, &client, self.id, now).await?;
        let title = report.media.display_title();
        let id = report.media.id;
        let msg = match report.outcome {
            FollowOutcome::NewlyFollowed => format!("Followed: {title} (id {id})"),
            FollowOutcome::RestoredFromDrop => {
                format!("Re-followed (was dropped): {title} (id {id})")
            }
            FollowOutcome::AlreadyFollowing => format!("Already following: {title} (id {id})"),
        };
        println!("{msg}");
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::store::{Db, ListFilter};

    fn one_piece_body() -> &'static str {
        r#"{
            "data": { "Media": {
                "id": 21,
                "title": {"romaji": "ONE PIECE", "english": "One Piece", "native": "ワンピース"},
                "status": "RELEASING", "episodes": null, "format": "TV",
                "nextAiringEpisode": {"episode": 1100, "airingAt": 1700000000}
            }}
        }"#
    }

    #[tokio::test]
    async fn follow_new_show_persists_tracked_item_and_cache() {
        let mut server = mockito::Server::new_async().await;
        let _m = server
            .mock("POST", "/")
            .with_status(200)
            .with_body(one_piece_body())
            .create_async()
            .await;
        let client = AniListClient::with_base_url(server.url());
        let mut db = Db::open_in_memory().unwrap();

        let report = follow_inner(&mut db, &client, 21, 1_000_000).await.unwrap();
        assert_eq!(report.outcome, FollowOutcome::NewlyFollowed);

        let list = db.list_follows(ListFilter::Active).unwrap();
        assert_eq!(list.len(), 1);
        assert_eq!(list[0].source_id, "21");
        assert_eq!(list[0].display_title, "One Piece");

        let cached = db.get_cache("anilist", "21").unwrap().unwrap();
        assert_eq!(cached.status.as_deref(), Some("RELEASING"));
        assert_eq!(cached.next_episode_number, Some(1100));
        // Releasing TTL is 6h, so expires_at = fetched_at + 21600.
        assert_eq!(cached.expires_at - cached.fetched_at, 6 * 3600);
    }

    #[tokio::test]
    async fn follow_unknown_id_yields_user_error() {
        let mut server = mockito::Server::new_async().await;
        let _m = server
            .mock("POST", "/")
            .with_status(200)
            .with_body(r#"{"data": {"Media": null}}"#)
            .create_async()
            .await;
        let client = AniListClient::with_base_url(server.url());
        let mut db = Db::open_in_memory().unwrap();
        let err = follow_inner(&mut db, &client, 999_999_999, 1)
            .await
            .expect_err("expected error");
        assert!(format!("{err:#}").contains("no AniList show with id 999999999"));
        assert_eq!(db.count_active().unwrap(), 0);
    }

    #[tokio::test]
    async fn re_follow_dropped_show_restores_and_keeps_original_followed_at() {
        let mut server = mockito::Server::new_async().await;
        let _m = server
            .mock("POST", "/")
            .with_status(200)
            .with_body(one_piece_body())
            .expect_at_least(2)
            .create_async()
            .await;
        let client = AniListClient::with_base_url(server.url());
        let mut db = Db::open_in_memory().unwrap();

        follow_inner(&mut db, &client, 21, 100).await.unwrap();
        db.drop_follow("anilist", "21", 200).unwrap();
        let report = follow_inner(&mut db, &client, 21, 300).await.unwrap();
        assert_eq!(report.outcome, FollowOutcome::RestoredFromDrop);

        let row = db.find_by_source("anilist", "21").unwrap().unwrap();
        assert_eq!(row.followed_at, 100, "original followed_at preserved");
        assert!(row.dropped_at.is_none());
    }

    #[tokio::test]
    async fn follow_same_id_twice_reports_already_following() {
        let mut server = mockito::Server::new_async().await;
        let _m = server
            .mock("POST", "/")
            .with_status(200)
            .with_body(one_piece_body())
            .expect_at_least(2)
            .create_async()
            .await;
        let client = AniListClient::with_base_url(server.url());
        let mut db = Db::open_in_memory().unwrap();

        follow_inner(&mut db, &client, 21, 100).await.unwrap();
        let report = follow_inner(&mut db, &client, 21, 200).await.unwrap();
        assert_eq!(report.outcome, FollowOutcome::AlreadyFollowing);
        assert_eq!(db.count_active().unwrap(), 1);
    }
}
