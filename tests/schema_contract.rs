//! C1B — proves `V0001` is real SQL and that its constraints actually bite.
//!
//! Section 3 requires the database to enforce ownership itself rather than
//! trusting Rust callers. A `CHECK` nobody has ever seen reject anything is a
//! comment, so each invariant here is exercised against a live SQLite.

#![allow(clippy::expect_used)]

use rusqlite::{Connection, ErrorCode};

const V0001: &str = include_str!("../migrations/V0001__daily_driver.sql");

fn db() -> Connection {
    let conn = Connection::open_in_memory().expect("open in-memory database");
    conn.pragma_update(None, "foreign_keys", "ON")
        .expect("enable foreign keys");
    conn.execute_batch(V0001).expect("apply V0001");
    conn
}

/// Inserts the minimum graph needed to hang events off: media, source media,
/// a fetch, and one observation, with the current pointer wired up.
fn seed(conn: &Connection) {
    conn.execute_batch(
        "
        INSERT INTO media (media_id, kind, display_title, created_at, updated_at)
            VALUES (1, 'anime', 'One Piece', 100, 100);
        INSERT INTO source_media
            (source_media_id, source, source_id, media_id, current_observation_id, created_at, updated_at)
            VALUES (1, 'anilist', 21, 1, NULL, 100, 100);
        INSERT INTO source_fetches
            (fetch_id, attempt_uuid, source, request_kind, request_fingerprint,
             requested_at, completed_at, outcome, body_json, byte_length)
            VALUES (1, 'attempt-1', 'anilist', 'detail', 'id=21', 100, 101, 'success', '{}', 2);
        INSERT INTO source_observations
            (observation_id, source_media_id, fetch_id, parser_version, observed_at,
             display_title, status, next_episode, next_airing_at)
            VALUES (1, 1, 1, 1, 101, 'One Piece', 'releasing', 5, 5000);
        UPDATE source_media SET current_observation_id = 1 WHERE source_media_id = 1;
        INSERT INTO follows (media_id, state, followed_at, updated_at)
            VALUES (1, 'active', 100, 100);
        ",
    )
    .expect("seed graph");
}

fn scheduled_event(conn: &Connection, id: i64, key: &str, state: &str) -> rusqlite::Result<usize> {
    conn.execute(
        "INSERT INTO release_events
            (release_event_id, event_uuid, media_id, source_media_id, source_event_key,
             sequence_number, scheduled_at, state, schedule_revision,
             first_observed_at, last_observed_at, last_observation_id)
         VALUES (?1, ?2, 1, 1, ?3, 5, 5000, ?4, 1, 101, 101, 1)",
        rusqlite::params![id, format!("uuid-{id}"), key, state],
    )
}

fn is_constraint_violation(error: &rusqlite::Error) -> bool {
    matches!(
        error.sqlite_error().map(|e| e.code),
        Some(ErrorCode::ConstraintViolation)
    )
}

#[test]
fn migration_applies_cleanly() {
    let _ = db();
}

#[test]
fn every_planned_table_exists() {
    let conn = db();
    for table in [
        "engine_state",
        "media",
        "source_fetches",
        "source_media",
        "source_observations",
        "source_refresh_state",
        "source_runtime_state",
        "follows",
        "release_events",
        "notification_jobs",
        "notification_surface_state",
    ] {
        let count: i64 = conn
            .query_row(
                "SELECT count(*) FROM sqlite_master WHERE type='table' AND name=?1",
                [table],
                |row| row.get(0),
            )
            .expect("query sqlite_master");
        assert_eq!(count, 1, "table {table} is missing");
    }
}

#[test]
fn no_blob_table_survives_from_the_earlier_design() {
    let conn = db();
    let count: i64 = conn
        .query_row(
            "SELECT count(*) FROM sqlite_master WHERE type='table' AND name='raw_payload_blobs'",
            [],
            |row| row.get(0),
        )
        .expect("query sqlite_master");
    assert_eq!(count, 0);
}

