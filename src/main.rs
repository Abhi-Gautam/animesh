//! animesh — AniList search, schedule, and local watchlist.
//!
//! Commands return object models; this binary is the only place that prints.
//!
//! Usage:
//!   cargo run -- search "one piece"
//!   cargo run -- schedule 21
//!   cargo run -- watchlist 21

mod commands;
mod db;
mod models;
mod sources;

use anyhow::Result;

use models::{AnimeDetail, SearchHit, WatchlistMutation};

fn usage() -> ! {
    eprintln!(
        "\
animesh — personal anime release radar (CLI slice)

Usage:
  animesh search <query>     Search AniList; print candidate list with ids
  animesh schedule <id>      Next airing episode for an AniList media id
  animesh watchlist <id>     Add/update anime on the local Turso watchlist

Examples:
  animesh search \"one piece\"
  animesh schedule 21
  animesh watchlist 21"
    );
    std::process::exit(1);
}

#[tokio::main]
async fn main() -> Result<()> {
    let mut args = std::env::args().skip(1);
    let cmd = match args.next() {
        Some(c) => c,
        None => usage(),
    };

    match cmd.as_str() {
        "search" => {
            let query = args.next().unwrap_or_else(|| usage());
            if query.trim().is_empty() {
                usage();
            }
            let hits = commands::search::run(&query).await?;
            print_search(&query, &hits);
            Ok(())
        }
        "schedule" => {
            let id = args.next().unwrap_or_else(|| usage());
            let detail = commands::schedule::run_str(&id).await?;
            print_detail(&detail);
            Ok(())
        }
        "watchlist" => {
            let id = args.next().unwrap_or_else(|| usage());
            let mutation = commands::watchlist::run_str(&id).await?;
            print_watchlist(&mutation);
            Ok(())
        }
        "-h" | "--help" | "help" => usage(),
        other => {
            eprintln!("unknown command: {other}\n");
            usage();
        }
    }
}

// ---------------------------------------------------------------------------
// Presentation only (CLI surface). Domain code never prints.
// ---------------------------------------------------------------------------

fn print_search(query: &str, hits: &[SearchHit]) {
    if hits.is_empty() {
        println!("No results for {query:?}.");
        return;
    }

    println!("Search results for {query:?}:\n");
    println!(
        "{:>3}  {:>6}  {:<48}  {:<12}  {:<8}  {:>4}  {:>4}",
        "#", "ID", "TITLE", "STATUS", "FORMAT", "EPS", "YEAR"
    );
    println!("{}", "-".repeat(96));

    for (i, m) in hits.iter().enumerate() {
        println!(
            "{:>3}  {:>6}  {:<48}  {:<12}  {:<8}  {:>4}  {:>4}",
            i + 1,
            m.id,
            truncate(&m.title, 48),
            m.status.as_deref().unwrap_or("?"),
            m.format.as_deref().unwrap_or("?"),
            opt_num(m.episodes),
            opt_num(m.season_year),
        );
    }

    println!("\nNext: animesh schedule <ID>");
}

fn print_detail(d: &AnimeDetail) {
    let h = &d.hit;
    println!("[{}] {}", h.id, h.title);
    println!("status   {}", h.status.as_deref().unwrap_or("?"));
    println!("format   {}", h.format.as_deref().unwrap_or("?"));
    println!("episodes {}", opt_num(h.episodes));
    match d.next {
        Some(n) => {
            println!("next     ep.{}", n.episode);
            println!("airs     {}", format_ts(n.airing_at));
            println!(
                "in       {}",
                format_duration(n.time_until_airing, n.airing_at)
            );
        }
        None => {
            println!("next     —");
            println!(
                "reason   {}",
                no_next_reason(h.status.as_deref().unwrap_or(""))
            );
        }
    }
}

fn print_watchlist(m: &WatchlistMutation) {
    let action = if m.inserted { "added" } else { "updated" };
    let h = &m.detail.hit;
    println!("{action} [{}] {}", h.id, h.title);
    println!("status   {}", h.status.as_deref().unwrap_or("?"));
    println!("format   {}", h.format.as_deref().unwrap_or("?"));
    println!("episodes {}", opt_num(h.episodes));
    match m.detail.next {
        Some(n) => {
            println!("next     ep.{}", n.episode);
            println!("airs     {}", format_ts(n.airing_at));
        }
        None => println!("next     —"),
    }
}

fn opt_num(v: Option<i64>) -> String {
    v.map(|n| n.to_string()).unwrap_or_else(|| "?".into())
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

fn format_ts(unix_secs: i64) -> String {
    match chrono::DateTime::from_timestamp(unix_secs.max(0), 0) {
        Some(dt) => dt.format("%Y-%m-%d %H:%M UTC").to_string(),
        None => "?".into(),
    }
}

fn format_duration(time_until: Option<i64>, airing_at: i64) -> String {
    let secs = time_until.unwrap_or_else(|| airing_at - chrono::Utc::now().timestamp());
    if secs <= 0 {
        return "now / aired".into();
    }
    let days = secs / 86_400;
    let hours = (secs % 86_400) / 3_600;
    let mins = (secs % 3_600) / 60;
    if days > 0 {
        format!("{days}d {hours}h")
    } else if hours > 0 {
        format!("{hours}h {mins}m")
    } else {
        format!("{mins}m")
    }
}

fn no_next_reason(status: &str) -> &'static str {
    match status {
        "FINISHED" => "series finished",
        "NOT_YET_RELEASED" => "not yet scheduled",
        "HIATUS" => "on hiatus; no episode scheduled",
        "CANCELLED" => "cancelled",
        "RELEASING" => "no upcoming episode scheduled",
        _ => "no upcoming episode scheduled",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reasons_match_status() {
        assert_eq!(no_next_reason("FINISHED"), "series finished");
        assert_eq!(no_next_reason("NOT_YET_RELEASED"), "not yet scheduled");
    }
}
