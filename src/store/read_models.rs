//! Serving queries. No writes, no network.

use rusqlite::Connection;

use super::connection::StoreError;
use crate::domain::ids::{
    AniListId, BoundedText, EpisodeNumber, EventUuid, MediaId, ReleaseEventId, UnixTimestamp,
};
use crate::domain::media::MAX_TITLE_LEN;
use crate::domain::read_models::{Freshness, RefreshCounts, UpcomingRelease};

/// The one `SELECT` behind both `animesh next` and the menu summary.
///
/// The `ORDER BY` reproduces the frozen total order. A test
/// compares it against the Rust implementation, because a read model whose
/// order depends on which layer produced it is not a read model.
const UPCOMING_SQL: &str = "
    SELECT re.release_event_id, re.event_uuid, re.media_id, sm.source_id,
           m.display_title, re.sequence_number, re.scheduled_at, re.schedule_revision,
           rs.last_success_at, rs.refresh_after, rs.retry_after
    FROM release_events re
    JOIN follows f       ON f.media_id = re.media_id AND f.state = 'active'
    JOIN media m         ON m.media_id = re.media_id
    JOIN source_media sm ON sm.source_media_id = re.source_media_id
    LEFT JOIN source_refresh_state rs ON rs.source_media_id = re.source_media_id
    WHERE re.state IN ('scheduled', 'elapsed')
      AND re.scheduled_at >= ?1 - ?3
      AND (re.state = 'scheduled' OR re.scheduled_at < ?1)
    ORDER BY re.scheduled_at ASC,
             re.media_id ASC,
             (re.sequence_number IS NULL) ASC,
             re.sequence_number ASC,
             re.release_event_id ASC
    LIMIT ?2
";

fn ts(value: i64) -> Result<UnixTimestamp, StoreError> {
    UnixTimestamp::new(value).map_err(|e| StoreError::Integrity(e.to_string()))
}

/// Classifies how current the source data behind a row is.
fn freshness(
    now: UnixTimestamp,
    refresh_after: Option<i64>,
    retry_after: Option<i64>,
) -> Freshness {
    // Backing off wins: a row in failure backoff is stale *and* explains why,
    // and the explanation is what the owner needs.
    if retry_after.is_some_and(|deadline| deadline > now.get()) {
        return Freshness::BackingOff;
    }
    match refresh_after {
        Some(after) if after <= now.get() => Freshness::Stale,
        Some(_) => Freshness::Fresh,
        // Never refreshed at all.
        None => Freshness::Stale,
    }
}

/// Serves upcoming releases plus anything that aired within `lookback_secs`.
///
/// Pass `0` for a strictly-future view. The CLI passes
/// [`AIRED_VISIBILITY_SECS`] so an episode stays visible through the window
/// where the owner is asking whether it dropped.
///
/// `elapsed` events are included only when genuinely in the past — an elapsed
/// row with a future airtime would mean corrupt state, not a recent airing.
pub fn upcoming(
    conn: &Connection,
    now: UnixTimestamp,
    limit: u32,
    lookback_secs: i64,
) -> Result<Vec<UpcomingRelease>, StoreError> {
    let mut stmt = conn.prepare(UPCOMING_SQL)?;
    let rows = stmt
        .query_map(rusqlite::params![now.get(), limit, lookback_secs], |row| {
            Ok((
                row.get::<_, i64>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, i64>(2)?,
                row.get::<_, i64>(3)?,
                row.get::<_, String>(4)?,
                row.get::<_, Option<i64>>(5)?,
                row.get::<_, i64>(6)?,
                row.get::<_, i64>(7)?,
                row.get::<_, Option<i64>>(8)?,
                row.get::<_, Option<i64>>(9)?,
                row.get::<_, Option<i64>>(10)?,
            ))
        })?
        .collect::<Result<Vec<_>, _>>()?;

    rows.into_iter()
        .map(|row| {
            let (
                event_id,
                uuid,
                media_id,
                anilist_id,
                title,
                sequence,
                scheduled_at,
                revision,
                last_success,
                refresh_after,
                retry_after,
            ) = row;
            let bad = |e: String| StoreError::Integrity(e);

            Ok(UpcomingRelease {
                release_event_id: ReleaseEventId::new(event_id).map_err(|e| bad(e.to_string()))?,
                event_uuid: EventUuid::from_uuid(
                    uuid.parse().map_err(|e| bad(format!("event uuid: {e}")))?,
                ),
                media_id: MediaId::new(media_id).map_err(|e| bad(e.to_string()))?,
                anilist_id: AniListId::new(anilist_id).map_err(|e| bad(e.to_string()))?,
                display_title: BoundedText::truncating(MAX_TITLE_LEN, &title)
                    .ok_or_else(|| bad("empty title".into()))?,
                episode: sequence
                    .map(EpisodeNumber::new)
                    .transpose()
                    .map_err(|e| bad(e.to_string()))?,
                scheduled_at: ts(scheduled_at)?,
                schedule_revision: revision,
                last_success_at: last_success.map(ts).transpose()?,
                freshness: freshness(now, refresh_after, retry_after),
                aired: scheduled_at <= now.get(),
            })
        })
        .collect()
}

