//! Projected release events — the Gold layer's schedule facts.

use serde::{Deserialize, Serialize};

use super::ids::{
    BoundedText, EpisodeNumber, EventUuid, IdError, MediaId, ObservationId, ReleaseEventId,
    SourceMediaId, UnixTimestamp,
};

/// Maximum stored length of a source event key, matching the `V0001` schema.
pub const MAX_EVENT_KEY_LEN: usize = 128;

/// Lifecycle of a single projected release.
///
/// Only `Scheduled` events are eligible for notification, and the schema
/// enforces at most one per source media via a partial unique index.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ReleaseEventState {
    Scheduled,
    /// Its airtime passed while it was still the source's current event.
    Elapsed,
    /// The source explicitly stopped reporting it before its airtime.
    Withdrawn,
    /// A later episode replaced it before its airtime arrived.
    Superseded,
}

impl ReleaseEventState {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Scheduled => "scheduled",
            Self::Elapsed => "elapsed",
            Self::Withdrawn => "withdrawn",
            Self::Superseded => "superseded",
        }
    }

    pub fn parse(value: &str) -> Result<Self, IdError> {
        match value {
            "scheduled" => Ok(Self::Scheduled),
            "elapsed" => Ok(Self::Elapsed),
            "withdrawn" => Ok(Self::Withdrawn),
            "superseded" => Ok(Self::Superseded),
            other => Err(IdError::UnknownSource(other.to_owned())),
        }
    }

    pub const fn is_scheduled(self) -> bool {
        matches!(self, Self::Scheduled)
    }
}

/// Identity of an event within its source media.
///
/// AniList exposes only the next episode, so episode number is the only stable
/// handle available. Deriving the key rather than using the episode number
/// directly leaves room for sources that key events differently, without
/// changing the schema.
pub fn source_event_key(episode: EpisodeNumber) -> Result<BoundedText, IdError> {
    BoundedText::new(
        "source_event_key",
        MAX_EVENT_KEY_LEN,
        format!("ep:{episode}"),
    )
}

/// A projected release event.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ReleaseEvent {
    pub id: ReleaseEventId,
    pub uuid: EventUuid,
    pub media_id: MediaId,
    pub source_media_id: SourceMediaId,
    pub source_event_key: BoundedText,
    pub sequence_number: Option<EpisodeNumber>,
    pub scheduled_at: UnixTimestamp,
    pub state: ReleaseEventState,
    /// Incremented whenever the scheduled instant changes, never on a no-op
    /// re-observation. This is what drives notification re-registration.
    pub schedule_revision: i64,
    pub first_observed_at: UnixTimestamp,
    pub last_observed_at: UnixTimestamp,
    pub last_observation_id: ObservationId,
}

impl ReleaseEvent {
    /// Whether this event's airtime has arrived as of `now`.
    pub const fn has_elapsed(&self, now: UnixTimestamp) -> bool {
        self.scheduled_at.get() <= now.get()
    }
}

/// What one observation decided about a source media's schedule.
///
/// Produced by the release reducer and consumed by the store, so it lives here
/// rather than beside either: a decision type owned by the layer that computes
/// it would force the layer that writes it to depend upward.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ReleaseTransition {
    NoChange,
    /// The source re-asserted an event unchanged; only observation bookkeeping.
    Touch {
        event_id: ReleaseEventId,
    },
    Insert {
        episode: EpisodeNumber,
        scheduled_at: UnixTimestamp,
    },
    /// Same episode at a new instant; schedule revision increments.
    Reschedule {
        event_id: ReleaseEventId,
        scheduled_at: UnixTimestamp,
    },
    /// A withdrawn event reappeared; schedule revision increments.
    Reinstate {
        event_id: ReleaseEventId,
        scheduled_at: UnixTimestamp,
    },
    /// Retire the current event and open the next one.
    Advance {
        retire: ReleaseEventId,
        retire_state: ReleaseEventState,
        episode: EpisodeNumber,
        scheduled_at: UnixTimestamp,
    },
    /// The source stopped reporting a still-future event.
    Withdraw {
        event_id: ReleaseEventId,
    },
    MarkElapsed {
        event_id: ReleaseEventId,
    },
    /// The source reported an episode earlier than one already recorded.
    IntegrityFailure {
        latest: EpisodeNumber,
        observed: EpisodeNumber,
    },
}

/// Whether a follow is active or has been dropped.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FollowState {
    Active,
    Dropped,
}

impl FollowState {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Active => "active",
            Self::Dropped => "dropped",
        }
    }

    pub fn parse(value: &str) -> Result<Self, IdError> {
        match value {
            "active" => Ok(Self::Active),
            "dropped" => Ok(Self::Dropped),
            other => Err(IdError::UnknownSource(other.to_owned())),
        }
    }

    pub const fn is_active(self) -> bool {
        matches!(self, Self::Active)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn event_state_encodings_are_stable() {
        for state in [
            ReleaseEventState::Scheduled,
            ReleaseEventState::Elapsed,
            ReleaseEventState::Withdrawn,
            ReleaseEventState::Superseded,
        ] {
            assert_eq!(ReleaseEventState::parse(state.as_str()), Ok(state));
        }
        assert!(ReleaseEventState::parse("pending").is_err());
    }

    #[test]
    fn follow_state_encodings_are_stable() {
        for state in [FollowState::Active, FollowState::Dropped] {
            assert_eq!(FollowState::parse(state.as_str()), Ok(state));
        }
        assert!(FollowState::parse("paused").is_err());
    }

    #[test]
    fn source_event_key_is_derived_from_episode() {
        let key =
            source_event_key(EpisodeNumber::new(1169).expect("valid episode")).expect("valid key");
        assert_eq!(key.as_str(), "ep:1169");
    }

    #[test]
    fn source_event_key_fits_the_schema_bound_at_the_extreme() {
        // The largest episode number a source can report must still produce a
        // key inside the column bound.
        let key = source_event_key(EpisodeNumber::new(EpisodeNumber::MAX).expect("valid episode"))
            .expect("valid key");
        assert!(key.as_str().len() <= MAX_EVENT_KEY_LEN);
    }

    #[test]
    fn elapsed_is_inclusive_of_the_scheduled_instant() {
        // At exactly the airtime the episode is airing, not still upcoming.
        let at = |s| UnixTimestamp::new(s).expect("valid timestamp");
        let event = ReleaseEvent {
            id: ReleaseEventId::new(1).expect("valid id"),
            uuid: EventUuid::generate(),
            media_id: MediaId::new(1).expect("valid id"),
            source_media_id: SourceMediaId::new(1).expect("valid id"),
            source_event_key: BoundedText::new("k", 128, "ep:1").expect("valid key"),
            sequence_number: Some(EpisodeNumber::new(1).expect("valid episode")),
            scheduled_at: at(1_000),
            state: ReleaseEventState::Scheduled,
            schedule_revision: 1,
            first_observed_at: at(900),
            last_observed_at: at(900),
            last_observation_id: ObservationId::new(1).expect("valid id"),
        };
        assert!(!event.has_elapsed(at(999)));
        assert!(event.has_elapsed(at(1_000)));
        assert!(event.has_elapsed(at(1_001)));
    }
}
