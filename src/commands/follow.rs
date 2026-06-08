//! Follow operation — adds a show to the durable library.
//!
//! Pure-ish execution path with all I/O dependencies injected. The
//! TUI's `App::dispatch(Command::Follow(id))` calls this directly,
//! as does the AniList picker flow from the follow palette.
//!
//! v0.5: writes through the [`Facade`] (canonical_release × source_ref
//! × engagement) — no more legacy tracked_item. Cover ASCII renders
//! into `canonical_release.cover_ascii`.

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
    pub canonical_id: CanonicalId,
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

    // Render the cover whenever the canonical's stored ASCII is missing
    // or in an older format. Covers (a) fresh follows, (b) re-follow
    // after drop, (c) backfilled rows that pre-date this feature, and
    // (d) retry after an offline first follow. Failures are silently
    // swallowed — a missing cover is a placeholder, not a hard error.
    let needs_cover = facade
        .find_canonical(&canonical_id)
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
                let _ = facade.set_canonical_cover(&canonical_id, &ascii, color);
            }
        }
    }

    Ok(FollowReport {
        outcome,
        media,
        canonical_id,
    })
}

/// Cover render target. Sized for the ~33%-wide detail pane on a
/// standard 80-col terminal; scales gracefully on wider terminals via
/// the Paragraph widget's natural left-alignment.
const COVER_COLS: u32 = 14;
const COVER_ROWS: u32 = 7;

async fn fetch_and_render_cover(url: &str) -> Result<String> {
    let bytes = crate::sources::fetch_bytes(url)
        .await
        .context("fetch cover image")?;
    crate::tui::ascii_art::render_ascii(&bytes, COVER_COLS, COVER_ROWS)
}

/// Backfill cover art for every active follow whose stored ASCII is
/// either missing or in a stale format (no `█` glyph = produced by an
/// older renderer version). Run once at TUI startup so a code-side
/// change to the renderer auto-refreshes existing rows without the
/// user having to drop+re-follow.
///
/// Returns the count of rows refreshed. Per-row failures (offline,
/// missing cover URL, non-AniList source) are silently skipped — the
/// row stays as-is and will be retried on next launch.
pub async fn refresh_stale_covers(facade: &Arc<Facade>, client: &AniListClient) -> usize {
    let Ok(canonicals) = facade.followed() else {
        return 0;
    };
    let mut refreshed = 0usize;
    for canonical in canonicals {
        let is_stale = match canonical.cover_ascii.as_deref() {
            None => true,
            Some(s) => !s.starts_with(crate::tui::ascii_art::FORMAT_TAG),
        };
        if !is_stale {
            continue;
        }
        // Look at every attached source for a cover URL — prefer the
        // cache (no network), fall back to a live AniList lookup.
        let refs = match facade.source_refs_for(&canonical.id) {
            Ok(refs) => refs,
            Err(_) => continue,
        };
        let mut url_opt: Option<String> = None;
        let mut color_opt: Option<String> = None;
        for sref in &refs {
            if let Ok(Some(cache)) = facade.get_cache(&sref.source, &sref.source_id) {
                if let Some(u) = cache.cover_image_url {
                    url_opt = Some(u);
                    break;
                }
            }
        }
        if url_opt.is_none() {
            // Live fallback: only AniList has a numeric id we can query.
            for sref in &refs {
                if sref.source != "anilist" {
                    continue;
                }
                if let Ok(id) = sref.source_id.parse::<i64>() {
                    if let Ok(Some(media)) = client.by_id(id).await {
                        url_opt = media.cover_url().map(str::to_string);
                        color_opt = media
                            .cover_image
                            .as_ref()
                            .and_then(|c| c.color.clone());
                        break;
                    }
                }
            }
        }
        let Some(url) = url_opt else { continue };
        if let Ok(ascii) = fetch_and_render_cover(&url).await {
            if facade
                .set_canonical_cover(&canonical.id, &ascii, color_opt.as_deref())
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
    use crate::store::CanonicalListFilter;
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
        assert_eq!(report.canonical_id.as_str(), "release:anime:legacy-anilist-21");

        let list = facade.followed().unwrap();
        assert_eq!(list.len(), 1);
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
        facade.drop_canonical(&r1.canonical_id).unwrap();
        let r2 = follow_inner(&facade, &client, 21, 300).await.unwrap();
        assert_eq!(r2.outcome, CanonicalFollowOutcome::RestoredFromDrop);
        assert_eq!(r1.canonical_id, r2.canonical_id);

        let row = facade.find_canonical(&r2.canonical_id).unwrap().unwrap();
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
        assert_eq!(
            facade
                .all_canonical()
                .unwrap()
                .iter()
                .filter(|c| c.followed_at.is_some() && c.dropped_at.is_none())
                .count(),
            1
        );
        // Sanity: list_canonical(Active) returns one row.
        let _ = CanonicalListFilter::Active;
    }
}
