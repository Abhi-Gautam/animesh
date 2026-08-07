//! The pure transition functions from plan section 10.
//!
//! No I/O, no clock, no storage: every input is passed in and every output is a
//! decision the caller writes. Follow and refresh call the same reducers, so
//! there is exactly one place where a schedule change becomes a state change.

use crate::domain::ids::{AniListId, EpisodeNumber, ReleaseEventId, UnixTimestamp};
use crate::domain::media::NextAiring;
use crate::domain::notification::{
    DeliveryMode, NativeRequest, NotificationJobState, OsIdentifier,
};
use crate::domain::release::{FollowState, ReleaseEvent, ReleaseEventState};

// ---------------------------------------------------------------------------
// Observation -> release event
// ---------------------------------------------------------------------------

/// Everything the release reducer needs about current state.
#[derive(Debug, Clone, Copy)]
pub struct ReleaseInputs<'a> {
    /// The one event currently in `scheduled` state, if any.
    pub scheduled: Option<&'a ReleaseEvent>,
    /// An existing event whose key matches the observed episode, in any state.
    pub same_key: Option<&'a ReleaseEvent>,
    /// The highest episode number ever recorded for this source media.
    ///
    /// Separate from `scheduled` so a source that rewinds after an event has
    /// already elapsed is still caught.
    pub latest_sequence: Option<EpisodeNumber>,
    /// The schedule the source just reported. `None` means an explicit null.
    pub observed: Option<NextAiring>,
    pub now: UnixTimestamp,
}

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

pub fn reduce_release(inputs: ReleaseInputs<'_>) -> ReleaseTransition {
    let ReleaseInputs {
        scheduled,
        same_key,
        latest_sequence,
        observed,
        now,
    } = inputs;

    let Some(next) = observed else {
        // An explicit null is the source saying "nothing is scheduled". Whether
        // that retires the current event as elapsed or withdraws it depends on
        // whether its airtime already passed.
        return match scheduled {
            None => ReleaseTransition::NoChange,
            Some(event) if event.has_elapsed(now) => {
                ReleaseTransition::MarkElapsed { event_id: event.id }
            }
            Some(event) => ReleaseTransition::Withdraw { event_id: event.id },
        };
    };

    if let Some(latest) = latest_sequence {
        if next.episode < latest {
            return ReleaseTransition::IntegrityFailure {
                latest,
                observed: next.episode,
            };
        }
    }

    if let Some(current) = scheduled {
        if current.sequence_number == Some(next.episode) {
            return if current.scheduled_at == next.airing_at {
                ReleaseTransition::Touch {
                    event_id: current.id,
                }
            } else {
                ReleaseTransition::Reschedule {
                    event_id: current.id,
                    scheduled_at: next.airing_at,
                }
            };
        }

        // A later episode. Whether the outgoing event elapsed or was superseded
        // is the difference between "it aired" and "it never will".
        let retire_state = if current.has_elapsed(now) {
            ReleaseEventState::Elapsed
        } else {
            ReleaseEventState::Superseded
        };
        return ReleaseTransition::Advance {
            retire: current.id,
            retire_state,
            episode: next.episode,
            scheduled_at: next.airing_at,
        };
    }

    match same_key {
        Some(event) if event.state == ReleaseEventState::Withdrawn => {
            ReleaseTransition::Reinstate {
                event_id: event.id,
                scheduled_at: next.airing_at,
            }
        }
        // An already-elapsed episode the source still reports. Resurrecting it
        // would schedule a second notification for an episode that already
        // aired, so this stays bookkeeping only.
        Some(event) => ReleaseTransition::Touch { event_id: event.id },
        None => ReleaseTransition::Insert {
            episode: next.episode,
            scheduled_at: next.airing_at,
        },
    }
}

