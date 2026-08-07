//! Human-readable output.
//!
//! Stdout is data; stderr is diagnostics. Nothing here formats for machines —
//! Plan 001 ships no stable JSON surface, and pretending otherwise would freeze
//! a contract nobody has designed.

use chrono::{Local, TimeZone};

use crate::domain::ids::UnixTimestamp;
use crate::domain::media::SearchCandidate;
use crate::domain::read_models::{
    FollowResult, FollowSummary, Freshness, HealthSnapshot, UpcomingRelease,
};

/// Formats an absolute instant in the viewer's local timezone.
pub fn local_time(at: UnixTimestamp) -> String {
    match Local.timestamp_opt(at.get(), 0).single() {
        Some(local) => local.format("%a %d %b %Y %H:%M").to_string(),
        // Ambiguous or nonexistent local times happen at DST boundaries; the
        // UTC instant is still true and still useful.
        None => format!("{} (UTC)", at.to_utc().format("%a %d %b %Y %H:%M")),
    }
}

/// A short relative duration, derived from the instant and the clock rather
/// than from anything the source told us.
pub fn relative(from: UnixTimestamp, to: UnixTimestamp) -> String {
    let seconds = from.seconds_until(to);
    if seconds <= 0 {
        return "now".to_owned();
    }
    let minutes = seconds / 60;
    let hours = minutes / 60;
    let days = hours / 24;

    if days > 0 {
        format!("in {days}d {}h", hours % 24)
    } else if hours > 0 {
        format!("in {hours}h {}m", minutes % 60)
    } else if minutes > 0 {
        format!("in {minutes}m")
    } else {
        "in under a minute".to_owned()
    }
}

fn freshness_note(freshness: Freshness) -> &'static str {
    match freshness {
        Freshness::Fresh => "",
        Freshness::Stale => "  (stale)",
        Freshness::BackingOff => "  (retrying)",
    }
}

pub fn upcoming(rows: &[UpcomingRelease], now: UnixTimestamp) -> String {
    if rows.is_empty() {
        return "Nothing scheduled.\n\nFind something with 'animesh search <query>', \
                then follow it with 'animesh follow <id>'."
            .to_owned();
    }

    let mut out = String::new();
    for row in rows {
        let episode = row
            .episode
            .map_or_else(|| "?".to_owned(), |e| e.get().to_string());
        out.push_str(&format!(
            "{}\n  Episode {}  {}  {}{}\n",
            row.display_title,
            episode,
            local_time(row.scheduled_at),
            relative(now, row.scheduled_at),
            freshness_note(row.freshness),
        ));
    }
    out.trim_end().to_owned()
}

pub fn search(candidates: &[SearchCandidate]) -> String {
    if candidates.is_empty() {
        return "No matches.".to_owned();
    }

    let mut out = String::new();
    for candidate in candidates {
        let year = candidate
            .season_year
            .map_or_else(String::new, |y| format!(" ({y})"));
        let episodes = candidate
            .episode_count
            .map_or_else(String::new, |e| format!(", {e} episodes"));
        out.push_str(&format!(
            "{:>9}  {}{}\n           {}{}\n",
            candidate.anilist_id,
            candidate.display_title,
            year,
            candidate.status.as_str(),
            episodes,
        ));
    }
    out.trim_end().to_owned()
}

pub fn follows(summaries: &[FollowSummary], now: UnixTimestamp) -> String {
    if summaries.is_empty() {
        return "Not following anything yet.".to_owned();
    }

    let mut out = String::new();
    for summary in summaries {
        let next = summary.upcoming.as_ref().map_or_else(
            || "  no scheduled episode".to_owned(),
            |u| {
                format!(
                    "  Episode {}  {}",
                    u.episode
                        .map_or_else(|| "?".to_owned(), |e| e.get().to_string()),
                    relative(now, u.scheduled_at)
                )
            },
        );
        out.push_str(&format!(
            "{:>9}  {}\n{}{}\n",
            summary.media_id,
            summary.display_title,
            next,
            freshness_note(summary.freshness),
        ));
    }
    out.trim_end().to_owned()
}

