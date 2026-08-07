//! Release events and notification jobs.

use rusqlite::{Connection, OptionalExtension, Row, Transaction};

use super::connection::StoreError;
use crate::domain::ids::{
    BoundedText, EpisodeNumber, EventUuid, MediaId, ObservationId, ReleaseEventId, SourceMediaId,
    UnixTimestamp,
};
use crate::domain::notification::{
    NativeRequest, NotificationJobState, NotificationKey, OsIdentifier,
};
use crate::domain::release::{
    source_event_key, ReleaseEvent, ReleaseEventState, MAX_EVENT_KEY_LEN,
};
use crate::library::reducers::{
    NotificationJobSnapshot, NotificationTransition, ReleaseTransition,
};

const EVENT_COLUMNS: &str = "release_event_id, event_uuid, media_id, source_media_id,
     source_event_key, sequence_number, scheduled_at, state, schedule_revision,
     first_observed_at, last_observed_at, last_observation_id";

fn ts(value: i64) -> Result<UnixTimestamp, StoreError> {
    UnixTimestamp::new(value).map_err(|e| StoreError::Integrity(e.to_string()))
}

/// The raw column tuple for [`EVENT_COLUMNS`], in order.
type EventRow = (
    i64,
    String,
    i64,
    i64,
    String,
    Option<i64>,
    i64,
    String,
    i64,
    i64,
    i64,
    i64,
);

fn row_to_event(row: &Row<'_>) -> rusqlite::Result<EventRow> {
    Ok((
        row.get(0)?,
        row.get(1)?,
        row.get(2)?,
        row.get(3)?,
        row.get(4)?,
        row.get(5)?,
        row.get(6)?,
        row.get(7)?,
        row.get(8)?,
        row.get(9)?,
        row.get(10)?,
        row.get(11)?,
    ))
}

fn build_event(raw: EventRow) -> Result<ReleaseEvent, StoreError> {
    let (
        id,
        uuid,
        media_id,
        source_media_id,
        key,
        sequence,
        scheduled_at,
        state,
        revision,
        first_seen,
        last_seen,
        observation,
    ) = raw;

    let bad = |e: String| StoreError::Integrity(e);
    Ok(ReleaseEvent {
        id: ReleaseEventId::new(id).map_err(|e| bad(e.to_string()))?,
        uuid: EventUuid::from_uuid(uuid.parse().map_err(|e| bad(format!("event uuid: {e}")))?),
        media_id: MediaId::new(media_id).map_err(|e| bad(e.to_string()))?,
        source_media_id: SourceMediaId::new(source_media_id).map_err(|e| bad(e.to_string()))?,
        source_event_key: BoundedText::truncating(MAX_EVENT_KEY_LEN, &key)
            .ok_or_else(|| bad("empty event key".into()))?,
        sequence_number: sequence
            .map(EpisodeNumber::new)
            .transpose()
            .map_err(|e| bad(e.to_string()))?,
        scheduled_at: ts(scheduled_at)?,
        state: ReleaseEventState::parse(&state).map_err(|e| bad(e.to_string()))?,
        schedule_revision: revision,
        first_observed_at: ts(first_seen)?,
        last_observed_at: ts(last_seen)?,
        last_observation_id: ObservationId::new(observation).map_err(|e| bad(e.to_string()))?,
    })
}

pub fn scheduled_event(
    conn: &Connection,
    source_media_id: SourceMediaId,
) -> Result<Option<ReleaseEvent>, StoreError> {
    conn.query_row(
        &format!(
            "SELECT {EVENT_COLUMNS} FROM release_events
             WHERE source_media_id = ?1 AND state = 'scheduled'"
        ),
        [source_media_id.get()],
        row_to_event,
    )
    .optional()?
    .map(build_event)
    .transpose()
}

pub fn event_by_key(
    conn: &Connection,
    source_media_id: SourceMediaId,
    key: &str,
) -> Result<Option<ReleaseEvent>, StoreError> {
    conn.query_row(
        &format!(
            "SELECT {EVENT_COLUMNS} FROM release_events
             WHERE source_media_id = ?1 AND source_event_key = ?2"
        ),
        rusqlite::params![source_media_id.get(), key],
        row_to_event,
    )
    .optional()?
    .map(build_event)
    .transpose()
}

pub fn event_by_id(
    conn: &Connection,
    event_id: ReleaseEventId,
) -> Result<Option<ReleaseEvent>, StoreError> {
    conn.query_row(
        &format!("SELECT {EVENT_COLUMNS} FROM release_events WHERE release_event_id = ?1"),
        [event_id.get()],
        row_to_event,
    )
    .optional()?
    .map(build_event)
    .transpose()
}

