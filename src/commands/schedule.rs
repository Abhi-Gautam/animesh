use anyhow::Result;
use async_trait::async_trait;
use chrono::{Duration, FixedOffset, TimeZone, Utc};
use comfy_table::{Attribute, Cell, Color, ContentArrangement, Table};
use serde_json::Value;

use crate::{
    anilist::AniListClient,
    commands::Command,
    renderer::format_datetime,
    utils::{get_user_timezone, match_timezone},
};

/// Command to show upcoming anime airing schedule
pub struct ScheduleCommand {
    interval: u32,
    timezone: Option<String>,
    past: bool,
    client: AniListClient,
}

impl ScheduleCommand {
    /// Create a new schedule command
    pub fn new(interval: u32, timezone: Option<String>, past: bool) -> Self {
        Self {
            interval,
            timezone,
            past,
            client: AniListClient::new(),
        }
    }

    /// Get the timezone to use for display
    fn get_timezone(&self) -> FixedOffset {
        if let Some(tz) = &self.timezone {
            match_timezone(tz).unwrap_or_else(|| {
                eprintln!("Invalid timezone: {}. Using default timezone.", tz);
                get_user_timezone()
            })
        } else {
            get_user_timezone()
        }
    }

    /// Get the time range for the schedule
    fn get_time_range(&self) -> (i64, i64) {
        let timezone = self.get_timezone();
        let now_utc = Utc::now();
        let now_local = now_utc.with_timezone(&timezone);

        if self.past {
            // For past episodes, we go backwards from current time
            let end = now_local.timestamp();
            let start = end - ((self.interval as i64) * 24 * 3600);
            (start, end)
        } else {
            // For future episodes, we go forwards from current time
            let start = now_local.timestamp();
            let end = start + ((self.interval as i64) * 24 * 3600);
            (start, end)
        }
    }

    /// Format relative time (e.g., "2h ago", "in 3h")
    fn format_relative_time(&self, airing_at: i64) -> String {
        let now = Utc::now().timestamp();
        let diff = airing_at - now;
        let duration = Duration::seconds(diff);

        if diff < 0 {
            // Past time
            let abs_duration = Duration::seconds(-diff);
            if abs_duration.num_hours() >= 24 {
                format!("{}d ago", abs_duration.num_days())
            } else if abs_duration.num_hours() > 0 {
                format!("{}h ago", abs_duration.num_hours())
            } else if abs_duration.num_minutes() > 0 {
                format!("{}m ago", abs_duration.num_minutes())
            } else {
                "just now".to_string()
            }
        } else {
            // Future time
            if duration.num_hours() >= 24 {
                format!("in {}d", duration.num_days())
            } else if duration.num_hours() > 0 {
                format!("in {}h", duration.num_hours())
            } else if duration.num_minutes() > 0 {
                format!("in {}m", duration.num_minutes())
            } else {
                "now".to_string()
            }
        }
    }
}

