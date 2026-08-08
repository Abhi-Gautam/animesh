//! Serving read models — what surfaces consume, and nothing more.

use std::cmp::Ordering;

use serde::{Deserialize, Serialize};

use super::ids::{
    AniListId, BoundedText, EpisodeNumber, EventUuid, MediaId, ReleaseEventId, UnixTimestamp,
};
use super::release::FollowState;

/// Default and maximum row counts for the upcoming read model.
pub const DEFAULT_UPCOMING_LIMIT: u32 = 50;
/// Hard ceiling on the upcoming read model, enforced server-side.
pub const MAX_UPCOMING_LIMIT: u32 = 500;

/// How long an episode stays visible after its airtime.
///
/// The original serving query dropped a row the instant its airtime passed, which blanked the app during exactly the window the owner is asking
/// "did it drop yet?" — and never once reported that an episode had aired.
/// Rows inside this band are returned with [`UpcomingRelease::aired`] set.
pub const AIRED_VISIBILITY_SECS: i64 = 24 * 60 * 60;

/// How current the source data behind a row is.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Freshness {
    Fresh,
    /// Past its refresh deadline with no refresh in flight.
    Stale,
    /// A failure put it in backoff with a future retry deadline.
    BackingOff,
}

/// One upcoming release, as served to the CLI and the menu.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct UpcomingRelease {
    pub release_event_id: ReleaseEventId,
    pub event_uuid: EventUuid,
    pub media_id: MediaId,
    pub anilist_id: AniListId,
    pub display_title: BoundedText,
    pub episode: Option<EpisodeNumber>,
    pub scheduled_at: UnixTimestamp,
    pub schedule_revision: i64,
    pub last_success_at: Option<UnixTimestamp>,
    pub freshness: Freshness,
    /// Its airtime has passed. Still inside [`AIRED_VISIBILITY_SECS`], so it is
    /// shown rather than hidden.
    pub aired: bool,
}

impl UpcomingRelease {
    /// The frozen total order.
    ///
    /// Implemented here as well as in SQL so a test can prove the two agree.
    /// A read model whose order depends on which layer produced it is not a
    /// read model.
    pub fn sort_key(&self) -> (i64, i64, bool, i64, i64) {
        (
            self.scheduled_at.get(),
            self.media_id.get(),
            // `NULLS LAST`: `false` sorts before `true`, so present values win.
            self.episode.is_none(),
            self.episode.map_or(0, EpisodeNumber::get),
            self.release_event_id.get(),
        )
    }
}

impl Ord for UpcomingRelease {
    fn cmp(&self, other: &Self) -> Ordering {
        self.sort_key().cmp(&other.sort_key())
    }
}

impl PartialOrd for UpcomingRelease {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

/// A followed title and its next release, if any.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FollowSummary {
    pub media_id: MediaId,
    pub anilist_id: AniListId,
    pub display_title: BoundedText,
    pub state: FollowState,
    pub upcoming: Option<UpcomingRelease>,
    pub last_success_at: Option<UnixTimestamp>,
    pub freshness: Freshness,
}

/// What a follow request actually did.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FollowOutcome {
    NewlyFollowed,
    Reactivated,
    AlreadyActive,
}

/// The committed result of a follow.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FollowResult {
    pub media_id: MediaId,
    pub anilist_id: AniListId,
    pub display_title: BoundedText,
    pub outcome: FollowOutcome,
    pub upcoming: Option<UpcomingRelease>,
    pub last_success_at: Option<UnixTimestamp>,
}

/// Whether a manual refresh started or joined a run already in progress.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RefreshDisposition {
    Started,
    AlreadyRunning,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct RefreshAccepted {
    pub disposition: RefreshDisposition,
    pub accepted_at: UnixTimestamp,
    pub data_revision: u64,
}

/// How far bootstrap got.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum BootstrapState {
    Starting,
    Ready,
    /// Permanently failed. The process stays alive and queryable rather than
    /// crash-looping under launchd's `KeepAlive`.
    Degraded,
}

/// Machine-readable reasons the app is not fully healthy.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DegradedReason {
    SchemaTooNew,
    MigrationFailed,
    DatabaseCorrupt,
    DatabaseReadOnly,
    PragmaUnsupported,
    NotificationsDenied,
    NotificationCapacityExceeded,
    SourceRateLimited,
}

