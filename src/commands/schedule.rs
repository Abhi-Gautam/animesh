//! `schedule` — next airing episode for a single AniList media id.

use anyhow::{bail, Context, Result};

use crate::sources::AniListClient;

/// Run `animesh schedule <anilist_id>`.
pub(crate) async fn run(id_raw: &str) -> Result<()> {
    let id: i64 = id_raw
        .parse()
        .context("schedule expects a numeric AniList id")?;
    if id <= 0 {
        bail!("AniList id must be a positive integer, got {id}");
    }

    let client = AniListClient::new();
    let media = client
        .media(id)
        .await
        .context("fetch AniList media")?
        .with_context(|| format!("No AniList anime with id {id}"))?;

    let status = media.status.as_deref().unwrap_or("?");
    let format = media.format.as_deref().unwrap_or("?");
    let episodes = media
        .episodes
        .map(|n| n.to_string())
        .unwrap_or_else(|| "?".into());

    println!("[{}] {}", media.id, media.display_title());
    println!("status   {status}");
    println!("format   {format}");
    println!("episodes {episodes}");

    match media.next_airing_episode {
        Some(next) => {
            println!("next     ep.{}", next.episode);
            println!("airs     {}", format_ts(next.airing_at));
            println!(
                "in       {}",
                format_duration(next.time_until_airing, next.airing_at)
            );
        }
        None => {
            println!("next     —");
            println!("reason   {}", no_next_reason(status));
        }
    }

    Ok(())
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

fn format_ts(unix_secs: i64) -> String {
    match chrono::DateTime::from_timestamp(unix_secs.max(0), 0) {
        Some(dt) => dt.format("%Y-%m-%d %H:%M UTC").to_string(),
        None => "?".into(),
    }
}

/// Prefer AniList `timeUntilAiring`; fall back to wall-clock delta from `airing_at`.
fn format_duration(time_until: Option<i64>, airing_at: i64) -> String {
    let secs = time_until.unwrap_or_else(|| {
        let now = chrono::Utc::now().timestamp();
        airing_at - now
    });

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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reasons_match_status() {
        assert_eq!(no_next_reason("FINISHED"), "series finished");
        assert_eq!(no_next_reason("NOT_YET_RELEASED"), "not yet scheduled");
        assert_eq!(no_next_reason("RELEASING"), "no upcoming episode scheduled");
    }

    #[test]
    fn duration_formats_days_hours() {
        assert_eq!(format_duration(Some(2 * 86_400 + 3 * 3_600), 0), "2d 3h");
        assert_eq!(format_duration(Some(90 * 60), 0), "1h 30m");
        assert_eq!(format_duration(Some(45), 0), "0m");
        assert_eq!(format_duration(Some(0), 0), "now / aired");
    }
}
