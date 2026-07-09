//! `search` — find anime by free-text query and list candidates.

use anyhow::{Context, Result};

use crate::sources::AniListClient;

const PER_PAGE: u32 = 10;

/// Run `animesh search <query>`.
pub(crate) async fn run(query: &str) -> Result<()> {
    let client = AniListClient::new();
    let results = client
        .search(query, PER_PAGE)
        .await
        .context("search AniList")?;

    if results.is_empty() {
        println!("No results for {query:?}.");
        return Ok(());
    }

    println!("Search results for {query:?}:\n");
    println!(
        "{:>3}  {:>6}  {:<48}  {:<12}  {:<8}  {:>4}  {:>4}",
        "#", "ID", "TITLE", "STATUS", "FORMAT", "EPS", "YEAR"
    );
    println!("{}", "-".repeat(96));

    for (i, m) in results.iter().enumerate() {
        let status = m.status.as_deref().unwrap_or("?");
        let format = m.format.as_deref().unwrap_or("?");
        let eps = m
            .episodes
            .map(|n| n.to_string())
            .unwrap_or_else(|| "?".into());
        let year = m
            .season_year
            .map(|n| n.to_string())
            .unwrap_or_else(|| "?".into());

        println!(
            "{:>3}  {:>6}  {:<48}  {:<12}  {:<8}  {:>4}  {:>4}",
            i + 1,
            m.id,
            truncate(m.display_title(), 48),
            status,
            format,
            eps,
            year,
        );
    }

    println!("\nNext: animesh schedule <ID>");
    Ok(())
}

fn truncate(s: &str, max_chars: usize) -> String {
    let count = s.chars().count();
    if count <= max_chars {
        return s.to_string();
    }
    let mut marked: String = s.chars().take(max_chars.saturating_sub(1)).collect();
    marked.push('…');
    marked
}