/// The highest episode number ever recorded, in any state.
pub fn latest_sequence(
    conn: &Connection,
    source_media_id: SourceMediaId,
) -> Result<Option<EpisodeNumber>, StoreError> {
    conn.query_row(
        "SELECT max(sequence_number) FROM release_events WHERE source_media_id = ?1",
        [source_media_id.get()],
        |row| row.get::<_, Option<i64>>(0),
    )
    .optional()?
    .flatten()
    .map(EpisodeNumber::new)
    .transpose()
    .map_err(|e| StoreError::Integrity(e.to_string()))
}

fn set_state(
    tx: &Transaction<'_>,
    event_id: ReleaseEventId,
    state: ReleaseEventState,
    bump_revision: bool,
    observation_id: ObservationId,
    now: UnixTimestamp,
) -> Result<(), StoreError> {
    tx.execute(
        "UPDATE release_events
         SET state = ?1,
             schedule_revision = schedule_revision + ?2,
             last_observed_at = ?3,
             last_observation_id = ?4
         WHERE release_event_id = ?5",
        rusqlite::params![
            state.as_str(),
            i64::from(bump_revision),
            now.get(),
            observation_id.get(),
            event_id.get()
        ],
    )?;
    Ok(())
}

fn insert_event(
    tx: &Transaction<'_>,
    source_media_id: SourceMediaId,
    media_id: MediaId,
    episode: EpisodeNumber,
    scheduled_at: UnixTimestamp,
    observation_id: ObservationId,
    now: UnixTimestamp,
) -> Result<ReleaseEventId, StoreError> {
    let key = source_event_key(episode).map_err(|e| StoreError::Integrity(e.to_string()))?;
    tx.execute(
        "INSERT INTO release_events
            (event_uuid, media_id, source_media_id, source_event_key, sequence_number,
             scheduled_at, state, schedule_revision, first_observed_at, last_observed_at,
             last_observation_id)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, 'scheduled', 1, ?7, ?7, ?8)",
        rusqlite::params![
            EventUuid::generate().to_string(),
            media_id.get(),
            source_media_id.get(),
            key.as_str(),
            episode.get(),
            scheduled_at.get(),
            now.get(),
            observation_id.get(),
        ],
    )?;
    ReleaseEventId::new(tx.last_insert_rowid()).map_err(|e| StoreError::Integrity(e.to_string()))
}

/// Writes one release transition.
///
/// Returns the event the transition left in `scheduled` state, if any, so the
/// caller can run the notification reducer against it without re-querying.
pub fn apply_transition(
    tx: &Transaction<'_>,
    source_media_id: SourceMediaId,
    media_id: MediaId,
    observation_id: ObservationId,
    transition: ReleaseTransition,
    now: UnixTimestamp,
) -> Result<Option<ReleaseEventId>, StoreError> {
    match transition {
        ReleaseTransition::NoChange | ReleaseTransition::IntegrityFailure { .. } => Ok(None),

        ReleaseTransition::Touch { event_id } => {
            tx.execute(
                "UPDATE release_events
                 SET last_observed_at = ?1, last_observation_id = ?2
                 WHERE release_event_id = ?3",
                rusqlite::params![now.get(), observation_id.get(), event_id.get()],
            )?;
            Ok(Some(event_id))
        }

        ReleaseTransition::Insert {
            episode,
            scheduled_at,
        } => Ok(Some(insert_event(
            tx,
            source_media_id,
            media_id,
            episode,
            scheduled_at,
            observation_id,
            now,
        )?)),

        ReleaseTransition::Reschedule {
            event_id,
            scheduled_at,
        }
        | ReleaseTransition::Reinstate {
            event_id,
            scheduled_at,
        } => {
            tx.execute(
                "UPDATE release_events
                 SET scheduled_at = ?1, state = 'scheduled',
                     schedule_revision = schedule_revision + 1,
                     last_observed_at = ?2, last_observation_id = ?3
                 WHERE release_event_id = ?4",
                rusqlite::params![
                    scheduled_at.get(),
                    now.get(),
                    observation_id.get(),
                    event_id.get()
                ],
            )?;
            Ok(Some(event_id))
        }

        ReleaseTransition::Advance {
            retire,
            retire_state,
            episode,
            scheduled_at,
        } => {
            // Retire first: the partial unique index permits only one scheduled
            // event per source media, so inserting first would collide.
            set_state(tx, retire, retire_state, false, observation_id, now)?;
            Ok(Some(insert_event(
                tx,
                source_media_id,
                media_id,
                episode,
                scheduled_at,
                observation_id,
                now,
            )?))
        }

        ReleaseTransition::Withdraw { event_id } => {
            set_state(
                tx,
                event_id,
                ReleaseEventState::Withdrawn,
                true,
                observation_id,
                now,
            )?;
            Ok(None)
        }

        ReleaseTransition::MarkElapsed { event_id } => {
            set_state(
                tx,
                event_id,
                ReleaseEventState::Elapsed,
                false,
                observation_id,
                now,
            )?;
            Ok(None)
        }
    }
}

