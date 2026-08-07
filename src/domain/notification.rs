//! Notification intent: what Animesh wants macOS to have registered.

use serde::{Deserialize, Serialize};

use super::ids::{
    AniListId, EpisodeNumber, EventUuid, IdError, InstallationUuid, ReleaseEventId, UnixTimestamp,
};

/// How late a notification may still be submitted after its airtime.
///
/// Past this the episode is old news and a banner would be noise rather than a
/// signal, so the job expires instead.
pub const CATCH_UP_GRACE_SECS: i64 = 15 * 60;

/// Prefix of every OS notification identifier Animesh owns.
///
/// Reconciliation removes pending requests that carry this prefix and are not
/// in the desired set, so it must never match a request from another app.
pub const OS_IDENTIFIER_PREFIX: &str = "dev.animesh.release.";

/// Maximum stored length of the canonical request JSON.
pub const MAX_REQUEST_JSON_LEN: usize = 4096;

/// How many pending requests Animesh will hold in the OS at once.
///
/// Apple keeps only the soonest-firing requests once an app exceeds its limit
/// and drops the rest silently, so this is deliberately a floor rather than a
/// guess at the ceiling: jobs beyond it stay durable and surface as deferred,
/// which is visible, instead of being registered and quietly discarded.
///
/// D1 must confirm the real lower bound on the minimum supported OS before
/// this is raised.
pub const NATIVE_CAPACITY: u32 = 64;

/// Lifecycle of a notification job.
///
/// There is deliberately no `ambiguous` state. Every reconciliation pass reads
/// the OS pending and delivered lists first and treats them as authoritative,
/// so a crash between OS acceptance and commit is resolved by observation on
/// the next pass rather than by speculative bookkeeping.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum NotificationJobState {
    /// Wanted, not yet confirmed present in the OS.
    Desired,
    /// Confirmed present in the OS pending list at the desired revision.
    Registered,
    /// Observed in the OS delivered list. Terminal for this episode.
    Delivered,
    /// Last attempt failed transiently; `retry_after` gates the next one.
    Failed,
    /// Airtime plus grace passed before it was ever registered.
    Expired,
    /// Follow dropped, or the event was withdrawn or superseded.
    Cancelled,
}

impl NotificationJobState {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Desired => "desired",
            Self::Registered => "registered",
            Self::Delivered => "delivered",
            Self::Failed => "failed",
            Self::Expired => "expired",
            Self::Cancelled => "cancelled",
        }
    }

    pub fn parse(value: &str) -> Result<Self, IdError> {
        match value {
            "desired" => Ok(Self::Desired),
            "registered" => Ok(Self::Registered),
            "delivered" => Ok(Self::Delivered),
            "failed" => Ok(Self::Failed),
            "expired" => Ok(Self::Expired),
            "cancelled" => Ok(Self::Cancelled),
            other => Err(IdError::UnknownSource(other.to_owned())),
        }
    }

    /// Whether Animesh still wants this request to exist in the OS.
    pub const fn wants_registration(self) -> bool {
        matches!(self, Self::Desired | Self::Failed | Self::Registered)
    }

    /// Whether no further work can change this job.
    pub const fn is_terminal(self) -> bool {
        matches!(self, Self::Delivered | Self::Expired | Self::Cancelled)
    }
}

/// Whether the request fires on a future trigger or immediately.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DeliveryMode {
    /// A future `UNCalendarNotificationTrigger`.
    Scheduled,
    /// Airtime already passed but is inside the grace window; fires on submit.
    CatchUp,
}

impl DeliveryMode {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Scheduled => "scheduled",
            Self::CatchUp => "catch_up",
        }
    }

    /// Chooses the mode for an airtime relative to `now`, or `None` when the
    /// airtime is too old to be worth surfacing.
    pub fn for_airtime(airtime: UnixTimestamp, now: UnixTimestamp) -> Option<Self> {
        let age = now.get() - airtime.get();
        if age < 0 {
            Some(Self::Scheduled)
        } else if age <= CATCH_UP_GRACE_SECS {
            Some(Self::CatchUp)
        } else {
            None
        }
    }
}

/// Stable per-event key, used as the `notification_jobs` primary key.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct NotificationKey(String);

impl NotificationKey {
    pub fn for_event(uuid: &EventUuid) -> Self {
        Self(uuid.to_string())
    }

