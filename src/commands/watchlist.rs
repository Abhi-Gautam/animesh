//! `watchlist` — resolve schedule detail, compose entry, upsert.
//! Returns object models only (no printing).

use anyhow::{Context, Result};

use crate::commands::schedule;
use crate::db::{self, watchlist};
use crate::models::WatchlistMutation;

/// Fetch schedule detail for `id`, upsert into local watchlist, return mutation.
pub(crate) async fn run(id: i64) -> Result<WatchlistMutation> {
    let detail = schedule::run(id).await.context("resolve schedule detail")?;
    let entry = watchlist::Entry::from(&detail);

    let conn = db::open_default().await.context("open local database")?;
    let inserted = watchlist::upsert(&conn, &entry)
        .await
        .context("write watchlist")?;

    Ok(WatchlistMutation { inserted, detail })
}

/// Parse a raw CLI id string, then [`run`].
pub(crate) async fn run_str(id_raw: &str) -> Result<WatchlistMutation> {
    let id: i64 = id_raw
        .parse()
        .context("watchlist expects a numeric AniList id")?;
    run(id).await
}