// ---------------------------------------------------------------------------
// Follow
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ExistingFollow {
    pub state: FollowState,
    /// Whether a current observation exists to serve from without the network.
    pub has_current_observation: bool,
    pub refresh_after: Option<UnixTimestamp>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FollowPlan {
    /// Nothing is cached, so the source must validate the ID before we commit.
    FetchDetail,
    Reactivate {
        queue_refresh: bool,
    },
    AlreadyActive {
        queue_refresh: bool,
    },
}

pub fn reduce_follow(existing: Option<ExistingFollow>, now: UnixTimestamp) -> FollowPlan {
    let Some(existing) = existing else {
        return FollowPlan::FetchDetail;
    };

    let stale = existing
        .refresh_after
        .is_none_or(|after| now.get() >= after.get());

    match existing.state {
        // A dropped follow with nothing cached cannot be reactivated into a
        // useful state, so it is treated as new.
        FollowState::Dropped if !existing.has_current_observation => FollowPlan::FetchDetail,
        FollowState::Dropped => FollowPlan::Reactivate {
            queue_refresh: stale,
        },
        FollowState::Active => FollowPlan::AlreadyActive {
            queue_refresh: stale,
        },
    }
}

// ---------------------------------------------------------------------------
// Event + follow -> notification job
// ---------------------------------------------------------------------------

/// The stored job as the reducer needs to see it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NotificationJobSnapshot {
    pub state: NotificationJobState,
    pub desired_revision: i64,
    pub desired_request_json: String,
}

#[derive(Debug, Clone)]
pub struct NotificationInputs<'a> {
    pub follow: FollowState,
    pub event: &'a ReleaseEvent,
    pub existing: Option<&'a NotificationJobSnapshot>,
    pub os_identifier: OsIdentifier,
    pub title: &'a str,
    pub anilist_id: AniListId,
    pub now: UnixTimestamp,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum NotificationTransition {
    NoChange,
    Create {
        revision: i64,
        request: Box<NativeRequest>,
    },
    /// The effective request changed before delivery; revision increments and
    /// the same OS identifier is replaced.
    Update {
        revision: i64,
        request: Box<NativeRequest>,
    },
    /// The follow was dropped, or the event was withdrawn or superseded.
    Cancel,
    /// Airtime plus grace passed while it was never registered.
    Expire,
}

/// Builds the request Animesh wants registered, or `None` when it wants none.
fn desired_request(inputs: &NotificationInputs<'_>) -> Option<NativeRequest> {
    if !inputs.follow.is_active() || !inputs.event.state.is_scheduled() {
        return None;
    }
    let episode = inputs.event.sequence_number?;
    let mode = DeliveryMode::for_airtime(inputs.event.scheduled_at, inputs.now)?;

    Some(NativeRequest::build(
        inputs.os_identifier.clone(),
        mode,
        inputs.event.scheduled_at,
        inputs.title,
        episode,
        inputs.anilist_id,
    ))
}