    pub fn from_stored(value: impl Into<String>) -> Self {
        Self(value.into())
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

/// The identifier macOS knows a request by.
///
/// Derived from the installation and event UUIDs so it is stable across
/// restarts and reinstalls-with-the-same-database. Stability is what makes
/// re-adding a request idempotent instead of duplicating a banner.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct OsIdentifier(String);

impl OsIdentifier {
    pub fn new(installation: &InstallationUuid, event: &EventUuid) -> Self {
        Self(format!("{OS_IDENTIFIER_PREFIX}{installation}.{event}"))
    }

    pub fn from_stored(value: impl Into<String>) -> Self {
        Self(value.into())
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }

    /// Whether an identifier read back from the OS belongs to this app.
    pub fn is_owned(value: &str) -> bool {
        value.starts_with(OS_IDENTIFIER_PREFIX)
    }
}

/// Builds the canonical AniList URL for a media item.
///
/// The only URL the app will ever open. Constructing it from a validated ID
/// rather than storing a source-provided string means there is no path by which
/// a hostile payload becomes a clickable link.
pub fn anilist_url(id: AniListId) -> String {
    format!("https://anilist.co/anime/{id}")
}

/// Every field that affects what macOS would actually present.
///
/// Serialized to `desired_request_json` and compared by string equality. Field
/// order here is the canonical order; changing it changes every stored value,
/// so treat it as part of the frozen contract.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct NativeRequest {
    pub os_identifier: OsIdentifier,
    pub mode: DeliveryMode,
    pub fire_at: UnixTimestamp,
    pub original_airtime: UnixTimestamp,
    pub title: String,
    pub body: String,
    pub sound: bool,
    pub url: String,
}

impl NativeRequest {
    pub fn build(
        os_identifier: OsIdentifier,
        mode: DeliveryMode,
        airtime: UnixTimestamp,
        title: &str,
        episode: EpisodeNumber,
        anilist_id: AniListId,
    ) -> Self {
        Self {
            os_identifier,
            mode,
            fire_at: airtime,
            original_airtime: airtime,
            title: title.to_owned(),
            body: format!("Episode {episode} is now airing."),
            sound: true,
            url: anilist_url(anilist_id),
        }
    }

    /// Canonical serialization used for change detection.
    ///
    /// `serde_json` emits struct fields in declaration order, so this is stable
    /// for a given struct definition without a canonicalizing pass.
    pub fn canonical_json(&self) -> Result<String, serde_json::Error> {
        serde_json::to_string(self)
    }
}

/// One entry in the desired registration plan.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct NotificationPlanItem {
    pub key: NotificationKey,
    pub release_event_id: ReleaseEventId,
    pub event_uuid: EventUuid,
    pub desired_revision: i64,
    pub request: NativeRequest,
    pub retry_after: Option<UnixTimestamp>,
    pub state: NotificationJobState,
}

impl NotificationPlanItem {
    /// Whether this item may be attempted at `now`, respecting its backoff.
    pub fn is_attemptable(&self, now: UnixTimestamp) -> bool {
        match self.retry_after {
            Some(after) => now.get() >= after.get(),
            None => true,
        }
    }
}

/// What one reconciliation pass concluded about one job.
///
/// Every variant is a statement about what the OS was observed to hold, not
/// about what Animesh asked for. There is no `Attempted`: an add whose result
/// is unknown produces nothing, and the next pass reads the truth instead.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum JobOutcome {
    Registered {
        key: NotificationKey,
        revision: i64,
    },
    Delivered {
        key: NotificationKey,
    },
    Failed {
        key: NotificationKey,
        error_code: String,
        retry_after: UnixTimestamp,
    },
}

