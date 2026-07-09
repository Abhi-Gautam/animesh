//! `schedule` — next airing detail for one AniList id. Returns object models only.

use anyhow::{bail, Context, Result};

use crate::models::AnimeDetail;
use crate::sources::AniListClient;

/// Fetch anime + next airing by AniList id (no I/O printing).
pub(crate) async fn run(id: i64) -> Result<AnimeDetail> {
    if id <= 0 {
        bail!("AniList id must be a positive integer, got {id}");
    }

    let client = AniListClient::new();
    let media = client
        .media(id)
        .await
        .context("fetch AniList media")?
        .with_context(|| format!("No AniList anime with id {id}"))?;

    Ok(AnimeDetail::from(media))
}

/// Parse a raw CLI id string, then [`run`].
pub(crate) async fn run_str(id_raw: &str) -> Result<AnimeDetail> {
    let id: i64 = id_raw
        .parse()
        .context("schedule expects a numeric AniList id")?;
    run(id).await
}