#[test]
fn every_table_is_strict() {
    // STRICT is what makes the integer columns actually integers. Without it
    // SQLite would happily store the string 'tomorrow' in scheduled_at.
    let conn = db();
    let mut stmt = conn
        .prepare(
            "SELECT name, sql FROM sqlite_master WHERE type='table' AND name NOT LIKE 'sqlite_%'",
        )
        .expect("prepare");
    let rows: Vec<(String, String)> = stmt
        .query_map([], |row| Ok((row.get(0)?, row.get(1)?)))
        .expect("query")
        .collect::<Result<_, _>>()
        .expect("collect");

    assert!(!rows.is_empty());
    for (name, sql) in rows {
        assert!(
            sql.to_uppercase().contains("STRICT"),
            "table {name} is not STRICT"
        );
    }
}

#[test]
fn strict_rejects_a_string_in_an_integer_column() {
    let conn = db();
    let error = conn
        .execute(
            "INSERT INTO media (media_id, kind, display_title, created_at, updated_at)
             VALUES ('one', 'anime', 'x', 1, 1)",
            [],
        )
        .expect_err("STRICT should reject a text media_id");
    assert!(error.to_string().contains("datatype mismatch"), "{error}");
}

#[test]
fn engine_state_admits_only_one_row() {
    let conn = db();
    conn.execute(
        "INSERT INTO engine_state (singleton_id, installation_uuid, created_at, updated_at)
         VALUES (1, 'install-a', 100, 100)",
        [],
    )
    .expect("first row");

    let error = conn
        .execute(
            "INSERT INTO engine_state (singleton_id, installation_uuid, created_at, updated_at)
             VALUES (2, 'install-b', 100, 100)",
            [],
        )
        .expect_err("a second identity must be impossible");
    assert!(is_constraint_violation(&error), "{error}");
}

#[test]
fn updated_at_may_not_precede_created_at() {
    let conn = db();
    let error = conn
        .execute(
            "INSERT INTO media (media_id, kind, display_title, created_at, updated_at)
             VALUES (1, 'anime', 'x', 100, 99)",
            [],
        )
        .expect_err("time must not run backwards");
    assert!(is_constraint_violation(&error), "{error}");
}

#[test]
fn a_successful_fetch_must_carry_its_body() {
    // Evidence with no evidence in it is not evidence.
    let conn = db();
    let error = conn
        .execute(
            "INSERT INTO source_fetches
                (fetch_id, attempt_uuid, source, request_kind, request_fingerprint,
                 requested_at, completed_at, outcome)
             VALUES (1, 'a', 'anilist', 'detail', 'id=21', 100, 101, 'success')",
            [],
        )
        .expect_err("success without a body must be rejected");
    assert!(is_constraint_violation(&error), "{error}");
}

#[test]
fn a_transport_failure_may_have_no_body() {
    let conn = db();
    conn.execute(
        "INSERT INTO source_fetches
            (fetch_id, attempt_uuid, source, request_kind, request_fingerprint,
             requested_at, completed_at, outcome)
         VALUES (1, 'a', 'anilist', 'detail', 'id=21', 100, 101, 'transport_error')",
        [],
    )
    .expect("a failed connection produces no bytes");
}

#[test]
fn body_and_byte_length_travel_together() {
    let conn = db();
    let error = conn
        .execute(
            "INSERT INTO source_fetches
                (fetch_id, attempt_uuid, source, request_kind, request_fingerprint,
                 requested_at, completed_at, outcome, body_json)
             VALUES (1, 'a', 'anilist', 'detail', 'id=21', 100, 101, 'success', '{}')",
            [],
        )
        .expect_err("a body without its length must be rejected");
    assert!(is_constraint_violation(&error), "{error}");
}

#[test]
fn an_unknown_fetch_outcome_is_rejected() {
    let conn = db();
    let error = conn
        .execute(
            "INSERT INTO source_fetches
                (fetch_id, attempt_uuid, source, request_kind, request_fingerprint,
                 requested_at, completed_at, outcome, body_json, byte_length)
             VALUES (1, 'a', 'anilist', 'detail', 'id=21', 100, 101, 'weird', '{}', 2)",
            [],
        )
        .expect_err("unknown outcome");
    assert!(is_constraint_violation(&error), "{error}");
}

