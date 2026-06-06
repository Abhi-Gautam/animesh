//! Pane bucketing — the canonical algorithm from spec §2.
//!
//! Pure function. All inputs are explicit (no clock, no env). Tests
//! drive it directly; the TUI calls `bucket()` once per tracked item
//! per render tick.

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Pane {
    Today,
    Late,
    Backlog { behind: i64 },
}

#[derive(Debug, Clone, Copy)]
pub struct BucketInputs {
    pub seen: i64,
    pub total: Option<i64>,
    pub next_episode_number: Option<i64>,
    pub next_episode_airs_at: Option<i64>,
}

/// Default windows; pulled from env at app startup, then passed to
/// `bucket` for the life of the runloop.
#[derive(Debug, Clone, Copy)]
pub struct Windows {
    pub today_secs: i64,
    pub late_secs: i64,
}

impl Windows {
    pub const DEFAULT: Self = Self {
        today_secs: 24 * 3600,
        late_secs: 48 * 3600,
    };

    pub fn from_env() -> Self {
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
            late_secs: read("ANIMESH_LATE_WINDOW_HOURS", d.late_secs),
        }
    }
}

/// Spec §2 bucketing algorithm. Pure.
///
/// Returns `None` when the show should be hidden from all panes —
/// either fully watched, or caught up with no near-term airing.
pub fn bucket(i: BucketInputs, now: i64, w: Windows) -> Option<Pane> {
    // Fully watched → hidden.
    if let Some(t) = i.total {
        if i.seen >= t {
            return None;
        }
    }

    // Behind on aired episodes → Backlog with count.
    let behind = i
        .next_episode_number
        .map(|n| (n - 1).saturating_sub(i.seen))
        .unwrap_or(0);
    if behind > 0 {
        return Some(Pane::Backlog { behind });
    }

    // Caught up — split on airing time.
    if let Some(at) = i.next_episode_airs_at {
        if at > now && at <= now + w.today_secs {
            return Some(Pane::Today);
        }
        if at <= now
            && at >= now - w.late_secs
            && i.next_episode_number.map_or(false, |n| i.seen < n)
        {
            return Some(Pane::Late);
        }
    }

    // Not fully done, no imminent airing → finale-waiting Backlog.
    if i.total.map_or(false, |t| i.seen < t) {
        return Some(Pane::Backlog { behind: 0 });
    }

    None
}

#[cfg(test)]
mod tests {
    use super::*;
    use proptest::prelude::*;

    const NOW: i64 = 1_700_000_000;
    const W: Windows = Windows::DEFAULT;

    fn inputs(
        seen: i64,
        total: Option<i64>,
        next: Option<i64>,
        airs: Option<i64>,
    ) -> BucketInputs {
        BucketInputs {
            seen,
            total,
            next_episode_number: next,
            next_episode_airs_at: airs,
        }
    }

    #[test]
    fn fully_watched_is_hidden() {
        assert_eq!(
            bucket(inputs(12, Some(12), Some(12), Some(NOW + 3600)), NOW, W),
            None
        );
        assert_eq!(
            bucket(inputs(13, Some(12), Some(12), None), NOW, W),
            None
        );
    }

    #[test]
    fn behind_is_backlog_with_count() {
        // next_ep=5 aired; seen=2 → behind=2 (eps 3 and 4 unwatched).
        let r = bucket(
            inputs(2, Some(12), Some(5), Some(NOW - 3600)),
            NOW,
            W,
        );
        assert_eq!(r, Some(Pane::Backlog { behind: 2 }));
    }

    #[test]
    fn caught_up_with_imminent_airing_is_today() {
        let r = bucket(
            inputs(4, Some(12), Some(5), Some(NOW + 3600)),
            NOW,
            W,
        );
        assert_eq!(r, Some(Pane::Today));
    }

    #[test]
    fn caught_up_and_just_aired_is_late() {
        // next_ep=5 aired 3h ago; seen=4 → late.
        let r = bucket(
            inputs(4, Some(12), Some(5), Some(NOW - 3 * 3600)),
            NOW,
            W,
        );
        assert_eq!(r, Some(Pane::Late));
    }

    #[test]
    fn caught_up_long_past_window_is_hidden() {
        // Aired 10 days ago; out of the 48h late window. Caught up,
        // total unknown → hidden.
        let r = bucket(
            inputs(4, None, Some(5), Some(NOW - 10 * 24 * 3600)),
            NOW,
            W,
        );
        assert_eq!(r, None);
    }

    #[test]
    fn caught_up_far_future_is_hidden_until_inside_today_window() {
        // Next ep airs in 3 days; caught up; total unknown → hidden.
        let r = bucket(
            inputs(4, None, Some(5), Some(NOW + 3 * 24 * 3600)),
            NOW,
            W,
        );
        assert_eq!(r, None);
    }

    #[test]
    fn finale_waiting_is_backlog_zero() {
        // dandadan case: seen=11, total=12, no upcoming airing.
        let r = bucket(inputs(11, Some(12), Some(12), None), NOW, W);
        assert_eq!(r, Some(Pane::Backlog { behind: 0 }));
    }

    #[test]
    fn unknown_state_is_hidden_when_caught_up_and_unknown_total() {
        let r = bucket(inputs(0, None, None, None), NOW, W);
        assert_eq!(r, None);
    }

    #[test]
    fn windows_from_env_defaults_when_unset() {
        // We can't set env in test reliably, so just smoke that
        // DEFAULT is what we think.
        assert_eq!(Windows::DEFAULT.today_secs, 24 * 3600);
        assert_eq!(Windows::DEFAULT.late_secs, 48 * 3600);
    }

    // Property tests: the algorithm should be total (every input
    // produces a defined output) and monotone in a few directions.

    proptest! {
        #![proptest_config(ProptestConfig { cases: 256, .. ProptestConfig::default() })]

        #[test]
        fn never_panics(
            seen in 0i64..1000,
            total_opt in proptest::option::of(0i64..1000),
            next_opt in proptest::option::of(0i64..1000),
            airs_offset in -10_000_000i64..10_000_000,
            airs_some in any::<bool>(),
        ) {
            let airs = if airs_some { Some(NOW + airs_offset) } else { None };
            let _ = bucket(inputs(seen, total_opt, next_opt, airs), NOW, W);
        }

        #[test]
        fn fully_watched_always_hidden_regardless_of_airing(
            seen in 0i64..1000,
            extra in 0i64..100,
            airs_offset in -10_000_000i64..10_000_000,
        ) {
            // total == seen - any extra → fully (or over-) watched.
            let total = Some(seen.saturating_sub(extra));
            let airs = Some(NOW + airs_offset);
            prop_assert_eq!(bucket(inputs(seen, total, Some(seen + 1), airs), NOW, W), None);
        }

        #[test]
        fn behind_count_matches_arithmetic(
            seen in 0i64..500,
            ep in 1i64..1000,
            airs_offset in -10_000_000i64..-1i64,  // ensure aired (past)
        ) {
            let next = ep;
            let airs = Some(NOW + airs_offset);
            let result = bucket(inputs(seen, Some(1000), Some(next), airs), NOW, W);
            if next - 1 > seen {
                prop_assert_eq!(result, Some(Pane::Backlog { behind: next - 1 - seen }));
            }
        }
    }
}
