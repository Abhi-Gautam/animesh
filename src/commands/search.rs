//! `search` — free-text AniList discovery. Returns object models only.

use anyhow::{Context, Result};

use crate::models::SearchHit;
use crate::sources::AniListClient;

const PER_PAGE: u32 = 10;

/// Search AniList and return structured hits (no I/O printing).
pub(crate) async fn run(query: &str) -> Result<Vec<SearchHit>> {
    let client = AniListClient::new();
    let results = client
        .search(query, PER_PAGE)
        .await
        .context("search AniList")?;
    Ok(results.into_iter().map(SearchHit::from).collect())
}
