//! Human-readable output.
//!
//! Stdout is data; stderr is diagnostics. Nothing here formats for machines —
//! There is no stable JSON surface yet, and pretending otherwise would freeze
//! a contract nobody has designed.

use chrono::{Local, TimeZone};

use crate::domain::ids::UnixTimestamp;
use crate::domain::media::SearchCandidate;
use crate::domain::read_models::{
    BootstrapState, FollowResult, FollowSummary, Freshness, HealthSnapshot, UpcomingRelease,
};

/// Formats an absolute instant in the viewer's local timezone, naming the zone.
///
/// The zone is not decoration. An airtime without one is exactly the thing that
/// gets misread when travelling, or when pasted somewhere else.
pub fn local_time(at: UnixTimestamp) -> String {
    match Local.timestamp_opt(at.get(), 0).single() {
        Some(local) => local.format("%a %d %b %Y %H:%M %Z").to_string(),
        // Ambiguous or nonexistent local times happen at DST boundaries; the
        // UTC instant is still true and still useful.
        None => format!("{} UTC", at.to_utc().format("%a %d %b %Y %H:%M")),
    }
}

/// A compact duration with no trailing zero unit.
///
/// `in 6d 0h` is noise; `in 6d` is the same information.
fn compact(seconds: i64) -> String {
    let minutes = seconds / 60;
    let hours = minutes / 60;
    let days = hours / 24;

    if days > 0 {
        let rem = hours % 24;
        if rem == 0 {
            format!("{days}d")
        } else {
            format!("{days}d {rem}h")
        }
    } else if hours > 0 {
        let rem = minutes % 60;
        if rem == 0 {
            format!("{hours}h")
        } else {
            format!("{hours}h {rem}m")
        }
    } else if minutes > 0 {
        format!("{minutes}m")
    } else {
        "under a minute".to_owned()
    }
}

/// How far away an airtime is, before anyone phrases it.
///
/// Separated from the wording so the CLI and the menu can share one decision
/// about which scale to use and still read naturally in their own voice. The
/// alternative — one of them matching on the other's strings — makes a phrasing
/// change silently break a surface.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Whenish {
    /// Already aired, this long ago.
    Aired(String),
    Now,
    /// Sooner than a day away.
    Within(String),
    /// This week; named rather than counted, because nobody thinks in "2d 2h".
    Weekday(String),
    Date(String),
}

/// Past its airtime this reports how long ago rather than "now", because "now"
/// reads as an instruction to go watch something that has not necessarily
/// posted yet.
pub fn when(from: UnixTimestamp, to: UnixTimestamp) -> Whenish {
    let seconds = from.seconds_until(to);

    if seconds < 0 {
        return Whenish::Aired(compact(-seconds));
    }
    if seconds < 60 {
        return Whenish::Now;
    }
    if seconds < 24 * 3600 {
        return Whenish::Within(compact(seconds));
    }

    match Local.timestamp_opt(to.get(), 0).single() {
        Some(local) if seconds < 7 * 24 * 3600 => {
            Whenish::Weekday(local.format("%A %H:%M").to_string().to_lowercase())
        }
        Some(local) => Whenish::Date(local.format("%a %d %b").to_string()),
        // A DST boundary makes the local wall time ambiguous; the elapsed
        // count is still exactly true.
        None => Whenish::Within(compact(seconds)),
    }
}

