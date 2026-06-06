//! Pure presentation. No I/O, no async, no domain logic — every
//! function takes typed input and returns a string for printing.

use chrono::{DateTime, Duration, FixedOffset, TimeZone, Utc};
use comfy_table::{Attribute, Cell, Color, ContentArrangement, Table};

use crate::store::TrackedItem;

/// One row of a schedule table — used by both followed-only and --all
/// rendering paths so the table style stays consistent.
#[derive(Debug, Clone)]
pub struct ScheduleRow {
    pub title: String,
    pub episode: i64,
    pub airing_at: i64,
}

/// Format a datetime to the user's timezone without the timezone info
pub fn format_datetime(dt: DateTime<Utc>, timezone: FixedOffset) -> String {
    let local_time = dt.with_timezone(&timezone);
    local_time.format("%H:%M %m/%d/%y").to_string()
}

/// Render a unix timestamp as a short date string in the given
/// timezone. Used by `list` and `doctor`.
fn fmt_date(unix_secs: i64, tz: FixedOffset) -> String {
    Utc.timestamp_opt(unix_secs, 0)
        .single()
        .map(|dt| dt.with_timezone(&tz).format("%Y-%m-%d").to_string())
        .unwrap_or_else(|| "?".into())
}

/// Render the library as a table. `colored` controls ANSI styling —
/// snapshot tests pass `false` so the output is deterministic.
pub fn render_tracked_items(items: &[TrackedItem], tz: FixedOffset, colored: bool) -> String {
    if items.is_empty() {
        return "No followed shows yet — try `animesh follow <query>`.\n".to_string();
    }
    let mut table = Table::new();
    table.set_content_arrangement(ContentArrangement::Dynamic);
    table.load_preset(comfy_table::presets::UTF8_FULL);
    let bold = |s: &str| Cell::new(s).add_attribute(Attribute::Bold);
    table.set_header(vec![
        bold("Title"),
        bold("Source"),
        bold("ID"),
        bold("Followed"),
        bold("State"),
    ]);
    for item in items {
        let title_cell = if colored {
            Cell::new(&item.display_title).fg(Color::Cyan)
        } else {
            Cell::new(&item.display_title)
        };
        let state_str = if item.dropped_at.is_some() {
            "dropped"
        } else {
            "active"
        };
        let state_cell = if colored && item.dropped_at.is_some() {
            Cell::new(state_str).fg(Color::DarkGrey)
        } else if colored {
            Cell::new(state_str).fg(Color::Green)
        } else {
            Cell::new(state_str)
        };
        table.add_row(vec![
            title_cell,
            Cell::new(&item.source),
            Cell::new(&item.source_id),
            Cell::new(fmt_date(item.followed_at, tz)),
            state_cell,
        ]);
    }
    table.to_string()
}

/// Format a future/past relative time. Mirrors the original
/// schedule command's "in 3h" / "2d ago" style.
fn format_relative(airing_at: i64, now: i64) -> String {
    let diff = airing_at - now;
    if diff < 0 {
        let d = Duration::seconds(-diff);
        if d.num_hours() >= 24 {
            format!("{}d ago", d.num_days())
        } else if d.num_hours() > 0 {
            format!("{}h ago", d.num_hours())
        } else if d.num_minutes() > 0 {
            format!("{}m ago", d.num_minutes())
        } else {
            "just now".into()
        }
    } else {
        let d = Duration::seconds(diff);
        if d.num_hours() >= 24 {
            format!("in {}d", d.num_days())
        } else if d.num_hours() > 0 {
            format!("in {}h", d.num_hours())
        } else if d.num_minutes() > 0 {
            format!("in {}m", d.num_minutes())
        } else {
            "now".into()
        }
    }
}

