//! Pane bucketing — manifesto-aligned playability state.
//!
//! Three coequal panes:
//! - `Playable`: verified streamable now on a subscribed streamer.
//! - `Dropping`: scheduled to drop inside `today_secs`, not yet verified.
//! - `Following`: everything else not fully done.
//!
//! Pure function. Kind-agnostic — drives anime, TV, film, music alike.

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Pane {
    Playable,
    Dropping,
    Following,
}

#[derive(Debug, Clone)]
pub(crate) struct BucketInputs {
    /// Earliest known future drop time across all kinds (anime episode
    /// air time, music release date, film release date). None when
    /// the source has no scheduled drop.
    pub next_drop_at: Option<i64>,
    /// Most recent `EngagementEvent::Verified` timestamp.
    pub verified_playable_at: Option<i64>,
    /// True iff the most recent verify event's streamer is in the user's subs.
    pub subscribed: bool,
    /// True iff the canonical is fully consumed (e.g. all episodes
    /// watched, album played). Hides from every pane.
    pub fully_done: bool,
}

#[derive(Debug, Clone, Copy)]
pub(crate) struct Windows {
    /// Inclusive horizon for `Dropping` — drops at-or-before
    /// `now + today_secs` show in Dropping.
    pub today_secs: i64,
    /// A verified link is considered "still playable" for this long
    /// after the verification timestamp.
    pub playable_secs: i64,
}

impl Windows {
    pub(crate) const DEFAULT: Self = Self {
        today_secs: 24 * 3600,
        playable_secs: 24 * 3600,
    };

    pub(crate) fn from_env() -> Self {
        let d = Self::DEFAULT;
        let read = |key: &str, default: i64| -> i64 {
            std::env::var(key)
                .ok()
                .and_then(|v| v.parse::<i64>().ok())
                .map(|hours| hours * 3600)
                .unwrap_or(default)
        };
        Self {
            today_secs: read("ANIMESH_TODAY_WINDOW_HOURS", d.today_secs),
            playable_secs: read("ANIMESH_PLAYABLE_WINDOW_HOURS", d.playable_secs),
        }
    }
}

pub(crate) fn bucket(i: BucketInputs, now: i64, w: Windows) -> Option<Pane> {
    if i.fully_done {
        return None;
    }
    if let Some(v) = i.verified_playable_at {
        if i.subscribed && v <= now && now - v <= w.playable_secs {
            return Some(Pane::Playable);
        }
    }
    if let Some(d) = i.next_drop_at {
        if d > now && d <= now + w.today_secs {
            return Some(Pane::Dropping);
        }
    }
    Some(Pane::Following)
}

#[cfg(test)]
mod tests {
    use super::*;

    const NOW: i64 = 1_700_000_000;
    const W: Windows = Windows::DEFAULT;

    fn empty() -> BucketInputs {
        BucketInputs {
            next_drop_at: None,
            verified_playable_at: None,
            subscribed: false,
            fully_done: false,
        }
    }

    #[test]
    fn verified_within_window_and_subscribed_is_playable() {
        let i = BucketInputs {
            verified_playable_at: Some(NOW - 5 * 60),
            subscribed: true,
            ..empty()
        };
        assert_eq!(bucket(i, NOW, W), Some(Pane::Playable));
    }

    #[test]
    fn verified_but_unsubscribed_is_following_not_playable() {
        let i = BucketInputs {
            verified_playable_at: Some(NOW - 5 * 60),
            subscribed: false,
            ..empty()
        };
        assert_eq!(bucket(i, NOW, W), Some(Pane::Following));
    }

    #[test]
    fn scheduled_inside_today_window_is_dropping() {
        let i = BucketInputs {
            next_drop_at: Some(NOW + 3 * 3600),
            ..empty()
        };
        assert_eq!(bucket(i, NOW, W), Some(Pane::Dropping));
    }

    #[test]
    fn scheduled_far_future_is_following() {
        let i = BucketInputs {
            next_drop_at: Some(NOW + 60 * 24 * 3600),
            ..empty()
        };
        assert_eq!(bucket(i, NOW, W), Some(Pane::Following));
    }

    #[test]
    fn fully_done_is_hidden() {
        let i = BucketInputs {
            fully_done: true,
            next_drop_at: Some(NOW + 3600),
            ..empty()
        };
        assert_eq!(bucket(i, NOW, W), None);
    }

    #[test]
    fn no_drop_no_verify_is_following() {
        assert_eq!(bucket(empty(), NOW, W), Some(Pane::Following));
    }

    #[test]
    fn verified_takes_precedence_over_scheduled() {
        // Even with a scheduled drop in the future, an active verified
        // playable on a subscribed streamer wins the Playable pane.
        let i = BucketInputs {
            next_drop_at: Some(NOW + 3600),
            verified_playable_at: Some(NOW - 60),
            subscribed: true,
            ..empty()
        };
        assert_eq!(bucket(i, NOW, W), Some(Pane::Playable));
    }
}