/// When something happens, phrased the way a person holds it.
pub fn relative(from: UnixTimestamp, to: UnixTimestamp) -> String {
    match when(from, to) {
        Whenish::Aired(ago) => format!("aired {ago} ago"),
        Whenish::Now => "airing now".to_owned(),
        Whenish::Within(left) => format!("in {left}"),
        Whenish::Weekday(named) | Whenish::Date(named) => named,
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
        // A leading mark so an already-aired episode is findable by shape
        // rather than by reading every timestamp.
        let mark = if row.aired { "*" } else { " " };
        out.push_str(&format!(
            "{mark} {}\n    Episode {}  {}  {}{}\n",
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
    // Rendered explicitly rather than through `{:?}`, which leaked a Rust
    // debug form into the one screen the owner reads when something is wrong.
    let state = match snapshot.bootstrap {
        BootstrapState::Starting => "starting up",
        BootstrapState::Ready => "running",
        BootstrapState::Degraded => "needs attention",
    };

    let mut out = format!(
        "Animesh {} — {state}\nRunning since {}\nFollowing {} title(s)\n",
        snapshot.process_version,
        local_time(snapshot.started_at),
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

    // The one question worth running `status` for: am I actually going to get
    // pinged tonight? It was in the snapshot and never printed.
    let notifications = &snapshot.notifications;
    out.push_str(&format!(
        "Notifications: {} registered with macOS",
        notifications.registered
    ));
    if notifications.desired > 0 {
        out.push_str(&format!(", {} waiting", notifications.desired));
    }
    if notifications.failed > 0 {
        out.push_str(&format!(", {} failed", notifications.failed));
    }
    if notifications.deferred_capacity > 0 {
        out.push_str(&format!(
            ", {} beyond what macOS will hold",
            notifications.deferred_capacity
        ));
    }
    out.push('\n');

    if snapshot.refresh.backing_off > 0 || snapshot.refresh.failed > 0 {
        out.push_str(&format!(
            "Refresh trouble: {} backing off, {} failed\n",
            snapshot.refresh.backing_off, snapshot.refresh.failed,
        ));
    }

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
    fn durations_inside_a_day_read_as_a_countdown() {
        let now = at(1_000_000);
        assert_eq!(relative(now, at(1_000_000 + 30)), "airing now");
        assert_eq!(relative(now, at(1_000_000 + 300)), "in 5m");
        assert_eq!(relative(now, at(1_000_000 + 3 * 3600 + 600)), "in 3h 10m");
    }

    #[test]
    fn a_past_airtime_says_how_long_ago_rather_than_now() {
        // "now" is an instruction to go watch. Something that aired two hours
        // ago needs a different answer, and it is the answer the owner is
        // actually looking for.
        let now = at(1_000_000);
        assert_eq!(relative(now, at(1_000_000 - 1_920)), "aired 32m ago");
        assert_eq!(relative(now, at(1_000_000 - 2 * 3600)), "aired 2h ago");
    }

    #[test]
    fn beyond_tomorrow_it_names_the_day() {
        // Nobody holds "in 2d 2h" in their head.
        let now = at(1_000_000);
        let text = relative(now, at(1_000_000 + 50 * 3600));
        assert!(!text.starts_with("in "), "still a countdown: {text}");

        const WEEKDAYS: [&str; 7] = [
            "monday",
            "tuesday",
            "wednesday",
            "thursday",
            "friday",
            "saturday",
            "sunday",
        ];
        assert!(
            WEEKDAYS.iter().any(|day| text.starts_with(day)),
            "expected a weekday name, got {text:?}"
        );
    }

    #[test]
    fn compact_durations_drop_a_trailing_zero_unit() {
        assert_eq!(compact(6 * 24 * 3600), "6d");
        assert_eq!(compact(3 * 3600), "3h");
        assert_eq!(compact(3 * 3600 + 600), "3h 10m");
        assert_eq!(compact(25 * 3600), "1d 1h");
    }

    #[test]
    fn local_time_names_the_zone() {
        // A bare 22:27 is what gets misread.
        let text = local_time(at(1_700_000_000));
        let last = text.split_whitespace().last().unwrap_or_default();
        assert!(!last.is_empty() && last != "22:27", "no zone in {text:?}");
        assert!(text.len() > "Fri 07 Aug 2026 22:27".len(), "{text:?}");
    }

    #[test]
    fn the_empty_state_explains_what_to_do_next() {
        let text = upcoming(&[], at(1_000));
        assert!(text.contains("animesh search"), "{text}");
        assert!(text.contains("animesh follow"), "{text}");
    }

    #[test]
    fn stale_rows_are_marked_rather_than_hidden() {
        // Stale rows still print, with the staleness visible.
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