// ---------------------------------------------------------------------------
// notification jobs
// ---------------------------------------------------------------------------

pub fn job_for_event(
    conn: &Connection,
    event_id: ReleaseEventId,
) -> Result<Option<NotificationJobSnapshot>, StoreError> {
    conn.query_row(
        "SELECT state, desired_revision, desired_request_json
         FROM notification_jobs WHERE release_event_id = ?1",
        [event_id.get()],
        |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, i64>(1)?,
                row.get::<_, String>(2)?,
            ))
        },
    )
    .optional()?
    .map(|(state, revision, json)| {
        Ok(NotificationJobSnapshot {
            state: NotificationJobState::parse(&state)
                .map_err(|e| StoreError::Integrity(e.to_string()))?,
            desired_revision: revision,
            desired_request_json: json,
        })
    })
    .transpose()
}

/// Writes one notification transition.
///
/// Returns whether the effective desired set changed, which is what the caller
/// uses to decide if the in-memory plan generation should move.
pub fn apply_notification(
    tx: &Transaction<'_>,
    event: &ReleaseEvent,
    transition: &NotificationTransition,
    now: UnixTimestamp,
) -> Result<bool, StoreError> {
    match transition {
        NotificationTransition::NoChange => Ok(false),

        NotificationTransition::Create { revision, request } => {
            let key = NotificationKey::for_event(&event.uuid);
            let json = request
                .canonical_json()
                .map_err(|e| StoreError::Integrity(e.to_string()))?;
            tx.execute(
                "INSERT INTO notification_jobs
                    (notification_key, release_event_id, os_identifier, desired_at,
                     desired_revision, desired_request_json, state, attempt_count, updated_at)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, 'desired', 0, ?7)",
                rusqlite::params![
                    key.as_str(),
                    event.id.get(),
                    request.os_identifier.as_str(),
                    request.fire_at.get(),
                    revision,
                    json,
                    now.get()
                ],
            )?;
            Ok(true)
        }

        NotificationTransition::Update { revision, request } => {
            let json = request
                .canonical_json()
                .map_err(|e| StoreError::Integrity(e.to_string()))?;
            // Back to `desired`: the OS holds a stale request, or none.
            // registered_revision is cleared so the CHECK stays satisfied.
            tx.execute(
                "UPDATE notification_jobs
                 SET desired_at = ?1, desired_revision = ?2, desired_request_json = ?3,
                     state = 'desired', registered_revision = NULL, registered_at = NULL,
                     retry_after = NULL, updated_at = ?4
                 WHERE release_event_id = ?5",
                rusqlite::params![
                    request.fire_at.get(),
                    revision,
                    json,
                    now.get(),
                    event.id.get()
                ],
            )?;
            Ok(true)
        }

        NotificationTransition::Cancel => {
            set_job_state(tx, event.id, NotificationJobState::Cancelled, now)?;
            Ok(true)
        }

        NotificationTransition::Expire => {
            set_job_state(tx, event.id, NotificationJobState::Expired, now)?;
            Ok(true)
        }
    }
}

fn set_job_state(
    tx: &Transaction<'_>,
    event_id: ReleaseEventId,
    state: NotificationJobState,
    now: UnixTimestamp,
) -> Result<(), StoreError> {
    tx.execute(
        "UPDATE notification_jobs
         SET state = ?1, registered_revision = NULL, registered_at = NULL, updated_at = ?2
         WHERE release_event_id = ?3",
        rusqlite::params![state.as_str(), now.get(), event_id.get()],
    )?;
    Ok(())
}

