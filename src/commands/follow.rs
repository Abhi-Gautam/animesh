//! Follow operation — adds a show to the durable library.
//!
//! Pure-ish execution path with all I/O dependencies injected. The
//! TUI's `App::dispatch(Command::Follow(id))` calls this directly.

use anyhow::{anyhow, Context, Result};

use crate::{
    anilist::{AniListClient, Media},
    errors::user_error,
    store::{CacheEntry, Db, FollowOutcome, TtlConfig},
};

/// Result of `follow_inner` — the outcome and the show we resolved.
#[derive(Debug)]
pub struct FollowReport {
    pub outcome: FollowOutcome,
    pub media: Media,
}

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

    // Render the cover art whenever the row is missing one. This covers
    // (a) brand-new follows, (b) re-follow after drop, (c) backfill of
    // rows that pre-date this feature, and (d) retry after an offline
    // first follow. Failures are silently swallowed.
    let needs_cover = db
        .find_by_source("anilist", &source_id)
        .ok()
        .flatten()
        .map(|row| row.cover_ascii.is_none())
        .unwrap_or(false);
    if needs_cover {
        if let Some(url) = media.cover_url() {
            if let Ok(ascii) = fetch_and_render_cover(url).await {
                let color = media
                    .cover_image
                    .as_ref()
                    .and_then(|c| c.color.as_deref());
                let _ = db.set_cover_ascii("anilist", &source_id, &ascii, color);
            }
        }
    }

    Ok(FollowReport { outcome, media })
}

/// Cover render target. Sized for the ~33%-wide detail pane on a
/// standard 80-col terminal; scales gracefully on wider terminals via
/// the Paragraph widget's natural left-alignment.
const COVER_COLS: u32 = 14;
const COVER_ROWS: u32 = 7;

async fn fetch_and_render_cover(url: &str) -> Result<String> {
    let bytes = reqwest::get(url)
        .await
        .context("fetch cover image")?
        .bytes()
        .await
        .context("read cover bytes")?;
    crate::tui::ascii_art::render_ascii(&bytes, COVER_COLS, COVER_ROWS)
}

/// Backfill cover art for every active follow whose stored ASCII is
/// either missing or in a stale format (no `█` glyph = produced by an
/// older renderer version). Run once at TUI startup so a code-side
/// change to the renderer auto-refreshes existing rows without the
/// user having to drop+re-follow.
///
/// Returns the count of rows refreshed. Per-row failures (offline,
/// missing cover URL in cache) are silently skipped — the row stays
/// NULL and will be retried on next launch.
pub async fn refresh_stale_covers(db: &Db, client: &crate::anilist::AniListClient) -> usize {
    use crate::store::ListFilter;
    let Ok(items) = db.list_follows(ListFilter::Active) else {
        return 0;
    };
    let mut refreshed = 0usize;
    for item in items {
        let is_stale = match item.cover_ascii.as_deref() {
            None => true,
            Some(s) => !s.starts_with(crate::tui::ascii_art::FORMAT_TAG),
        };
        if !is_stale {
            continue;
        }
        // Prefer the cached URL; only call AniList by_id as a fallback.
        let url_from_cache = db
            .get_cache(&item.source, &item.source_id)
            .ok()
            .flatten()
            .and_then(|c| c.cover_image_url);
        let (url_opt, color_opt) = if let Some(u) = url_from_cache {
            (Some(u), None)
        } else {
            match item.source_id.parse::<i64>() {
                Ok(id) => match client.by_id(id).await {
                    Ok(Some(media)) => {
                        let url = media.cover_url().map(str::to_string);
                        let color = media
                            .cover_image
                            .as_ref()
                            .and_then(|c| c.color.clone());
                        (url, color)
                    }
                    _ => (None, None),
                },
                Err(_) => (None, None),
            }
        };
        let Some(url) = url_opt else { continue };
        if let Ok(ascii) = fetch_and_render_cover(&url).await {
            if db
                .set_cover_ascii(&item.source, &item.source_id, &ascii, color_opt.as_deref())
                .is_ok()
            {
                refreshed += 1;
            }
        }
    }
    refreshed
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