pub fn reduce_notification(
    inputs: &NotificationInputs<'_>,
) -> Result<NotificationTransition, serde_json::Error> {
    // Delivery is terminal for an episode. A later schedule correction must not
    // notify the same episode twice.
    if let Some(existing) = inputs.existing {
        if existing.state == NotificationJobState::Delivered {
            return Ok(NotificationTransition::NoChange);
        }
    }

    let desired = desired_request(inputs);

    match (desired, inputs.existing) {
        (Some(request), None) => Ok(NotificationTransition::Create {
            revision: 1,
            request: Box::new(request),
        }),

        (Some(request), Some(existing)) => {
            let canonical = request.canonical_json()?;
            if canonical == existing.desired_request_json && existing.state.wants_registration() {
                // Unchanged: no write, no plan generation change. This is what
                // keeps an idle reconciliation pass at zero database writes.
                Ok(NotificationTransition::NoChange)
            } else {
                Ok(NotificationTransition::Update {
                    revision: existing.desired_revision + 1,
                    request: Box::new(request),
                })
            }
        }

        (None, None) => Ok(NotificationTransition::NoChange),

        (None, Some(existing)) if existing.state.is_terminal() => {
            Ok(NotificationTransition::NoChange)
        }

        (None, Some(_)) => {
            // Distinguish "we no longer want this" from "we wanted it and ran
            // out of time": the first is a cancellation, the second an expiry,
            // and health reports them differently.
            let no_longer_wanted = !inputs.follow.is_active() || !inputs.event.state.is_scheduled();
            if no_longer_wanted {
                Ok(NotificationTransition::Cancel)
            } else {
                Ok(NotificationTransition::Expire)
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    use crate::domain::ids::{EventUuid, MediaId, ObservationId, SourceMediaId};
    use crate::domain::notification::CATCH_UP_GRACE_SECS;
    use crate::domain::release::source_event_key;

    fn at(seconds: i64) -> UnixTimestamp {
        UnixTimestamp::new(seconds).expect("valid timestamp")
    }

    fn ep(number: i64) -> EpisodeNumber {
        EpisodeNumber::new(number).expect("valid episode")
    }

    fn airing(episode: i64, airing_at: i64) -> NextAiring {
        NextAiring {
            episode: ep(episode),
            airing_at: at(airing_at),
        }
    }

    fn event(id: i64, episode: i64, scheduled_at: i64, state: ReleaseEventState) -> ReleaseEvent {
        ReleaseEvent {
            id: ReleaseEventId::new(id).expect("valid id"),
            uuid: EventUuid::generate(),
            media_id: MediaId::new(1).expect("valid id"),
            source_media_id: SourceMediaId::new(1).expect("valid id"),
            source_event_key: source_event_key(ep(episode)).expect("valid key"),
            sequence_number: Some(ep(episode)),
            scheduled_at: at(scheduled_at),
            state,
            schedule_revision: 1,
            first_observed_at: at(0),
            last_observed_at: at(0),
            last_observation_id: ObservationId::new(1).expect("valid id"),
        }
    }

    fn inputs<'a>(
        scheduled: Option<&'a ReleaseEvent>,
        same_key: Option<&'a ReleaseEvent>,
        latest_sequence: Option<EpisodeNumber>,
        observed: Option<NextAiring>,
        now: i64,
    ) -> ReleaseInputs<'a> {
        ReleaseInputs {
            scheduled,
            same_key,
            latest_sequence,
            observed,
            now: at(now),
        }
    }

    // --- Section 10, observation -> release event, row by row ---

    #[test]
    fn no_event_plus_a_schedule_inserts() {
        assert_eq!(
            reduce_release(inputs(None, None, None, Some(airing(1, 5_000)), 1_000)),
            ReleaseTransition::Insert {
                episode: ep(1),
                scheduled_at: at(5_000),
            }
        );
    }

    #[test]
    fn same_episode_same_time_only_touches() {
        // Must not bump the schedule revision: that would re-register the
        // notification on every refresh.
        let current = event(1, 5, 5_000, ReleaseEventState::Scheduled);
        assert_eq!(
            reduce_release(inputs(
                Some(&current),
                Some(&current),
                Some(ep(5)),
                Some(airing(5, 5_000)),
                1_000
            )),
            ReleaseTransition::Touch {
                event_id: current.id
            }
        );
    }

    #[test]
    fn same_episode_new_time_reschedules() {
        let current = event(1, 5, 5_000, ReleaseEventState::Scheduled);
        assert_eq!(
            reduce_release(inputs(
                Some(&current),
                Some(&current),
                Some(ep(5)),
                Some(airing(5, 6_000)),
                1_000
            )),
            ReleaseTransition::Reschedule {
                event_id: current.id,
                scheduled_at: at(6_000),
            }
        );
    }

    #[test]
    fn a_withdrawn_event_that_reappears_is_reinstated() {
        let withdrawn = event(1, 5, 5_000, ReleaseEventState::Withdrawn);
        assert_eq!(
            reduce_release(inputs(
                None,
                Some(&withdrawn),
                Some(ep(5)),
                Some(airing(5, 5_500)),
                1_000
            )),
            ReleaseTransition::Reinstate {
                event_id: withdrawn.id,
                scheduled_at: at(5_500),
            }
        );
    }

    #[test]
    fn a_later_episode_after_airtime_retires_the_old_one_as_elapsed() {
        let current = event(1, 5, 5_000, ReleaseEventState::Scheduled);
        assert_eq!(
            reduce_release(inputs(
                Some(&current),
                None,
                Some(ep(5)),
                Some(airing(6, 12_000)),
                6_000
            )),
            ReleaseTransition::Advance {
                retire: current.id,
                retire_state: ReleaseEventState::Elapsed,
                episode: ep(6),
                scheduled_at: at(12_000),
            }
        );
    }

    #[test]
    fn a_later_episode_before_airtime_supersedes_the_old_one() {
        // The old episode never aired, so calling it elapsed would be a lie the
        // health snapshot would repeat.
        let current = event(1, 5, 5_000, ReleaseEventState::Scheduled);
        assert_eq!(
            reduce_release(inputs(
                Some(&current),
                None,
                Some(ep(5)),
                Some(airing(6, 12_000)),
                1_000
            )),
            ReleaseTransition::Advance {
                retire: current.id,
                retire_state: ReleaseEventState::Superseded,
                episode: ep(6),
                scheduled_at: at(12_000),
            }
        );
    }

    #[test]
    fn an_earlier_episode_is_an_integrity_failure() {
        let current = event(1, 5, 5_000, ReleaseEventState::Scheduled);
        assert_eq!(
            reduce_release(inputs(
                Some(&current),
                None,
                Some(ep(5)),
                Some(airing(4, 4_000)),
                1_000
            )),
            ReleaseTransition::IntegrityFailure {
                latest: ep(5),
                observed: ep(4),
            }
        );
    }

    #[test]
    fn rewinding_after_an_event_elapsed_is_still_an_integrity_failure() {
        // No scheduled event exists here, so only latest_sequence catches it.
        let elapsed = event(1, 5, 5_000, ReleaseEventState::Elapsed);
        assert_eq!(
            reduce_release(inputs(
                None,
                None,
                Some(ep(5)),
                Some(airing(3, 9_000)),
                6_000
            )),
            ReleaseTransition::IntegrityFailure {
                latest: ep(5),
                observed: ep(3),
            }
        );
        let _ = elapsed;
    }

    #[test]
    fn explicit_null_on_a_future_event_withdraws_it() {
        let current = event(1, 5, 5_000, ReleaseEventState::Scheduled);
        assert_eq!(
            reduce_release(inputs(Some(&current), None, Some(ep(5)), None, 1_000)),
            ReleaseTransition::Withdraw {
                event_id: current.id
            }
        );
    }

    #[test]
    fn explicit_null_after_airtime_marks_it_elapsed() {
        let current = event(1, 5, 5_000, ReleaseEventState::Scheduled);
        assert_eq!(
            reduce_release(inputs(Some(&current), None, Some(ep(5)), None, 5_000)),
            ReleaseTransition::MarkElapsed {
                event_id: current.id
            }
        );
    }

    #[test]
    fn explicit_null_with_no_event_does_nothing() {
        assert_eq!(
            reduce_release(inputs(None, None, None, None, 1_000)),
            ReleaseTransition::NoChange
        );
    }

    #[test]
    fn an_elapsed_event_the_source_keeps_reporting_is_not_resurrected() {
        // The post-air case. Re-scheduling would notify an aired episode twice.
        let elapsed = event(1, 5, 5_000, ReleaseEventState::Elapsed);
        assert_eq!(
            reduce_release(inputs(
                None,
                Some(&elapsed),
                Some(ep(5)),
                Some(airing(5, 5_000)),
                9_000
            )),
            ReleaseTransition::Touch {
                event_id: elapsed.id
            }
        );
    }

    // --- Section 10, follow ---

    #[test]
    fn an_unseen_id_must_be_validated_by_the_source() {
        assert_eq!(reduce_follow(None, at(1_000)), FollowPlan::FetchDetail);
    }

    #[test]
    fn an_active_fresh_follow_makes_no_request() {
        let existing = ExistingFollow {
            state: FollowState::Active,
            has_current_observation: true,
            refresh_after: Some(at(9_000)),
        };
        assert_eq!(
            reduce_follow(Some(existing), at(1_000)),
            FollowPlan::AlreadyActive {
                queue_refresh: false
            }
        );
    }

    #[test]
    fn an_active_stale_follow_queues_a_refresh() {
        let existing = ExistingFollow {
            state: FollowState::Active,
            has_current_observation: true,
            refresh_after: Some(at(500)),
        };
        assert_eq!(
            reduce_follow(Some(existing), at(1_000)),
            FollowPlan::AlreadyActive {
                queue_refresh: true
            }
        );
    }

    #[test]
    fn a_dropped_follow_with_cached_data_reactivates_without_the_network() {
        let existing = ExistingFollow {
            state: FollowState::Dropped,
            has_current_observation: true,
            refresh_after: Some(at(9_000)),
        };
        assert_eq!(
            reduce_follow(Some(existing), at(1_000)),
            FollowPlan::Reactivate {
                queue_refresh: false
            }
        );
    }

    #[test]
    fn a_dropped_follow_without_cached_data_needs_a_fetch() {
        let existing = ExistingFollow {
            state: FollowState::Dropped,
            has_current_observation: false,
            refresh_after: None,
        };
        assert_eq!(
            reduce_follow(Some(existing), at(1_000)),
            FollowPlan::FetchDetail
        );
    }

    #[test]
    fn a_never_refreshed_follow_counts_as_stale() {
        let existing = ExistingFollow {
            state: FollowState::Active,
            has_current_observation: true,
            refresh_after: None,
        };
        assert_eq!(
            reduce_follow(Some(existing), at(1_000)),
            FollowPlan::AlreadyActive {
                queue_refresh: true
            }
        );
    }

    // --- Section 10, notification job ---

    fn notification_inputs<'a>(
        follow: FollowState,
        event: &'a ReleaseEvent,
        existing: Option<&'a NotificationJobSnapshot>,
        now: i64,
    ) -> NotificationInputs<'a> {
        NotificationInputs {
            follow,
            event,
            existing,
            os_identifier: OsIdentifier::from_stored("dev.animesh.release.install.event"),
            title: "One Piece",
            anilist_id: AniListId::new(21).expect("valid id"),
            now: at(now),
        }
    }

    fn snapshot(state: NotificationJobState, revision: i64, json: &str) -> NotificationJobSnapshot {
        NotificationJobSnapshot {
            state,
            desired_revision: revision,
            desired_request_json: json.to_owned(),
        }
    }

    fn canonical_for(event: &ReleaseEvent, now: i64) -> String {
        let inputs = notification_inputs(FollowState::Active, event, None, now);
        desired_request(&inputs)
            .expect("desired request")
            .canonical_json()
            .expect("serialize")
    }

    #[test]
    fn a_new_scheduled_event_creates_a_job_at_revision_one() {
        let scheduled = event(1, 5, 5_000, ReleaseEventState::Scheduled);
        let inputs = notification_inputs(FollowState::Active, &scheduled, None, 1_000);

        match reduce_notification(&inputs).expect("reduce") {
            NotificationTransition::Create { revision, request } => {
                assert_eq!(revision, 1);
                assert_eq!(request.mode, DeliveryMode::Scheduled);
                assert_eq!(request.body, "Episode 5 is now airing.");
            }
            other => panic!("expected a create, got {other:?}"),
        }
    }

    #[test]
    fn an_unchanged_request_is_a_no_op() {
        // Zero writes on an idle pass depends on this exact case.
        let scheduled = event(1, 5, 5_000, ReleaseEventState::Scheduled);
        let json = canonical_for(&scheduled, 1_000);
        let existing = snapshot(NotificationJobState::Registered, 1, &json);
        let inputs = notification_inputs(FollowState::Active, &scheduled, Some(&existing), 1_000);

        assert_eq!(
            reduce_notification(&inputs).expect("reduce"),
            NotificationTransition::NoChange
        );
    }

    #[test]
    fn a_moved_airtime_updates_and_increments_the_revision() {
        let original = event(1, 5, 5_000, ReleaseEventState::Scheduled);
        let json = canonical_for(&original, 1_000);
        let existing = snapshot(NotificationJobState::Registered, 3, &json);

        let moved = event(1, 5, 6_000, ReleaseEventState::Scheduled);
        let inputs = notification_inputs(FollowState::Active, &moved, Some(&existing), 1_000);

        match reduce_notification(&inputs).expect("reduce") {
            NotificationTransition::Update { revision, request } => {
                assert_eq!(revision, 4);
                assert_eq!(request.fire_at, at(6_000));
            }
            other => panic!("expected an update, got {other:?}"),
        }
    }

    #[test]
    fn a_dropped_follow_cancels_an_active_job() {
        let scheduled = event(1, 5, 5_000, ReleaseEventState::Scheduled);
        let json = canonical_for(&scheduled, 1_000);
        let existing = snapshot(NotificationJobState::Registered, 1, &json);
        let inputs = notification_inputs(FollowState::Dropped, &scheduled, Some(&existing), 1_000);

        assert_eq!(
            reduce_notification(&inputs).expect("reduce"),
            NotificationTransition::Cancel
        );
    }

    #[test]
    fn a_withdrawn_event_cancels_its_job() {
        let withdrawn = event(1, 5, 5_000, ReleaseEventState::Withdrawn);
        let existing = snapshot(NotificationJobState::Registered, 1, "{}");
        let inputs = notification_inputs(FollowState::Active, &withdrawn, Some(&existing), 1_000);

        assert_eq!(
            reduce_notification(&inputs).expect("reduce"),
            NotificationTransition::Cancel
        );
    }

    #[test]
    fn an_unregistered_job_past_the_grace_window_expires() {
        let scheduled = event(1, 5, 5_000, ReleaseEventState::Scheduled);
        let existing = snapshot(NotificationJobState::Desired, 1, "{}");
        let too_late = 5_000 + CATCH_UP_GRACE_SECS + 1;
        let inputs =
            notification_inputs(FollowState::Active, &scheduled, Some(&existing), too_late);

        assert_eq!(
            reduce_notification(&inputs).expect("reduce"),
            NotificationTransition::Expire
        );
    }

    #[test]
    fn inside_the_grace_window_it_is_a_catch_up_not_an_expiry() {
        let scheduled = event(1, 5, 5_000, ReleaseEventState::Scheduled);
        let existing = snapshot(NotificationJobState::Desired, 1, "{}");
        let just_late = 5_000 + CATCH_UP_GRACE_SECS;
        let inputs =
            notification_inputs(FollowState::Active, &scheduled, Some(&existing), just_late);

        match reduce_notification(&inputs).expect("reduce") {
            NotificationTransition::Update { request, .. } => {
                assert_eq!(request.mode, DeliveryMode::CatchUp);
                // The original airtime survives so the banner can say when it
                // actually aired rather than when we got around to it.
                assert_eq!(request.original_airtime, at(5_000));
            }
            other => panic!("expected a catch-up update, got {other:?}"),
        }
    }

    #[test]
    fn delivery_is_terminal_for_an_episode() {
        // A later schedule correction must not produce a second banner.
        let scheduled = event(1, 5, 5_000, ReleaseEventState::Scheduled);
        let existing = snapshot(NotificationJobState::Delivered, 1, "{}");
        let inputs = notification_inputs(FollowState::Active, &scheduled, Some(&existing), 1_000);

        assert_eq!(
            reduce_notification(&inputs).expect("reduce"),
            NotificationTransition::NoChange
        );

        let moved = event(1, 5, 9_000, ReleaseEventState::Scheduled);
        let inputs = notification_inputs(FollowState::Active, &moved, Some(&existing), 1_000);
        assert_eq!(
            reduce_notification(&inputs).expect("reduce"),
            NotificationTransition::NoChange
        );
    }

    #[test]
    fn a_failed_job_with_an_unchanged_request_stays_a_no_op() {
        // Retry timing is the reconciler's business; the reducer must not churn
        // the desired revision just because an attempt failed.
        let scheduled = event(1, 5, 5_000, ReleaseEventState::Scheduled);
        let json = canonical_for(&scheduled, 1_000);
        let existing = snapshot(NotificationJobState::Failed, 2, &json);
        let inputs = notification_inputs(FollowState::Active, &scheduled, Some(&existing), 1_000);

        assert_eq!(
            reduce_notification(&inputs).expect("reduce"),
            NotificationTransition::NoChange
        );
    }

    #[test]
    fn a_cancelled_job_whose_follow_returns_is_revived_by_an_update() {
        let scheduled = event(1, 5, 5_000, ReleaseEventState::Scheduled);
        let json = canonical_for(&scheduled, 1_000);
        let existing = snapshot(NotificationJobState::Cancelled, 2, &json);
        let inputs = notification_inputs(FollowState::Active, &scheduled, Some(&existing), 1_000);

        match reduce_notification(&inputs).expect("reduce") {
            NotificationTransition::Update { revision, .. } => assert_eq!(revision, 3),
            other => panic!("expected a revival update, got {other:?}"),
        }
    }

    #[test]
    fn an_event_without_an_episode_number_produces_no_job() {
        let mut scheduled = event(1, 5, 5_000, ReleaseEventState::Scheduled);
        scheduled.sequence_number = None;
        let inputs = notification_inputs(FollowState::Active, &scheduled, None, 1_000);

        assert_eq!(
            reduce_notification(&inputs).expect("reduce"),
            NotificationTransition::NoChange
        );
    }

    #[test]
    fn no_job_and_nothing_wanted_is_a_no_op() {
        let withdrawn = event(1, 5, 5_000, ReleaseEventState::Withdrawn);
        let inputs = notification_inputs(FollowState::Active, &withdrawn, None, 1_000);
        assert_eq!(
            reduce_notification(&inputs).expect("reduce"),
            NotificationTransition::NoChange
        );
    }
}
