use anyhow::Result;
use async_trait::async_trait;
use chrono::{Datelike, FixedOffset, TimeZone, Utc, Duration};
use prettytable::{color, Row};
use serde_json::Value;

use crate::{
    api::AniListClient,
    commands::Command,
    display::{create_table, format_datetime, styled_cell},
    utils::{get_user_timezone, parse_day_of_week, match_timezone},
};

/// Command to show upcoming anime airing schedule
pub struct ScheduleCommand {
    day: Option<String>,
    interval: u32,
    timezone: Option<String>,
    client: AniListClient,
}

impl ScheduleCommand {
    /// Create a new schedule command
    pub fn new(day: Option<String>, interval: u32, timezone: Option<String>) -> Self {
        Self {
            day,
            interval,
            timezone,
            client: AniListClient::new(),
        }
    }

    /// Get the timezone to use for display
    fn get_timezone(&self) -> FixedOffset {
        if let Some(tz) = &self.timezone {
            match_timezone(&tz).unwrap_or_else(|| {
                eprintln!("Invalid timezone: {}. Using default timezone.", tz);
                get_user_timezone()
            })
        } else {
            get_user_timezone()
        }
    }

    /// Get the day to show schedule for
    fn get_target_day(&self) -> u32 {
        if let Some(day) = &self.day {
            parse_day_of_week(day).unwrap_or(Utc::now().weekday().num_days_from_monday())
        } else {
            Utc::now().weekday().num_days_from_monday()
        }
    }

    /// Get the time range for the schedule
    fn get_time_range(&self) -> (i64, i64) {
        let timezone = self.get_timezone();
        let now_utc = Utc::now();
        let now_local = now_utc.with_timezone(&timezone);
        
        let target_day = self.get_target_day();
        let current_day = now_local.weekday().num_days_from_monday();
        let days_diff = if target_day > current_day {
            target_day - current_day
        } else {
            0
        };
        
        let start = now_local.timestamp() + ((days_diff as i64) * 24 * 3600);
        let end = start + ((self.interval as i64) * 24 * 3600);
        
        (start, end)
    }

    /// Format relative time (e.g., "2h ago", "in 3h")
    fn format_relative_time(&self, airing_at: i64) -> String {
        let now = Utc::now().timestamp();
        let diff = airing_at - now;
        let duration = Duration::seconds(diff);

        if diff < 0 {
            // Past time
            let abs_duration = Duration::seconds(-diff);
            if abs_duration.num_hours() > 24 {
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
            if duration.num_hours() > 24 {
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
            let offset = timezone.utc_minus_local();
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
        let schedules = response["data"]["Page"]["airingSchedules"].as_array().unwrap();

        // Create and populate table with timezone header
        let mut table = create_table(&[&format!("Schedule ({})", tz_name), "Episode", "Time", "Status"]);
        
        for schedule in schedules {
            let title = schedule["media"]["title"]["english"]
                .as_str()
                .or(schedule["media"]["title"]["romaji"].as_str())
                .unwrap_or("Unknown Title");
            
            let episode: i64 = schedule["episode"].as_i64().unwrap_or(0);
            let airing_at: i64 = schedule["airingAt"].as_i64().unwrap_or(0);
            
            let airing_time = Utc.timestamp_opt(airing_at, 0).unwrap();
            let formatted_time = format_datetime(airing_time, timezone);
            let relative_time = self.format_relative_time(airing_at);

            table.add_row(Row::new(vec![
                styled_cell(title, color::CYAN),
                styled_cell(&episode.to_string(), color::YELLOW),
                styled_cell(&formatted_time, color::GREEN),
                styled_cell(&relative_time, if airing_at < Utc::now().timestamp() { color::RED } else { color::BLUE }),
            ]));
        }

        table.printstd();
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_schedule_command_today() {
        let command = ScheduleCommand::new(None, 2, None);
        assert!(command.execute().await.is_ok());
    }

    #[tokio::test]
    async fn test_schedule_command_specific_day() {
        let command = ScheduleCommand::new(Some("monday".to_string()), 2, None);
        assert!(command.execute().await.is_ok());
    }

    #[tokio::test]
    async fn test_schedule_command_with_timezone() {
        let command = ScheduleCommand::new(None, 2, Some("UTC".to_string()));
        assert!(command.execute().await.is_ok());
    }

    #[test]
    fn test_get_timezone() {
        let command = ScheduleCommand::new(None, 2, None);
        let tz = command.get_timezone();
        assert!(tz.utc_minus_local() >= -14 * 3600 && tz.utc_minus_local() <= 14 * 3600);

        let command = ScheduleCommand::new(None, 2, Some("UTC".to_string()));
        let tz = command.get_timezone();
        assert_eq!(tz.utc_minus_local(), 0);

        let command = ScheduleCommand::new(None, 2, Some("IST".to_string()));
        let tz = command.get_timezone();
        assert_eq!(tz.utc_minus_local(), -(5 * 3600 + 30 * 60));
    }

    #[test]
    fn test_get_target_day() {
        let command = ScheduleCommand::new(None, 2, None);
        let day = command.get_target_day();
        assert!(day < 7);

        let command = ScheduleCommand::new(Some("monday".to_string()), 2, None);
        assert_eq!(command.get_target_day(), 0);
    }

    #[test]
    fn test_get_time_range() {
        let command = ScheduleCommand::new(None, 2, None);
        let (start, end) = command.get_time_range();
        assert!(end - start == 2 * 24 * 3600);
    }

    #[test]
    fn test_format_relative_time() {
        let command = ScheduleCommand::new(None, 2, None);
        let now = Utc::now().timestamp();
        
        // Test past times
        assert!(command.format_relative_time(now - 3600).contains("1h ago"));
        assert!(command.format_relative_time(now - 86400).contains("1d ago"));
        
        // Test future times
        assert!(command.format_relative_time(now + 3600).contains("in 1h"));
        assert!(command.format_relative_time(now + 86400).contains("in 1d"));
    }
} 