impl DegradedReason {
    /// What the owner should actually do about it.
    pub const fn remediation(self) -> &'static str {
        match self {
            Self::SchemaTooNew => {
                "The database was written by a newer version of Animesh. Install the newer version."
            }
            Self::MigrationFailed => {
                "A schema migration failed and was rolled back. Check the log for the failing statement."
            }
            Self::DatabaseCorrupt => {
                "The database failed its integrity check. Restore a copy or remove it to start fresh."
            }
            Self::DatabaseReadOnly => {
                "The database file is not writable. Check permissions on the Application Support directory."
            }
            Self::PragmaUnsupported => {
                "SQLite rejected a required durability pragma. This build cannot guarantee durable writes."
            }
            Self::NotificationsDenied => {
                "Notifications are denied. Enable them in System Settings > Notifications > Animesh."
            }
            Self::NotificationCapacityExceeded => {
                "More releases are scheduled than the system will hold. The nearest ones are registered."
            }
            Self::SourceRateLimited => "AniList is rate limiting requests. Refreshes resume automatically.",
        }
    }
}

/// Notification counts by disposition.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct NotificationCounts {
    pub desired: u32,
    pub registered: u32,
    pub failed: u32,
    /// Wanted, but beyond the proven native capacity.
    pub deferred_capacity: u32,
}

/// Refresh counts by disposition.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct RefreshCounts {
    pub due: u32,
    pub stale: u32,
    pub backing_off: u32,
    pub failed: u32,
}

/// Reported notification authorization.
///
/// Named after the macOS states because that is the only surface with a real
/// permission model; on a desktop that has none, an adapter reports Authorized
/// when the notification service answers at all.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AuthorizationState {
    #[default]
    Unknown,
    NotDetermined,
    Denied,
    Authorized,
    Provisional,
    Ephemeral,
}

impl AuthorizationState {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Unknown => "unknown",
            Self::NotDetermined => "not_determined",
            Self::Denied => "denied",
            Self::Authorized => "authorized",
            Self::Provisional => "provisional",
            Self::Ephemeral => "ephemeral",
        }
    }

    pub fn parse(value: &str) -> Option<Self> {
        match value {
            "unknown" => Some(Self::Unknown),
            "not_determined" => Some(Self::NotDetermined),
            "denied" => Some(Self::Denied),
            "authorized" => Some(Self::Authorized),
            "provisional" => Some(Self::Provisional),
            "ephemeral" => Some(Self::Ephemeral),
            _ => None,
        }
    }

    /// Whether submitting a request could plausibly succeed.
    pub const fn permits_registration(self) -> bool {
        matches!(self, Self::Authorized | Self::Provisional | Self::Ephemeral)
    }
}

/// The full status surface.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct HealthSnapshot {
    pub process_version: String,
    pub schema_version: i64,
    pub protocol_version: u32,
    pub app_instance_id: String,
    pub started_at: UnixTimestamp,
    pub bootstrap: BootstrapState,
    pub database_ready: bool,
    pub active_follows: u32,
    pub earliest_upcoming: Option<UpcomingRelease>,
    pub last_success_at: Option<UnixTimestamp>,
    pub refresh: RefreshCounts,
    pub source_blocked_until: Option<UnixTimestamp>,
    pub notifications: NotificationCounts,
    pub last_reconciled_at: Option<UnixTimestamp>,
    pub authorization: AuthorizationState,
    pub authorization_observed_at: Option<UnixTimestamp>,
    pub degraded: Vec<DegradedReason>,
}

#[cfg(test)]
mod tests {
    use super::*;

    fn at(seconds: i64) -> UnixTimestamp {
        UnixTimestamp::new(seconds).expect("valid timestamp")
    }