/// Counts for the health snapshot.
pub fn refresh_counts(conn: &Connection, now: UnixTimestamp) -> Result<RefreshCounts, StoreError> {
    let row = conn.query_row(
        "SELECT
            sum(CASE WHEN rs.refresh_after <= ?1 AND (rs.retry_after IS NULL OR rs.retry_after <= ?1)
                     THEN 1 ELSE 0 END),
            sum(CASE WHEN rs.refresh_after <= ?1 THEN 1 ELSE 0 END),
            sum(CASE WHEN rs.retry_after > ?1 THEN 1 ELSE 0 END),
            sum(CASE WHEN rs.consecutive_failures > 0 THEN 1 ELSE 0 END)
         FROM source_refresh_state rs
         JOIN source_media sm ON sm.source_media_id = rs.source_media_id
         JOIN follows f ON f.media_id = sm.media_id AND f.state = 'active'",
        [now.get()],
        |row| {
            Ok((
                row.get::<_, Option<i64>>(0)?,
                row.get::<_, Option<i64>>(1)?,
                row.get::<_, Option<i64>>(2)?,
                row.get::<_, Option<i64>>(3)?,
            ))
        },
    )?;

    let count = |value: Option<i64>| u32::try_from(value.unwrap_or(0)).unwrap_or(u32::MAX);
    Ok(RefreshCounts {
        due: count(row.0),
        stale: count(row.1),
        backing_off: count(row.2),
        failed: count(row.3),
    })
}