/// Render a schedule table. Used by `animesh schedule` in both
/// followed-only and `--all` modes. `now` is supplied so tests can
/// pin the relative-time column deterministically.
pub fn render_schedule(
    rows: &[ScheduleRow],
    tz: FixedOffset,
    tz_label: &str,
    now: i64,
    colored: bool,
) -> String {
    if rows.is_empty() {
        return "No episodes in this window.\n".to_string();
    }
    let mut table = Table::new();
    table.set_content_arrangement(ContentArrangement::Dynamic);
    table.load_preset(comfy_table::presets::UTF8_FULL);
    let bold = |s: String| Cell::new(s).add_attribute(Attribute::Bold);
    table.set_header(vec![
        bold(format!("Schedule ({tz_label})")),
        bold("Episode".into()),
        bold("Time".into()),
        bold("Status".into()),
    ]);
    for row in rows {
        let airing_utc = Utc
            .timestamp_opt(row.airing_at, 0)
            .single()
            .expect("airing_at out of range");
        let time = format_datetime(airing_utc, tz);
        let rel = format_relative(row.airing_at, now);
        let title_cell = if colored {
            Cell::new(&row.title).fg(Color::Cyan)
        } else {
            Cell::new(&row.title)
        };
        let ep_cell = if colored {
            Cell::new(row.episode.to_string()).fg(Color::Yellow)
        } else {
            Cell::new(row.episode.to_string())
        };
        let time_cell = if colored {
            Cell::new(time).fg(Color::Green)
        } else {
            Cell::new(time)
        };
        let rel_cell = if colored {
            let color = if row.airing_at < now { Color::Red } else { Color::Blue };
            Cell::new(rel).fg(color)
        } else {
            Cell::new(rel)
        };
        table.add_row(vec![title_cell, ep_cell, time_cell, rel_cell]);
    }
    table.to_string()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::store::TrackedItem;

    fn item(source_id: &str, display: &str, dropped: bool) -> TrackedItem {
        TrackedItem {
            id: 1,
            source: "anilist".into(),
            source_id: source_id.into(),
            kind: "anime".into(),
            display_title: display.into(),
            followed_at: 1_700_000_000,
            dropped_at: if dropped { Some(1_700_100_000) } else { None },
            user_note: None,
        }
    }

    fn utc() -> FixedOffset {
        FixedOffset::east_opt(0).unwrap()
    }

    #[test]
    fn empty_library_renders_friendly_hint() {
        let out = render_tracked_items(&[], utc(), false);
        assert!(
            out.contains("No followed shows"),
            "missing hint, got: {out}"
        );
        assert!(out.contains("animesh follow"));
    }

    #[test]
    fn renders_titles_state_and_source_id() {
        let items = vec![
            item("21", "One Piece", false),
            item("1", "Cowboy Bebop", true),
        ];
        let out = render_tracked_items(&items, utc(), false);
        assert!(out.contains("One Piece"));
        assert!(out.contains("Cowboy Bebop"));
        assert!(out.contains("anilist"));
        assert!(out.contains("active"));
        assert!(out.contains("dropped"));
        assert!(out.contains("21"));
    }

    #[test]
    fn renders_dates_in_supplied_timezone() {
        let items = vec![item("21", "One Piece", false)];
        // 1_700_000_000 == 2023-11-14 22:13:20 UTC
        let out = render_tracked_items(&items, utc(), false);
        assert!(out.contains("2023-11-14"));
    }

    #[test]
    fn no_color_output_is_ansi_free() {
        let items = vec![item("21", "One Piece", false)];
        let out = render_tracked_items(&items, utc(), false);
        assert!(!out.contains("\x1b["), "expected no ANSI escapes: {out:?}");
    }

    #[test]
    fn render_schedule_empty_prints_hint() {
        let out = render_schedule(&[], utc(), "UTC", 0, false);
        assert!(out.contains("No episodes"));
    }

    #[test]
    fn render_schedule_lists_rows_with_relative_time() {
        let now = 1_700_000_000;
        let rows = vec![ScheduleRow {
            title: "One Piece".into(),
            episode: 1100,
            airing_at: now + 3600,
        }];
        let out = render_schedule(&rows, utc(), "UTC", now, false);
        assert!(out.contains("One Piece"));
        assert!(out.contains("1100"));
        assert!(out.contains("in 1h"));
    }

    #[test]
    fn render_schedule_marks_past_as_ago() {
        let now = 1_700_000_000;
        let rows = vec![ScheduleRow {
            title: "Cowboy Bebop".into(),
            episode: 26,
            airing_at: now - 86400,
        }];
        let out = render_schedule(&rows, utc(), "UTC", now, false);
        assert!(out.contains("1d ago"));
    }
}