    fn upcoming(
        scheduled_at: i64,
        media_id: i64,
        episode: Option<i64>,
        event_id: i64,
    ) -> UpcomingRelease {
        UpcomingRelease {
            release_event_id: ReleaseEventId::new(event_id).expect("valid id"),
            event_uuid: EventUuid::generate(),
            media_id: MediaId::new(media_id).expect("valid id"),
            anilist_id: AniListId::new(21).expect("valid id"),
            display_title: BoundedText::new("t", 512, "Title").expect("valid title"),
            episode: episode.map(|e| EpisodeNumber::new(e).expect("valid episode")),
            scheduled_at: at(scheduled_at),
            schedule_revision: 1,
            last_success_at: None,
            freshness: Freshness::Fresh,
            aired: false,
        }
    }

    #[test]
    fn order_is_by_scheduled_time_first() {
        let mut rows = [upcoming(200, 1, Some(1), 1), upcoming(100, 9, Some(9), 9)];
        rows.sort();
        assert_eq!(rows[0].scheduled_at.get(), 100);
    }

    #[test]
    fn ties_break_on_media_then_episode_then_event() {
        let mut rows = [
            upcoming(100, 2, Some(1), 5),
            upcoming(100, 1, Some(2), 4),
            upcoming(100, 1, Some(1), 3),
        ];
        rows.sort();
        assert_eq!(
            rows.iter()
                .map(|r| (r.media_id.get(), r.episode.map(EpisodeNumber::get)))
                .collect::<Vec<_>>(),
            vec![(1, Some(1)), (1, Some(2)), (2, Some(1))]
        );
    }

    #[test]
    fn null_episodes_sort_last_within_a_tie() {
        // Rust's natural Option order puts None first; the frozen contract says
        // NULLS LAST, so this asserts the override actually took effect.
        let mut rows = [upcoming(100, 1, None, 2), upcoming(100, 1, Some(7), 1)];
        rows.sort();
        assert_eq!(rows[0].episode.map(EpisodeNumber::get), Some(7));
        assert_eq!(rows[1].episode, None);
    }

    #[test]
    fn event_id_is_the_final_tiebreak() {
        let mut rows = [upcoming(100, 1, Some(1), 9), upcoming(100, 1, Some(1), 2)];
        rows.sort();
        assert_eq!(rows[0].release_event_id.get(), 2);
    }

    #[test]
    fn ordering_is_total_so_sorting_is_deterministic() {
        let rows = vec![
            upcoming(100, 1, None, 2),
            upcoming(100, 1, Some(1), 1),
            upcoming(50, 3, Some(4), 8),
        ];
        let mut forward = rows.clone();
        forward.sort();
        let mut reversed = rows;
        reversed.reverse();
        reversed.sort();
        assert_eq!(
            forward.iter().map(|r| r.sort_key()).collect::<Vec<_>>(),
            reversed.iter().map(|r| r.sort_key()).collect::<Vec<_>>()
        );
    }

    #[test]
    fn authorization_encodings_are_stable() {
        for state in [
            AuthorizationState::Unknown,
            AuthorizationState::NotDetermined,
            AuthorizationState::Denied,
            AuthorizationState::Authorized,
            AuthorizationState::Provisional,
            AuthorizationState::Ephemeral,
        ] {
            assert_eq!(AuthorizationState::parse(state.as_str()), Some(state));
        }
        assert_eq!(AuthorizationState::parse("granted"), None);
    }

    #[test]
    fn only_granting_states_permit_registration() {
        assert!(AuthorizationState::Authorized.permits_registration());
        assert!(AuthorizationState::Provisional.permits_registration());
        assert!(AuthorizationState::Ephemeral.permits_registration());
        assert!(!AuthorizationState::Denied.permits_registration());
        assert!(!AuthorizationState::NotDetermined.permits_registration());
        assert!(!AuthorizationState::Unknown.permits_registration());
    }

    #[test]
    fn every_degraded_reason_carries_an_actionable_remediation() {
        for reason in [
            DegradedReason::SchemaTooNew,
            DegradedReason::MigrationFailed,
            DegradedReason::DatabaseCorrupt,
            DegradedReason::DatabaseReadOnly,
            DegradedReason::PragmaUnsupported,
            DegradedReason::NotificationsDenied,
            DegradedReason::NotificationCapacityExceeded,
            DegradedReason::SourceRateLimited,
        ] {
            let text = reason.remediation();
            assert!(!text.is_empty(), "{reason:?} has no remediation");
        }
    }
}
