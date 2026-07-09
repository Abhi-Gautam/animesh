//! animesh — search anime on AniList from the command line.
//!
//! Usage:
//!   cargo run -- "one piece"
//!   cargo run -- "hunter x hunter"

mod sources;

use anyhow::{Context, Result};
use sources::AniListClient;

#[tokio::main]
async fn main() -> Result<()> {
    let query = std::env::args().nth(1).unwrap_or_else(|| {
        eprintln!("usage: animesh <search query>");
        std::process::exit(1);
    });

    let client = AniListClient::new();
    let results = client.search(&query, 10).await.context("search AniList")?;

    if results.is_empty() {
        println!("No results for {query:?}.");
        return Ok(());
    }

    println!("Results for {query:?}:\n");
    for (i, m) in results.iter().enumerate() {
        let status = m.status.as_deref().unwrap_or("?");
        let fmt = m.format.as_deref().unwrap_or("?");
        let next = match m.next_airing_episode {
            Some(e) => format!(" ep.{} @ {}", e.episode, format_ts(e.airing_at)),
            None => " — ".into(),
        };
        println!(
            "{:>3}. [{:>6}] {:50}  {:10}  {:12}  {}",
            i + 1,
            m.id,
            m.display_title(),
            status,
            fmt,
            next,
        );
    }

    Ok(())
}

fn format_ts(unix_secs: i64) -> String {
    let secs = if unix_secs >= 0 {
        unix_secs as u64
    } else {
        0_u64
    };
    match chrono::DateTime::from_timestamp(secs as i64, 0) {
        Some(dt) => dt.format("%Y-%m-%d %H:%M UTC").to_string(),
        None => "?".into(),
    }
}