#[async_trait]
impl Command for ScheduleCommand {
    async fn execute(&self) -> Result<()> {
        let timezone = self.get_timezone();
        let (start, end) = self.get_time_range();

        // Get timezone name for display
        let tz_name = if let Some(tz) = &self.timezone {
            tz.to_uppercase()
        } else {
            let offset = timezone.local_minus_utc();
            let hours = offset.abs() / 3600;
            let minutes = (offset.abs() % 3600) / 60;
            let sign = if offset >= 0 { "+" } else { "-" };
            format!("UTC{}{:02}:{:02}", sign, hours, minutes)
        };

        // GraphQL query for airing schedule
        let query = r#"
            query ($start: Int, $end: Int) {
                Page(perPage: 50) {
                    airingSchedules(airingAt_greater: $start, airingAt_lesser: $end) {
                        airingAt
                        episode
                        media {
                            title {
                                romaji
                                english
                            }
                        }
                    }
                }
            }
        "#;

        let variables = serde_json::json!({
            "start": start,
            "end": end,
        });

        let response: Value = self.client.query(query, variables).await?;
        let schedules = response["data"]["Page"]["airingSchedules"]
            .as_array()
            .unwrap();

        let mut table = Table::new();

        // Set dynamic width arrangement - THIS ENABLES WRAPPING AND DYNAMIC SIZING
        table.set_content_arrangement(ContentArrangement::Dynamic);
        table.load_preset(comfy_table::presets::UTF8_FULL);

        // Set table headers (can add styling like bold)
        table.set_header(vec![
            Cell::new(format!("Schedule ({})", tz_name)).add_attribute(Attribute::Bold),
            Cell::new("Episode").add_attribute(Attribute::Bold),
            Cell::new("Time").add_attribute(Attribute::Bold),
            Cell::new("Status").add_attribute(Attribute::Bold),
        ]);

        for schedule in schedules {
            let title = schedule["media"]["title"]["english"]
                .as_str()
                .or(schedule["media"]["title"]["romaji"].as_str())
                .unwrap_or("Unknown Title");

            let episode: i64 = schedule["episode"].as_i64().unwrap_or(0);
            let airing_at: i64 = schedule["airingAt"].as_i64().unwrap_or(0);

            let airing_time_utc = Utc.timestamp_opt(airing_at, 0).unwrap();
            let formatted_time = format_datetime(airing_time_utc, timezone);
            let relative_time = self.format_relative_time(airing_at);

            // Add row with comfy_table Cells and styling
            table.add_row(vec![
                Cell::new(title).fg(Color::Cyan), // Use comfy_table::Color
                Cell::new(episode.to_string()).fg(Color::Yellow),
                Cell::new(formatted_time).fg(Color::Green),
                Cell::new(relative_time).fg(if airing_at < Utc::now().timestamp() {
                    Color::Red
                } else {
                    Color::Blue
                }),
            ]);
        }
        println!("{table}");
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_schedule_command_today() {
        let command = ScheduleCommand::new(2, None, false);
        assert!(command.execute().await.is_ok());
    }

    #[tokio::test]
    async fn test_schedule_command_with_timezone() {
        let command = ScheduleCommand::new(2, Some("UTC".to_string()), false);
        assert!(command.execute().await.is_ok());
    }

    #[test]
    fn test_get_timezone() {
        let command = ScheduleCommand::new(2, None, false);
        let tz = command.get_timezone();
        assert!(tz.utc_minus_local() >= -14 * 3600 && tz.utc_minus_local() <= 14 * 3600);

        let command = ScheduleCommand::new(2, Some("UTC".to_string()), false);
        let tz = command.get_timezone();
        assert_eq!(tz.utc_minus_local(), 0);

        let command = ScheduleCommand::new(2, Some("IST".to_string()), false);
        let tz = command.get_timezone();
        assert_eq!(tz.utc_minus_local(), -(5 * 3600 + 30 * 60));
    }

    #[test]
    fn test_get_time_range() {
        let command = ScheduleCommand::new(2, None, false);
        let (start, end) = command.get_time_range();
        assert!(end - start == 2 * 24 * 3600);
    }

    #[test]
    fn test_format_relative_time() {
        let command = ScheduleCommand::new(2, None, false);
        let now = Utc::now().timestamp();

        // Test past times
        let one_hour_ago = command.format_relative_time(now - 3600);
        println!("One hour ago: {}", one_hour_ago);
        assert!(one_hour_ago.contains("1h ago"));

        let one_day_ago = command.format_relative_time(now - 86400);
        println!("One day ago: {}", one_day_ago);
        assert!(one_day_ago.contains("1d ago"));

        // Test future times
        let one_hour_later = command.format_relative_time(now + 3600);
        println!("One hour later: {}", one_hour_later);
        assert!(one_hour_later.contains("in 1h"));

        let one_day_later = command.format_relative_time(now + 86400);
        println!("One day later: {}", one_day_later);
        assert!(one_day_later.contains("in 1d"));
    }
}
