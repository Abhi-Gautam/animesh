//! Pure presentation. No I/O, no async, no domain logic — every
//! function takes typed input and returns a string for printing.

use chrono::{DateTime, FixedOffset, TimeZone, Utc};
use comfy_table::{Attribute, Cell, Color, ContentArrangement, Table};

use crate::store::TrackedItem;

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
}