/// The most recent successful refresh across active follows.
pub fn last_success(conn: &Connection) -> Result<Option<UnixTimestamp>, StoreError> {
    conn.query_row(
        "SELECT max(rs.last_success_at)
         FROM source_refresh_state rs
         JOIN source_media sm ON sm.source_media_id = rs.source_media_id
         JOIN follows f ON f.media_id = sm.media_id AND f.state = 'active'",
        [],
        |row| row.get::<_, Option<i64>>(0),
    )?
    .map(ts)
    .transpose()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::ids::{ObservationId, SourceKey, SourceMediaId};
    use crate::domain::media::{
        MediaObservation, MediaStatus, NextAiring, TitleSet, PARSER_VERSION,
    };
    use crate::domain::release::FollowState;
    use crate::library::reducers::ReleaseTransition;
    use crate::store::graph::{self, FetchRecord};
    use crate::store::{connection::configure, migrations, releases};

    fn at(seconds: i64) -> UnixTimestamp {
        UnixTimestamp::new(seconds).expect("valid timestamp")
    }

    fn ep(number: i64) -> EpisodeNumber {
        EpisodeNumber::new(number).expect("valid episode")
    }

    fn title(value: &str) -> BoundedText {
        BoundedText::new("t", MAX_TITLE_LEN, value).expect("valid title")
    }

    struct World {
        conn: Connection,
    }

    impl World {
        fn new() -> Self {
            let mut conn = Connection::open_in_memory().expect("open");
            configure(&conn, false).expect("configure");
            migrations::apply(&mut conn, 100).expect("migrate");
            Self { conn }
        }

        /// Adds a followed show with one scheduled episode.
        fn add(
            &mut self,
            anilist_id: i64,
            name: &str,
            episode: i64,
            scheduled_at: i64,
            follow: FollowState,
        ) -> (MediaId, SourceMediaId, ObservationId) {
            let tx = self.conn.transaction().expect("begin");
            let row = graph::create_media(
                &tx,
                AniListId::new(anilist_id).expect("id"),
                &title(name),
                at(100),
            )
            .expect("media");
            let fetch = graph::insert_fetch(
                &tx,
                &FetchRecord {
                    attempt_uuid: &format!("attempt-{anilist_id}"),
                    request_kind: "detail",
                    request_fingerprint: "x",
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
                source_key: SourceKey::anilist(AniListId::new(anilist_id).expect("id")),
                display_title: title(name),
                titles: TitleSet::default(),
                status: MediaStatus::Releasing,
                status_raw: None,
                format_raw: None,
                episode_count: None,
                season_year: None,
                next_airing: Some(NextAiring {
                    episode: ep(episode),
                    airing_at: at(scheduled_at),
                }),
                parser_version: PARSER_VERSION,
            };
            let obs =
                graph::insert_observation(&tx, row.source_media_id, fetch, &observation, at(101))
                    .expect("observation");
            graph::set_current_observation(&tx, row.source_media_id, obs, at(101)).expect("point");
            graph::set_follow(&tx, row.media_id, follow, at(100)).expect("follow");
            releases::apply_transition(
                &tx,
                row.source_media_id,
                row.media_id,
                obs,
                ReleaseTransition::Insert {
                    episode: ep(episode),
                    scheduled_at: at(scheduled_at),
                },
                at(101),
            )
            .expect("event");
            tx.commit().expect("commit");
            (row.media_id, row.source_media_id, obs)
        }
    }

    #[test]
    fn upcoming_returns_followed_scheduled_events() {
        let mut world = World::new();
        world.add(21, "One Piece", 5, 5_000, FollowState::Active);

        let rows = upcoming(&world.conn, at(1_000), 50, 0).expect("query");
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].display_title.as_str(), "One Piece");
        assert_eq!(rows[0].episode, Some(ep(5)));
        assert_eq!(rows[0].anilist_id.get(), 21);
    }

    #[test]
    fn dropped_follows_are_excluded() {
        let mut world = World::new();
        world.add(21, "One Piece", 5, 5_000, FollowState::Dropped);
        assert!(upcoming(&world.conn, at(1_000), 50, 0)
            .expect("query")
            .is_empty());
    }

    #[test]
    fn events_in_the_past_are_excluded() {
        let mut world = World::new();
        world.add(21, "One Piece", 5, 5_000, FollowState::Active);
        assert!(upcoming(&world.conn, at(5_001), 50, 0)
            .expect("query")
            .is_empty());
        // Exactly at airtime the episode is still returned: it is airing now.
        assert_eq!(
            upcoming(&world.conn, at(5_000), 50, 0)
                .expect("query")
                .len(),
            1
        );
    }

    #[test]
    fn the_limit_is_applied() {
        let mut world = World::new();
        for i in 1..=5 {
            world.add(
                20 + i,
                &format!("Show {i}"),
                1,
                5_000 + i,
                FollowState::Active,
            );
        }
        assert_eq!(
            upcoming(&world.conn, at(1_000), 3, 0).expect("query").len(),
            3
        );
    }

    #[test]
    fn sql_ordering_matches_the_rust_total_order() {
        // The read model must not depend on which layer sorted it.
        let mut world = World::new();
        world.add(30, "C", 1, 9_000, FollowState::Active);
        world.add(21, "A", 2, 5_000, FollowState::Active);
        world.add(25, "B", 1, 5_000, FollowState::Active);

        let from_sql = upcoming(&world.conn, at(0), 50, 0).expect("query");
        let mut sorted = from_sql.clone();
        sorted.sort();

        assert_eq!(
            from_sql.iter().map(|r| r.sort_key()).collect::<Vec<_>>(),
            sorted.iter().map(|r| r.sort_key()).collect::<Vec<_>>(),
            "SQL order and Rust order disagree"
        );
    }

    #[test]
    fn a_never_refreshed_row_reads_as_stale() {
        let mut world = World::new();
        world.add(21, "One Piece", 5, 5_000, FollowState::Active);
        let rows = upcoming(&world.conn, at(1_000), 50, 0).expect("query");
        assert_eq!(rows[0].freshness, Freshness::Stale);
    }

    #[test]
    fn a_recently_refreshed_row_reads_as_fresh() {
        let mut world = World::new();
        let (_, source_media_id, _) = world.add(21, "One Piece", 5, 5_000, FollowState::Active);
        let tx = world.conn.transaction().expect("begin");
        graph::record_refresh_success(&tx, source_media_id, at(500), at(4_000)).expect("success");
        tx.commit().expect("commit");

        let rows = upcoming(&world.conn, at(1_000), 50, 0).expect("query");
        assert_eq!(rows[0].freshness, Freshness::Fresh);
        assert_eq!(rows[0].last_success_at, Some(at(500)));
    }

    #[test]
    fn a_backing_off_row_says_so_rather_than_merely_stale() {
        // Stale and backing-off are both out of date, but only one of them
        // explains why, and that is what the owner needs to see.
        let mut world = World::new();
        let (_, source_media_id, _) = world.add(21, "One Piece", 5, 5_000, FollowState::Active);
        let tx = world.conn.transaction().expect("begin");
        graph::record_refresh_failure(&tx, source_media_id, at(500), "http_503", at(2_000))
            .expect("failure");
        tx.commit().expect("commit");

        let rows = upcoming(&world.conn, at(1_000), 50, 0).expect("query");
        assert_eq!(rows[0].freshness, Freshness::BackingOff);
    }

    #[test]
    fn counts_only_cover_active_follows() {
        let mut world = World::new();
        let (_, active, _) = world.add(21, "A", 5, 5_000, FollowState::Active);
        let (_, dropped, _) = world.add(25, "B", 5, 5_000, FollowState::Dropped);

        let tx = world.conn.transaction().expect("begin");
        graph::record_refresh_failure(&tx, active, at(500), "timeout", at(2_000)).expect("fail");
        graph::record_refresh_failure(&tx, dropped, at(500), "timeout", at(2_000)).expect("fail");
        tx.commit().expect("commit");

        let counts = refresh_counts(&world.conn, at(1_000)).expect("counts");
        assert_eq!(counts.backing_off, 1);
        assert_eq!(counts.failed, 1);
    }

    #[test]
    fn last_success_reports_the_most_recent_across_follows() {
        let mut world = World::new();
        let (_, a, _) = world.add(21, "A", 5, 5_000, FollowState::Active);
        let (_, b, _) = world.add(25, "B", 5, 5_000, FollowState::Active);

        let tx = world.conn.transaction().expect("begin");
        graph::record_refresh_success(&tx, a, at(500), at(4_000)).expect("a");
        graph::record_refresh_success(&tx, b, at(900), at(4_000)).expect("b");
        tx.commit().expect("commit");

        assert_eq!(last_success(&world.conn).expect("read"), Some(at(900)));
    }

    #[test]
    fn an_empty_library_reports_zeroes_rather_than_failing() {
        let world = World::new();
        assert_eq!(
            refresh_counts(&world.conn, at(1_000)).expect("counts"),
            RefreshCounts::default()
        );
        assert_eq!(last_success(&world.conn).expect("read"), None);
        assert!(upcoming(&world.conn, at(1_000), 50, 0)
            .expect("query")
            .is_empty());
    }
}
