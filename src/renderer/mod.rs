use chrono::{DateTime, FixedOffset, Utc};

/// Format a datetime to the user's timezone without the timezone info
pub fn format_datetime(dt: DateTime<Utc>, timezone: FixedOffset) -> String {
    let local_time = dt.with_timezone(&timezone);
    local_time.format("%H:%M %m/%d/%y").to_string()
}
