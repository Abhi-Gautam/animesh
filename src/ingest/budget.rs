use crate::ids::ReleaseKind;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RequestBudget {
    pub max_enter_search_requests: usize,
    pub max_follow_ingest_requests: usize,
    pub max_startup_background_requests: usize,
    pub max_periodic_requests: usize,
    pub max_manual_sync_requests: usize,
}

impl Default for RequestBudget {
    fn default() -> Self {
        Self {
            max_enter_search_requests: 4,
            max_follow_ingest_requests: 1,
            max_startup_background_requests: 10,
            max_periodic_requests: 5,
            max_manual_sync_requests: 50,
        }
    }
}

pub const SEARCH_CACHE_TTL_SECS: i64 = 24 * 3600;
pub const ACTIVE_REFRESH_TTL_SECS: i64 = 6 * 3600;
pub const NEAR_EVENT_REFRESH_TTL_SECS: i64 = 3600;
pub const FINISHED_REFRESH_TTL_SECS: i64 = 30 * 24 * 3600;
pub const MUSIC_ARTIST_REFRESH_TTL_SECS: i64 = 7 * 24 * 3600;
pub const FAILURE_BACKOFF_CAP_SECS: i64 = 24 * 3600;
const NEAR_EVENT_WINDOW_SECS: i64 = 48 * 3600;

pub fn next_refresh_due_at(
    kind: ReleaseKind,
    status: Option<&str>,
    next_event_at: Option<i64>,
    now: i64,
) -> i64 {
    if matches!(next_event_at, Some(t) if t > now && t <= now + NEAR_EVENT_WINDOW_SECS) {
        return now + NEAR_EVENT_REFRESH_TTL_SECS;
    }

    if kind == ReleaseKind::MusicArtist {
        return now + MUSIC_ARTIST_REFRESH_TTL_SECS;
    }

    let normalized = status.unwrap_or_default().to_lowercase();
    if is_active_status(&normalized) {
        now + ACTIVE_REFRESH_TTL_SECS
    } else if is_finished_status(&normalized) {
        now + FINISHED_REFRESH_TTL_SECS
    } else {
        now + ACTIVE_REFRESH_TTL_SECS
    }
}

pub fn failure_backoff(failure_count: i64) -> i64 {
    let count = failure_count.max(1).min(10) as u32;
    let secs = 15 * 60 * 2_i64.pow(count - 1);
    secs.min(FAILURE_BACKOFF_CAP_SECS)
}

fn is_active_status(status: &str) -> bool {
    [
        "active",
        "upcoming",
        "releasing",
        "running",
        "airing",
        "currently airing",
        "not_yet_released",
        "not yet aired",
        "tba",
        "unreleased",
        "current",
    ]
    .iter()
    .any(|needle| status.contains(needle))
}

fn is_finished_status(status: &str) -> bool {
    ["finished", "released", "ended", "complete"]
        .iter()
        .any(|needle| status.contains(needle))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn near_future_event_refreshes_soon() {
        assert_eq!(
            next_refresh_due_at(ReleaseKind::Anime, Some("FINISHED"), Some(2_000), 1_000),
            4_600
        );
    }

    #[test]
    fn active_status_refreshes_in_six_hours() {
        assert_eq!(
            next_refresh_due_at(ReleaseKind::Anime, Some("RELEASING"), None, 1_000),
            1_000 + ACTIVE_REFRESH_TTL_SECS
        );
    }

    #[test]
    fn finished_status_refreshes_in_thirty_days() {
        assert_eq!(
            next_refresh_due_at(ReleaseKind::Tv, Some("Ended"), None, 1_000),
            1_000 + FINISHED_REFRESH_TTL_SECS
        );
    }

    #[test]
    fn failure_backoff_is_exponential_and_capped() {
        assert_eq!(failure_backoff(1), 15 * 60);
        assert_eq!(failure_backoff(2), 30 * 60);
        assert_eq!(failure_backoff(100), FAILURE_BACKOFF_CAP_SECS);
    }
}