#[test]
fn source_id_is_bounded_to_the_range_anilist_uses() {
    let conn = db();
    conn.execute(
        "INSERT INTO media (media_id, kind, display_title, created_at, updated_at)
         VALUES (1, 'anime', 'x', 100, 100)",
        [],
    )
    .expect("media");

    for bad in [0_i64, -1, 2_147_483_648] {
        let error = conn
            .execute(
                "INSERT INTO source_media
                    (source_media_id, source, source_id, media_id, created_at, updated_at)
                 VALUES (99, 'anilist', ?1, 1, 100, 100)",
                [bad],
            )
            .expect_err("out-of-range source id");
        assert!(is_constraint_violation(&error), "{bad}: {error}");
    }
}

#[test]
fn a_half_populated_schedule_cannot_be_stored() {
    // The database counterpart of the NextAiring struct: an episode without an
    // airtime would be indistinguishable from an explicit null.
    let conn = db();
    seed(&conn);

    for (episode, airing) in [("5", "NULL"), ("NULL", "5000")] {
        let sql = format!(
            "INSERT INTO source_observations
                (observation_id, source_media_id, fetch_id, parser_version, observed_at,
                 display_title, status, next_episode, next_airing_at)
             VALUES (99, 1, 1, 1, 102, 'x', 'releasing', {episode}, {airing})"
        );
        let error = conn.execute(&sql, []).expect_err("half-populated schedule");
        assert!(is_constraint_violation(&error), "{error}");
    }
}

#[test]
fn the_current_observation_pointer_must_belong_to_the_same_source_media() {
    // Without the composite foreign key this would silently point one show's
    // current facts at another show's observation.
    let conn = db();
    seed(&conn);
    conn.execute(
        "INSERT INTO media (media_id, kind, display_title, created_at, updated_at)
         VALUES (2, 'anime', 'Other', 100, 100)",
        [],
    )
    .expect("second media");
    conn.execute(
        "INSERT INTO source_media
            (source_media_id, source, source_id, media_id, created_at, updated_at)
         VALUES (2, 'anilist', 11061, 2, 100, 100)",
        [],
    )
    .expect("second source media");

    let error = conn
        .execute(
            "UPDATE source_media SET current_observation_id = 1 WHERE source_media_id = 2",
            [],
        )
        .expect_err("observation 1 belongs to source_media 1");
    assert!(is_constraint_violation(&error), "{error}");
}

#[test]
fn only_one_scheduled_event_per_source_media() {
    // Two would mean two competing notifications for the same show.
    let conn = db();
    seed(&conn);
    scheduled_event(&conn, 1, "ep:5", "scheduled").expect("first scheduled event");

    let error = scheduled_event(&conn, 2, "ep:6", "scheduled").expect_err("second scheduled event");
    assert!(is_constraint_violation(&error), "{error}");
}

#[test]
fn retired_events_may_coexist_with_a_scheduled_one() {
    let conn = db();
    seed(&conn);
    scheduled_event(&conn, 1, "ep:5", "elapsed").expect("elapsed");
    scheduled_event(&conn, 2, "ep:6", "superseded").expect("superseded");
    scheduled_event(&conn, 3, "ep:7", "withdrawn").expect("withdrawn");
    scheduled_event(&conn, 4, "ep:8", "scheduled").expect("scheduled");
}

#[test]
fn an_event_key_is_unique_within_its_source_media() {
    let conn = db();
    seed(&conn);
    scheduled_event(&conn, 1, "ep:5", "elapsed").expect("first");
    let error = scheduled_event(&conn, 2, "ep:5", "withdrawn").expect_err("duplicate key");
    assert!(is_constraint_violation(&error), "{error}");
}

#[test]
fn a_registered_job_must_record_the_revision_the_os_holds() {
    let conn = db();
    seed(&conn);
    scheduled_event(&conn, 1, "ep:5", "scheduled").expect("event");

    let error = conn
        .execute(
            "INSERT INTO notification_jobs
                (notification_key, release_event_id, os_identifier, desired_at,
                 desired_revision, desired_request_json, state, updated_at)
             VALUES ('k', 1, 'dev.animesh.release.a.b', 5000, 1, '{}', 'registered', 100)",
            [],
        )
        .expect_err("registered without registered_revision");
    assert!(is_constraint_violation(&error), "{error}");
}