/// Marks a job as present in the OS at a given revision.
pub fn mark_registered(
    tx: &Transaction<'_>,
    key: &NotificationKey,
    revision: i64,
    now: UnixTimestamp,
) -> Result<(), StoreError> {
    tx.execute(
        "UPDATE notification_jobs
         SET state = 'registered', registered_revision = ?1, registered_at = ?2,
             retry_after = NULL, last_error_code = NULL, updated_at = ?2
         WHERE notification_key = ?3 AND desired_revision = ?1",
        rusqlite::params![revision, now.get(), key.as_str()],
    )?;
    Ok(())
}

pub fn mark_delivered(
    tx: &Transaction<'_>,
    key: &NotificationKey,
    now: UnixTimestamp,
) -> Result<(), StoreError> {
    tx.execute(
        "UPDATE notification_jobs
         SET state = 'delivered', delivered_observed_at = ?1, updated_at = ?1
         WHERE notification_key = ?2",
        rusqlite::params![now.get(), key.as_str()],
    )?;
    Ok(())
}

pub fn mark_failed(
    tx: &Transaction<'_>,
    key: &NotificationKey,
    error_code: &str,
    retry_after: UnixTimestamp,
    now: UnixTimestamp,
) -> Result<(), StoreError> {
    tx.execute(
        "UPDATE notification_jobs
         SET state = 'failed', attempt_count = attempt_count + 1, last_attempt_at = ?1,
             retry_after = ?2, last_error_code = ?3, registered_revision = NULL,
             registered_at = NULL, updated_at = ?1
         WHERE notification_key = ?4",
        rusqlite::params![now.get(), retry_after.get(), error_code, key.as_str()],
    )?;
    Ok(())
}

/// One row of the desired registration plan.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PlanRow {
    pub key: NotificationKey,
    pub os_identifier: OsIdentifier,
    pub desired_revision: i64,
    pub desired_request_json: String,
    pub desired_at: UnixTimestamp,
    pub state: NotificationJobState,
    pub retry_after: Option<UnixTimestamp>,
}

impl PlanRow {
    pub fn request(&self) -> Result<NativeRequest, StoreError> {
        serde_json::from_str(&self.desired_request_json)
            .map_err(|e| StoreError::Integrity(format!("stored request: {e}")))
    }
}

/// The nearest jobs that still want to exist in the OS, soonest first.
///
/// `limit` is the proven native capacity; anything past it stays durable and is
/// reported as deferred rather than dropped.
pub fn plan(conn: &Connection, limit: u32) -> Result<Vec<PlanRow>, StoreError> {
    let mut stmt = conn.prepare(
        "SELECT notification_key, os_identifier, desired_revision, desired_request_json,
                desired_at, state, retry_after
         FROM notification_jobs
         WHERE state IN ('desired', 'registered', 'failed')
         ORDER BY desired_at ASC, notification_key ASC
         LIMIT ?1",
    )?;
    let rows = stmt
        .query_map([limit], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, i64>(2)?,
                row.get::<_, String>(3)?,
                row.get::<_, i64>(4)?,
                row.get::<_, String>(5)?,
                row.get::<_, Option<i64>>(6)?,
            ))
        })?
        .collect::<Result<Vec<_>, _>>()?;

    rows.into_iter()
        .map(|(key, os_id, revision, json, desired_at, state, retry)| {
            Ok(PlanRow {
                key: NotificationKey::from_stored(key),
                os_identifier: OsIdentifier::from_stored(os_id),
                desired_revision: revision,
                desired_request_json: json,
                desired_at: ts(desired_at)?,
                state: NotificationJobState::parse(&state)
                    .map_err(|e| StoreError::Integrity(e.to_string()))?,
                retry_after: retry.map(ts).transpose()?,
            })
        })
        .collect()
}

