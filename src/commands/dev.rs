//! `dev-airing` — dev-only harness to drive the notifier daemon during manual
//! testing. NOT part of the product surface.
//!
//! Real AniList airings are days away, so they can't exercise the daemon in a
//! tight loop. This upserts a watchlist row with a caller-chosen episode and an
//! airing `secs_from_now` seconds ahead, through a CDC-enabled writer so the
//! daemon observes the change. Re-running with a new episode/offset simulates
//! the sync flow (episode advance, schedule shift).

use anyhow::{Context, Result};
use turso::Connection;

use crate::db::watchlist::{self, Entry};
use crate::models::{NextAiring, SearchHit};

/// Upsert a synthetic watchlist airing for `id`, `episode`, airing at
/// `now + secs_from_now`. Returns the resulting entry's airing timestamp.
pub(crate) async fn airing(
    conn: &Connection,
    id: i64,
    episode: i64,
    secs_from_now: i64,
) -> Result<i64> {
    let airing_at = chrono::Utc::now().timestamp() + secs_from_now;

    let entry = Entry {
        hit: SearchHit {
            id,
            title: format!("dev-{id}"),
            title_english: None,
            title_romaji: None,
            title_native: None,
            status: Some("RELEASING".into()),
            format: Some("TV".into()),
            episodes: None,
            season_year: None,
        },
        next: Some(NextAiring {
            episode,
            airing_at,
            time_until_airing: None,
        }),
    };

    watchlist::upsert(conn, &entry)
        .await
        .context("dev-airing upsert")?;

    Ok(airing_at)
}