#[test]
fn a_delivered_job_must_record_when_it_was_observed() {
    let conn = db();
    seed(&conn);
    scheduled_event(&conn, 1, "ep:5", "scheduled").expect("event");

    let error = conn
        .execute(
            "INSERT INTO notification_jobs
                (notification_key, release_event_id, os_identifier, desired_at,
                 desired_revision, desired_request_json, state, updated_at)
             VALUES ('k', 1, 'dev.animesh.release.a.b', 5000, 1, '{}', 'delivered', 100)",
            [],
        )
        .expect_err("delivered without delivered_observed_at");
    assert!(is_constraint_violation(&error), "{error}");
}

#[test]
fn the_removed_ambiguous_state_is_rejected() {
    let conn = db();
    seed(&conn);
    scheduled_event(&conn, 1, "ep:5", "scheduled").expect("event");

    let error = conn
        .execute(
            "INSERT INTO notification_jobs
                (notification_key, release_event_id, os_identifier, desired_at,
                 desired_revision, desired_request_json, state, updated_at)
             VALUES ('k', 1, 'dev.animesh.release.a.b', 5000, 1, '{}', 'ambiguous', 100)",
            [],
        )
        .expect_err("ambiguous is no longer a state");
    assert!(is_constraint_violation(&error), "{error}");
}

#[test]
fn one_job_per_release_event() {
    let conn = db();
    seed(&conn);
    scheduled_event(&conn, 1, "ep:5", "scheduled").expect("event");
    conn.execute(
        "INSERT INTO notification_jobs
            (notification_key, release_event_id, os_identifier, desired_at,
             desired_revision, desired_request_json, state, updated_at)
         VALUES ('k1', 1, 'dev.animesh.release.a.b', 5000, 1, '{}', 'desired', 100)",
        [],
    )
    .expect("first job");

    let error = conn
        .execute(
            "INSERT INTO notification_jobs
                (notification_key, release_event_id, os_identifier, desired_at,
                 desired_revision, desired_request_json, state, updated_at)
             VALUES ('k2', 1, 'dev.animesh.release.c.d', 5000, 1, '{}', 'desired', 100)",
            [],
        )
        .expect_err("two jobs for one event");
    assert!(is_constraint_violation(&error), "{error}");
}

#[test]
fn foreign_keys_are_enforced_when_the_pragma_is_on() {
    let conn = db();
    let error = conn
        .execute(
            "INSERT INTO follows (media_id, state, followed_at, updated_at)
             VALUES (999, 'active', 100, 100)",
            [],
        )
        .expect_err("follow of a nonexistent media");
    assert!(is_constraint_violation(&error), "{error}");
}

#[test]
fn the_due_and_plan_indexes_exist() {
    // The scheduler and the reconciler both scan these; without the indexes
    // they degrade to full table scans as the library grows.
    let conn = db();
    for index in [
        "idx_refresh_due",
        "idx_release_events_due",
        "idx_notification_jobs_plan",
        "idx_one_scheduled_event_per_source_media",
    ] {
        let count: i64 = conn
            .query_row(
                "SELECT count(*) FROM sqlite_master WHERE type='index' AND name=?1",
                [index],
                |row| row.get(0),
            )
            .expect("query sqlite_master");
        assert_eq!(count, 1, "index {index} is missing");
    }
}

#[test]
fn the_upcoming_query_uses_an_index_rather_than_scanning() {
    let conn = db();
    let plan: String = conn
        .query_row(
            "EXPLAIN QUERY PLAN
             SELECT release_event_id FROM release_events
             WHERE state = 'scheduled' AND scheduled_at >= 0
             ORDER BY scheduled_at",
            [],
            |row| row.get(3),
        )
        .expect("explain");
    assert!(
        plan.contains("USING INDEX") || plan.contains("USING COVERING INDEX"),
        "upcoming query falls back to a scan: {plan}"
    );
}