/// How many jobs want registration beyond `limit`.
pub fn deferred_count(conn: &Connection, limit: u32) -> Result<u32, StoreError> {
    let total: i64 = conn.query_row(
        "SELECT count(*) FROM notification_jobs
         WHERE state IN ('desired', 'registered', 'failed')",
        [],
        |row| row.get(0),
    )?;
    Ok(u32::try_from(total.saturating_sub(i64::from(limit))).unwrap_or(0))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::ids::AniListId;
    use crate::domain::ids::SourceKey;
    use crate::domain::media::{
        MediaObservation, MediaStatus, NextAiring, TitleSet, MAX_TITLE_LEN, PARSER_VERSION,
    };
    use crate::store::graph::{self, FetchRecord};
    use crate::store::{connection::configure, migrations};

    fn at(seconds: i64) -> UnixTimestamp {
        UnixTimestamp::new(seconds).expect("valid timestamp")
    }

    fn ep(number: i64) -> EpisodeNumber {
        EpisodeNumber::new(number).expect("valid episode")
    }

    struct Fixture {
        conn: Connection,
        source_media_id: SourceMediaId,
        media_id: MediaId,
        observation_id: ObservationId,
    }

    fn fixture() -> Fixture {
        let mut conn = Connection::open_in_memory().expect("open");
        configure(&conn, false).expect("configure");
        migrations::apply(&mut conn, 100).expect("migrate");

        let tx = conn.transaction().expect("begin");
        let title = BoundedText::new("t", MAX_TITLE_LEN, "One Piece").expect("title");
        let row = graph::create_media(&tx, AniListId::new(21).expect("id"), &title, at(100))
            .expect("create media");
        let fetch = graph::insert_fetch(
            &tx,
            &FetchRecord {
                attempt_uuid: "a",
                request_kind: "detail",
                request_fingerprint: "id=21",
                requested_at: at(100),
                completed_at: at(101),
                outcome: "success",
                http_status: Some(200),
                retry_after: None,
                rate_limit_remaining: None,
                rate_limit_reset_at: None,
                body_json: Some("{}"),
                error_code: None,
            },
        )
        .expect("fetch");
        let observation = MediaObservation {
            source_key: SourceKey::anilist(AniListId::new(21).expect("id")),
            display_title: title.clone(),
            titles: TitleSet::default(),
            status: MediaStatus::Releasing,
            status_raw: None,
            format_raw: None,
            episode_count: None,
            season_year: None,
            next_airing: Some(NextAiring {
                episode: ep(5),
                airing_at: at(5_000),
            }),
            parser_version: PARSER_VERSION,
        };
        let observation_id =
            graph::insert_observation(&tx, row.source_media_id, fetch, &observation, at(101))
                .expect("observation");
        graph::set_current_observation(&tx, row.source_media_id, observation_id, at(101))
            .expect("point");
        tx.commit().expect("commit");

        Fixture {
            conn,
            source_media_id: row.source_media_id,
            media_id: row.media_id,
            observation_id,
        }
    }

    fn apply(f: &mut Fixture, transition: ReleaseTransition, now: i64) -> Option<ReleaseEventId> {
        let tx = f.conn.transaction().expect("begin");
        let result = apply_transition(
            &tx,
            f.source_media_id,
            f.media_id,
            f.observation_id,
            transition,
            at(now),
        )
        .expect("apply");
        tx.commit().expect("commit");
        result
    }

    #[test]
    fn insert_creates_a_scheduled_event_at_revision_one() {
        let mut f = fixture();
        let id = apply(
            &mut f,
            ReleaseTransition::Insert {
                episode: ep(5),
                scheduled_at: at(5_000),
            },
            101,
        )
        .expect("event id");

        let event = event_by_id(&f.conn, id).expect("read").expect("present");
        assert_eq!(event.state, ReleaseEventState::Scheduled);
        assert_eq!(event.schedule_revision, 1);
        assert_eq!(event.sequence_number, Some(ep(5)));
        assert_eq!(event.source_event_key.as_str(), "ep:5");
    }

    #[test]
    fn touch_does_not_move_the_schedule_revision() {
        // Bumping it here would re-register the notification on every refresh.
        let mut f = fixture();
        let id = apply(
            &mut f,
            ReleaseTransition::Insert {
                episode: ep(5),
                scheduled_at: at(5_000),
            },
            101,
        )
        .expect("event id");

        apply(&mut f, ReleaseTransition::Touch { event_id: id }, 200);

        let event = event_by_id(&f.conn, id).expect("read").expect("present");
        assert_eq!(event.schedule_revision, 1);
        assert_eq!(event.last_observed_at, at(200));
    }

    #[test]
    fn reschedule_moves_the_time_and_the_revision() {
        let mut f = fixture();
        let id = apply(
            &mut f,
            ReleaseTransition::Insert {
                episode: ep(5),
                scheduled_at: at(5_000),
            },
            101,
        )
        .expect("event id");

        apply(
            &mut f,
            ReleaseTransition::Reschedule {
                event_id: id,
                scheduled_at: at(6_000),
            },
            200,
        );

        let event = event_by_id(&f.conn, id).expect("read").expect("present");
        assert_eq!(event.scheduled_at, at(6_000));
        assert_eq!(event.schedule_revision, 2);
    }

    #[test]
    fn advance_retires_before_inserting_so_the_index_holds() {
        // Only one scheduled event per source media is permitted; inserting
        // before retiring would violate the partial unique index.
        let mut f = fixture();
        let first = apply(
            &mut f,
            ReleaseTransition::Insert {
                episode: ep(5),
                scheduled_at: at(5_000),
            },
            101,
        )
        .expect("first");

        let second = apply(
            &mut f,
            ReleaseTransition::Advance {
                retire: first,
                retire_state: ReleaseEventState::Elapsed,
                episode: ep(6),
                scheduled_at: at(12_000),
            },
            6_000,
        )
        .expect("second");

        assert_eq!(
            event_by_id(&f.conn, first)
                .expect("read")
                .expect("present")
                .state,
            ReleaseEventState::Elapsed
        );
        let current = scheduled_event(&f.conn, f.source_media_id)
            .expect("read")
            .expect("present");
        assert_eq!(current.id, second);
        assert_eq!(current.sequence_number, Some(ep(6)));
    }

    #[test]
    fn withdraw_bumps_the_revision_but_elapse_does_not() {
        // A withdrawal changes what should be registered, so it must move the
        // revision. Elapsing is just the passage of time.
        let mut f = fixture();
        let id = apply(
            &mut f,
            ReleaseTransition::Insert {
                episode: ep(5),
                scheduled_at: at(5_000),
            },
            101,
        )
        .expect("event id");

        apply(&mut f, ReleaseTransition::Withdraw { event_id: id }, 200);
        let event = event_by_id(&f.conn, id).expect("read").expect("present");
        assert_eq!(event.state, ReleaseEventState::Withdrawn);
        assert_eq!(event.schedule_revision, 2);

        apply(&mut f, ReleaseTransition::MarkElapsed { event_id: id }, 300);
        let event = event_by_id(&f.conn, id).expect("read").expect("present");
        assert_eq!(event.state, ReleaseEventState::Elapsed);
        assert_eq!(event.schedule_revision, 2);
    }

    #[test]
    fn reinstate_returns_a_withdrawn_event_to_scheduled() {
        let mut f = fixture();
        let id = apply(
            &mut f,
            ReleaseTransition::Insert {
                episode: ep(5),
                scheduled_at: at(5_000),
            },
            101,
        )
        .expect("event id");
        apply(&mut f, ReleaseTransition::Withdraw { event_id: id }, 200);
        apply(
            &mut f,
            ReleaseTransition::Reinstate {
                event_id: id,
                scheduled_at: at(7_000),
            },
            300,
        );

        let event = event_by_id(&f.conn, id).expect("read").expect("present");
        assert_eq!(event.state, ReleaseEventState::Scheduled);
        assert_eq!(event.scheduled_at, at(7_000));
        assert_eq!(event.schedule_revision, 3);
    }

    #[test]
    fn an_integrity_failure_writes_nothing() {
        let mut f = fixture();
        let result = apply(
            &mut f,
            ReleaseTransition::IntegrityFailure {
                latest: ep(5),
                observed: ep(4),
            },
            200,
        );
        assert_eq!(result, None);
        let count: i64 = f
            .conn
            .query_row("SELECT count(*) FROM release_events", [], |r| r.get(0))
            .expect("count");
        assert_eq!(count, 0);
    }

    #[test]
    fn latest_sequence_spans_every_state() {
        let mut f = fixture();
        let first = apply(
            &mut f,
            ReleaseTransition::Insert {
                episode: ep(5),
                scheduled_at: at(5_000),
            },
            101,
        )
        .expect("first");
        apply(
            &mut f,
            ReleaseTransition::Advance {
                retire: first,
                retire_state: ReleaseEventState::Elapsed,
                episode: ep(9),
                scheduled_at: at(12_000),
            },
            6_000,
        );

        assert_eq!(
            latest_sequence(&f.conn, f.source_media_id).expect("read"),
            Some(ep(9))
        );
    }

    #[test]
    fn lookup_by_key_finds_retired_events() {
        let mut f = fixture();
        let id = apply(
            &mut f,
            ReleaseTransition::Insert {
                episode: ep(5),
                scheduled_at: at(5_000),
            },
            101,
        )
        .expect("event id");
        apply(&mut f, ReleaseTransition::Withdraw { event_id: id }, 200);

        let found = event_by_key(&f.conn, f.source_media_id, "ep:5")
            .expect("read")
            .expect("present");
        assert_eq!(found.id, id);
        assert_eq!(found.state, ReleaseEventState::Withdrawn);
    }
}