pub fn follow_result(result: &FollowResult, now: UnixTimestamp) -> String {
    let verb = match result.outcome {
        crate::domain::read_models::FollowOutcome::NewlyFollowed => "Now following",
        crate::domain::read_models::FollowOutcome::Reactivated => "Following again",
        crate::domain::read_models::FollowOutcome::AlreadyActive => "Already following",
    };

    let next = result.upcoming.as_ref().map_or_else(
        || "  No scheduled episode yet.".to_owned(),
        |u| {
            format!(
                "  Episode {} airs {} ({}).",
                u.episode
                    .map_or_else(|| "?".to_owned(), |e| e.get().to_string()),
                local_time(u.scheduled_at),
                relative(now, u.scheduled_at)
            )
        },
    );

    format!(
        "{verb}: {} (media {})\n{next}",
        result.display_title, result.media_id
    )
}

pub fn health(snapshot: &HealthSnapshot, now: UnixTimestamp) -> String {
    let mut out = format!(
        "Animesh {} (schema {}, protocol {})\n\
         Instance {}, up since {}\n\
         Bootstrap: {:?}\n\
         Following {} title(s)\n",
        snapshot.process_version,
        snapshot.schema_version,
        snapshot.protocol_version,
        snapshot.app_instance_id,
        local_time(snapshot.started_at),
        snapshot.bootstrap,
        snapshot.active_follows,
    );

    match &snapshot.earliest_upcoming {
        Some(next) => out.push_str(&format!(
            "Next: {} episode {} {}\n",
            next.display_title,
            next.episode
                .map_or_else(|| "?".to_owned(), |e| e.get().to_string()),
            relative(now, next.scheduled_at)
        )),
        None => out.push_str("Next: nothing scheduled\n"),
    }

    out.push_str(&match snapshot.last_success_at {
        Some(at) => format!("Last successful refresh: {}\n", local_time(at)),
        None => "Last successful refresh: never\n".to_owned(),
    });

    out.push_str(&format!(
        "Refresh: {} due, {} stale, {} backing off, {} failed\n",
        snapshot.refresh.due,
        snapshot.refresh.stale,
        snapshot.refresh.backing_off,
        snapshot.refresh.failed,
    ));

    if let Some(until) = snapshot.source_blocked_until {
        out.push_str(&format!(
            "AniList is rate limiting until {}\n",
            local_time(until)
        ));
    }

    for reason in &snapshot.degraded {
        out.push_str(&format!("Attention: {}\n", reason.remediation()));
    }

    out.trim_end().to_owned()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn at(seconds: i64) -> UnixTimestamp {
        UnixTimestamp::new(seconds).expect("valid timestamp")
    }

    #[test]
    fn relative_durations_read_naturally() {
        let now = at(1_000_000);
        assert_eq!(relative(now, at(1_000_000 + 30)), "in under a minute");
        assert_eq!(relative(now, at(1_000_000 + 300)), "in 5m");
        assert_eq!(relative(now, at(1_000_000 + 3 * 3600 + 600)), "in 3h 10m");
        assert_eq!(relative(now, at(1_000_000 + 50 * 3600)), "in 2d 2h");
    }

    #[test]
    fn an_elapsed_instant_reads_as_now_not_as_negative_time() {
        let now = at(1_000_000);
        assert_eq!(relative(now, at(1_000_000)), "now");
        assert_eq!(relative(now, at(999_000)), "now");
    }

    #[test]
    fn the_empty_state_explains_what_to_do_next() {
        let text = upcoming(&[], at(1_000));
        assert!(text.contains("animesh search"), "{text}");
        assert!(text.contains("animesh follow"), "{text}");
    }

    #[test]
    fn stale_rows_are_marked_rather_than_hidden() {
        // Section 16: stale rows still print, with the staleness visible.
        assert_eq!(freshness_note(Freshness::Fresh), "");
        assert!(freshness_note(Freshness::Stale).contains("stale"));
        assert!(freshness_note(Freshness::BackingOff).contains("retrying"));
    }

    #[test]
    fn an_empty_search_says_so() {
        assert_eq!(search(&[]), "No matches.");
    }

    #[test]
    fn an_empty_follow_list_says_so() {
        assert_eq!(follows(&[], at(1_000)), "Not following anything yet.");
    }
}