impl JobOutcome {
    pub fn key(&self) -> &NotificationKey {
        match self {
            Self::Registered { key, .. } | Self::Delivered { key } | Self::Failed { key, .. } => {
                key
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn at(seconds: i64) -> UnixTimestamp {
        UnixTimestamp::new(seconds).expect("valid timestamp")
    }

    #[test]
    fn job_state_encodings_are_stable() {
        for state in [
            NotificationJobState::Desired,
            NotificationJobState::Registered,
            NotificationJobState::Delivered,
            NotificationJobState::Failed,
            NotificationJobState::Expired,
            NotificationJobState::Cancelled,
        ] {
            assert_eq!(NotificationJobState::parse(state.as_str()), Ok(state));
        }
    }

    #[test]
    fn ambiguous_is_not_a_representable_state() {
        // The state was deliberately removed; a database carrying it is from a
        // schema this build does not understand.
        assert!(NotificationJobState::parse("ambiguous").is_err());
    }

    #[test]
    fn terminal_states_want_no_registration() {
        assert!(NotificationJobState::Delivered.is_terminal());
        assert!(NotificationJobState::Expired.is_terminal());
        assert!(NotificationJobState::Cancelled.is_terminal());
        for state in [
            NotificationJobState::Delivered,
            NotificationJobState::Expired,
            NotificationJobState::Cancelled,
        ] {
            assert!(!state.wants_registration());
        }
    }

    #[test]
    fn failed_jobs_still_want_registration() {
        // A transient add failure must not abandon the episode.
        assert!(NotificationJobState::Failed.wants_registration());
    }

    #[test]
    fn future_airtime_is_scheduled() {
        assert_eq!(
            DeliveryMode::for_airtime(at(2_000), at(1_000)),
            Some(DeliveryMode::Scheduled)
        );
    }

    #[test]
    fn airtime_exactly_now_is_catch_up() {
        assert_eq!(
            DeliveryMode::for_airtime(at(1_000), at(1_000)),
            Some(DeliveryMode::CatchUp)
        );
    }

    #[test]
    fn airtime_at_the_grace_boundary_is_still_catch_up() {
        let airtime = at(1_000);
        let last_moment = at(1_000 + CATCH_UP_GRACE_SECS);
        assert_eq!(
            DeliveryMode::for_airtime(airtime, last_moment),
            Some(DeliveryMode::CatchUp)
        );
        assert_eq!(
            DeliveryMode::for_airtime(airtime, at(1_001 + CATCH_UP_GRACE_SECS)),
            None
        );
    }

    #[test]
    fn os_identifier_is_stable_for_the_same_pair() {
        let installation = InstallationUuid::generate();
        let event = EventUuid::generate();
        assert_eq!(
            OsIdentifier::new(&installation, &event),
            OsIdentifier::new(&installation, &event)
        );
    }

    #[test]
    fn os_identifier_differs_per_event() {
        let installation = InstallationUuid::generate();
        assert_ne!(
            OsIdentifier::new(&installation, &EventUuid::generate()),
            OsIdentifier::new(&installation, &EventUuid::generate())
        );
    }

    #[test]
    fn ownership_check_rejects_foreign_identifiers() {
        let owned = OsIdentifier::new(&InstallationUuid::generate(), &EventUuid::generate());
        assert!(OsIdentifier::is_owned(owned.as_str()));
        assert!(!OsIdentifier::is_owned("com.other.app.reminder"));
        // A near-miss prefix must not be treated as ours; removing another
        // app's pending request would be a visible bug.
        assert!(!OsIdentifier::is_owned("dev.animesh.other.1"));
    }

    #[test]
    fn anilist_url_is_built_from_a_validated_id() {
        let id = AniListId::new(21).expect("valid id");
        assert_eq!(anilist_url(id), "https://anilist.co/anime/21");
    }

    fn request(title: &str, airtime: i64) -> NativeRequest {
        NativeRequest::build(
            OsIdentifier::from_stored("dev.animesh.release.a.b"),
            DeliveryMode::Scheduled,
            at(airtime),
            title,
            EpisodeNumber::new(12).expect("valid episode"),
            AniListId::new(21).expect("valid id"),
        )
    }

    #[test]
    fn canonical_json_is_deterministic() {
        let a = request("One Piece", 1_000);
        let b = request("One Piece", 1_000);
        assert_eq!(
            a.canonical_json().expect("serialize"),
            b.canonical_json().expect("serialize")
        );
    }

    #[test]
    fn canonical_json_changes_when_airtime_moves() {
        // This is the signal that drives re-registration under the same OS id.
        assert_ne!(
            request("One Piece", 1_000)
                .canonical_json()
                .expect("serialize"),
            request("One Piece", 2_000)
                .canonical_json()
                .expect("serialize")
        );
    }

    #[test]
    fn canonical_json_changes_when_title_changes() {
        assert_ne!(
            request("One Piece", 1_000)
                .canonical_json()
                .expect("serialize"),
            request("Two Piece", 1_000)
                .canonical_json()
                .expect("serialize")
        );
    }

    #[test]
    fn canonical_json_fits_the_stored_bound() {
        let long_title = "x".repeat(512);
        let json = request(&long_title, 1_000)
            .canonical_json()
            .expect("serialize");
        assert!(
            json.len() <= MAX_REQUEST_JSON_LEN,
            "canonical json is {} bytes, over the {MAX_REQUEST_JSON_LEN} bound",
            json.len()
        );
    }

    #[test]
    fn body_names_the_episode() {
        assert_eq!(request("One Piece", 1).body, "Episode 12 is now airing.");
    }

    #[test]
    fn retry_deadline_gates_attempts() {
        let item = NotificationPlanItem {
            key: NotificationKey::from_stored("k"),
            release_event_id: ReleaseEventId::new(1).expect("valid id"),
            event_uuid: EventUuid::generate(),
            desired_revision: 1,
            request: request("One Piece", 1_000),
            retry_after: Some(at(500)),
            state: NotificationJobState::Failed,
        };
        assert!(!item.is_attemptable(at(499)));
        assert!(item.is_attemptable(at(500)));
    }
}
