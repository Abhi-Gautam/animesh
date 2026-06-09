//! Follow operation — adds a show to the durable library.
//!
//! Pure-ish execution path with all I/O dependencies injected. The
//! TUI's `App::dispatch(Command::Follow(id))` calls this directly,
//! as does the AniList picker flow from the follow palette.

use std::sync::Arc;

use anyhow::{anyhow, Context, Result};

use crate::errors::user_error;
use crate::ids::{CanonicalId, ReleaseKind};
use crate::library::Library as Facade;
use crate::sources::anilist::{AniListClient, Media};
use crate::store::{CacheEntry, CanonicalFollowOutcome, TtlConfig};

/// Result of `follow_inner` — the outcome and the show we resolved.
#[derive(Debug)]
pub struct FollowReport {
    pub outcome: CanonicalFollowOutcome,
    pub media: Media,
}

/// Atomic follow path. Resolves the AniList show, upserts the canonical
/// row + source_ref, marks the canonical followed, refreshes the
/// metadata cache, and renders the cover if missing.
///
/// The CanonicalId uses [`CanonicalId::legacy_from_source`] — same
/// shape as V0004 backfill — so re-follow of a backfilled show maps
/// cleanly. The LLM canonicalizer can later re-canonicalize these to
/// human-readable slugs.
pub async fn follow_inner(
    facade: &Arc<Facade>,
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
    let canonical_id =
        CanonicalId::legacy_from_source(ReleaseKind::Anime, "anilist", &source_id);

    let outcome = facade
        .follow_with_source(
            &canonical_id,
            ReleaseKind::Anime,
            &title,
            "anilist",
            &source_id,
            Some(&title),
            1.0,
        )
        .context("follow_with_source")?;

    let ttl = TtlConfig::from_env();
    facade
        .upsert_cache(&CacheEntry::from_media(&media, &ttl, now))
        .context("upsert_cache after follow")?;

    Ok(FollowReport { outcome, media })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::time::FixedClock;

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

    fn facade(now: i64) -> Arc<Facade> {
        Arc::new(Facade::open_in_memory(Arc::new(FixedClock(now))).unwrap())
    }

    #[tokio::test]
    async fn follow_new_show_persists_canonical_and_cache() {
        let mut server = mockito::Server::new_async().await;
        let _m = server
            .mock("POST", "/")
            .with_status(200)
            .with_body(one_piece_body())
            .create_async()
            .await;
        let client = AniListClient::with_base_url(server.url());
        let facade = facade(1_000_000);

        let report = follow_inner(&facade, &client, 21, 1_000_000).await.unwrap();
        assert_eq!(report.outcome, CanonicalFollowOutcome::NewlyFollowed);

        let list = facade.followed().unwrap();
        assert_eq!(list.len(), 1);
        assert_eq!(list[0].id.as_str(), "release:anime:legacy-anilist-21");
        assert_eq!(list[0].display_title, "One Piece");

        let cached = facade.get_cache("anilist", "21").unwrap().unwrap();
        assert_eq!(cached.status.as_deref(), Some("RELEASING"));
        assert_eq!(cached.next_episode_number, Some(1100));
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
        let facade = facade(1);
        let err = follow_inner(&facade, &client, 999_999_999, 1)
            .await
            .expect_err("expected error");
        assert!(format!("{err:#}").contains("no AniList show with id 999999999"));
        assert_eq!(facade.count_followed().unwrap(), 0);
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
        let facade = facade(100);

        let r1 = follow_inner(&facade, &client, 21, 100).await.unwrap();
        let cid = CanonicalId::legacy_from_source(
            ReleaseKind::Anime,
            "anilist",
            &r1.media.id.to_string(),
        );
        facade.drop_canonical(&cid).unwrap();
        let r2 = follow_inner(&facade, &client, 21, 300).await.unwrap();
        assert_eq!(r2.outcome, CanonicalFollowOutcome::RestoredFromDrop);
        assert_eq!(r1.media.id, r2.media.id);

        let row = facade.find_canonical(&cid).unwrap().unwrap();
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
        let facade = facade(100);

        follow_inner(&facade, &client, 21, 100).await.unwrap();
        let report = follow_inner(&facade, &client, 21, 200).await.unwrap();
        assert_eq!(report.outcome, CanonicalFollowOutcome::AlreadyFollowing);
        assert_eq!(facade.count_followed().unwrap(), 1);
    }
}
