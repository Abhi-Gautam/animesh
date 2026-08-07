//! Canonical graph and evidence repositories.
//!
//! Every function takes a transaction or connection handle and never opens its
//! own, so the Library decides what is atomic with what.

use rusqlite::{Connection, OptionalExtension, Transaction};

use super::connection::StoreError;
use crate::domain::ids::{
    AniListId, BoundedText, EpisodeNumber, FetchId, InstallationUuid, MediaId, ObservationId,
    SourceKey, SourceMediaId, UnixTimestamp,
};
use crate::domain::media::{
    MediaObservation, MediaStatus, NextAiring, TitleSet, MAX_RAW_LEN, MAX_TITLE_LEN,
};
use crate::domain::release::FollowState;

fn ts(value: i64) -> Result<UnixTimestamp, StoreError> {
    UnixTimestamp::new(value).map_err(|e| StoreError::Integrity(e.to_string()))
}

fn text(value: String, max: usize) -> Result<BoundedText, StoreError> {
    BoundedText::truncating(max, &value)
        .ok_or_else(|| StoreError::Integrity("stored text is empty".into()))
}

// ---------------------------------------------------------------------------
// engine_state
// ---------------------------------------------------------------------------

/// Returns the installation identity, minting it on first use.
///
/// The OS notification identifier is derived from this, so it must survive for
/// the life of the database; regenerating it would orphan every pending request
/// macOS already holds.
pub fn ensure_installation(
    tx: &Transaction<'_>,
    now: UnixTimestamp,
) -> Result<InstallationUuid, StoreError> {
    if let Some(existing) = tx
        .query_row(
            "SELECT installation_uuid FROM engine_state WHERE singleton_id = 1",
            [],
            |row| row.get::<_, String>(0),
        )
        .optional()?
    {
        let parsed = existing
            .parse::<uuid::Uuid>()
            .map_err(|e| StoreError::Integrity(format!("installation uuid: {e}")))?;
        return Ok(InstallationUuid::from_uuid(parsed));
    }

    let fresh = InstallationUuid::generate();
    tx.execute(
        "INSERT INTO engine_state (singleton_id, installation_uuid, created_at, updated_at)
         VALUES (1, ?1, ?2, ?2)",
        rusqlite::params![fresh.to_string(), now.get()],
    )?;
    Ok(fresh)
}

// ---------------------------------------------------------------------------
// media + source_media
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SourceMediaRow {
    pub source_media_id: SourceMediaId,
    pub anilist_id: AniListId,
    pub media_id: MediaId,
    pub current_observation_id: Option<ObservationId>,
}

pub fn find_source_media(
    conn: &Connection,
    anilist_id: AniListId,
) -> Result<Option<SourceMediaRow>, StoreError> {
    conn.query_row(
        "SELECT source_media_id, source_id, media_id, current_observation_id
         FROM source_media WHERE source = 'anilist' AND source_id = ?1",
        [anilist_id.get()],
        |row| {
            Ok((
                row.get::<_, i64>(0)?,
                row.get::<_, i64>(1)?,
                row.get::<_, i64>(2)?,
                row.get::<_, Option<i64>>(3)?,
            ))
        },
    )
    .optional()?
    .map(|(smid, sid, mid, obs)| {
        Ok(SourceMediaRow {
            source_media_id: SourceMediaId::new(smid)
                .map_err(|e| StoreError::Integrity(e.to_string()))?,
            anilist_id: AniListId::new(sid).map_err(|e| StoreError::Integrity(e.to_string()))?,
            media_id: MediaId::new(mid).map_err(|e| StoreError::Integrity(e.to_string()))?,
            current_observation_id: obs
                .map(ObservationId::new)
                .transpose()
                .map_err(|e| StoreError::Integrity(e.to_string()))?,
        })
    })
    .transpose()
}

/// Creates the canonical media row and its source identity together.
///
/// The current-observation pointer starts null and is set once the observation
/// exists; the composite foreign key makes any other order impossible.
pub fn create_media(
    tx: &Transaction<'_>,
    anilist_id: AniListId,
    display_title: &BoundedText,
    now: UnixTimestamp,
) -> Result<SourceMediaRow, StoreError> {
    tx.execute(
        "INSERT INTO media (kind, display_title, created_at, updated_at)
         VALUES ('anime', ?1, ?2, ?2)",
        rusqlite::params![display_title.as_str(), now.get()],
    )?;
    let media_id =
        MediaId::new(tx.last_insert_rowid()).map_err(|e| StoreError::Integrity(e.to_string()))?;

    tx.execute(
        "INSERT INTO source_media
            (source, source_id, media_id, current_observation_id, created_at, updated_at)
         VALUES ('anilist', ?1, ?2, NULL, ?3, ?3)",
        rusqlite::params![anilist_id.get(), media_id.get(), now.get()],
    )?;
    let source_media_id = SourceMediaId::new(tx.last_insert_rowid())
        .map_err(|e| StoreError::Integrity(e.to_string()))?;

    Ok(SourceMediaRow {
        source_media_id,
        anilist_id,
        media_id,
        current_observation_id: None,
    })
}

pub fn set_current_observation(
    tx: &Transaction<'_>,
    source_media_id: SourceMediaId,
    observation_id: ObservationId,
    now: UnixTimestamp,
) -> Result<(), StoreError> {
    tx.execute(
        "UPDATE source_media SET current_observation_id = ?1, updated_at = ?2
         WHERE source_media_id = ?3",
        rusqlite::params![observation_id.get(), now.get(), source_media_id.get()],
    )?;
    Ok(())
}

/// Keeps the canonical display title in step with the current observation.
pub fn set_display_title(
    tx: &Transaction<'_>,
    media_id: MediaId,
    title: &BoundedText,
    now: UnixTimestamp,
) -> Result<(), StoreError> {
    tx.execute(
        "UPDATE media SET display_title = ?1, updated_at = ?2 WHERE media_id = ?3",
        rusqlite::params![title.as_str(), now.get(), media_id.get()],
    )?;
    Ok(())
}

// ---------------------------------------------------------------------------
// follows
// ---------------------------------------------------------------------------

pub fn follow_state(
    conn: &Connection,
    media_id: MediaId,
) -> Result<Option<FollowState>, StoreError> {
    conn.query_row(
        "SELECT state FROM follows WHERE media_id = ?1",
        [media_id.get()],
        |row| row.get::<_, String>(0),
    )
    .optional()?
    .map(|raw| FollowState::parse(&raw).map_err(|e| StoreError::Integrity(e.to_string())))
    .transpose()
}

pub fn set_follow(
    tx: &Transaction<'_>,
    media_id: MediaId,
    state: FollowState,
    now: UnixTimestamp,
) -> Result<(), StoreError> {
    tx.execute(
        "INSERT INTO follows (media_id, state, followed_at, updated_at)
         VALUES (?1, ?2, ?3, ?3)
         ON CONFLICT(media_id) DO UPDATE SET state = excluded.state, updated_at = excluded.updated_at",
        rusqlite::params![media_id.get(), state.as_str(), now.get()],
    )?;
    Ok(())
}

pub fn active_follow_count(conn: &Connection) -> Result<u32, StoreError> {
    let count: i64 = conn.query_row(
        "SELECT count(*) FROM follows WHERE state = 'active'",
        [],
        |row| row.get(0),
    )?;
    Ok(u32::try_from(count).unwrap_or(u32::MAX))
}

// ---------------------------------------------------------------------------
// source_refresh_state
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RefreshState {
    pub request_generation: i64,
    pub last_success_at: Option<UnixTimestamp>,
    pub refresh_after: UnixTimestamp,
    pub consecutive_failures: i64,
    pub retry_after: Option<UnixTimestamp>,
}

pub fn refresh_state(
    conn: &Connection,
    source_media_id: SourceMediaId,
) -> Result<Option<RefreshState>, StoreError> {
    conn.query_row(
        "SELECT request_generation, last_success_at, refresh_after, consecutive_failures, retry_after
         FROM source_refresh_state WHERE source_media_id = ?1",
        [source_media_id.get()],
        |row| {
            Ok((
                row.get::<_, i64>(0)?,
                row.get::<_, Option<i64>>(1)?,
                row.get::<_, i64>(2)?,
                row.get::<_, i64>(3)?,
                row.get::<_, Option<i64>>(4)?,
            ))
        },
    )
    .optional()?
    .map(|(generation, success, after, failures, retry)| {
        Ok(RefreshState {
            request_generation: generation,
            last_success_at: success.map(ts).transpose()?,
            refresh_after: ts(after)?,
            consecutive_failures: failures,
            retry_after: retry.map(ts).transpose()?,
        })
    })
    .transpose()
}

/// Records a successful refresh and clears any backoff.
pub fn record_refresh_success(
    tx: &Transaction<'_>,
    source_media_id: SourceMediaId,
    now: UnixTimestamp,
    refresh_after: UnixTimestamp,
) -> Result<(), StoreError> {
    tx.execute(
        "INSERT INTO source_refresh_state
            (source_media_id, request_generation, last_attempt_at, last_success_at,
             refresh_after, consecutive_failures, last_error_code, last_error_at,
             retry_after, updated_at)
         VALUES (?1, 0, ?2, ?2, ?3, 0, NULL, NULL, NULL, ?2)
         ON CONFLICT(source_media_id) DO UPDATE SET
            last_attempt_at = excluded.last_attempt_at,
            last_success_at = excluded.last_success_at,
            refresh_after = excluded.refresh_after,
            consecutive_failures = 0,
            last_error_code = NULL,
            last_error_at = NULL,
            retry_after = NULL,
            updated_at = excluded.updated_at",
        rusqlite::params![source_media_id.get(), now.get(), refresh_after.get()],
    )?;
    Ok(())
}

/// Records a failure without touching last success or the serving projection.
pub fn record_refresh_failure(
    tx: &Transaction<'_>,
    source_media_id: SourceMediaId,
    now: UnixTimestamp,
    error_code: &str,
    retry_after: UnixTimestamp,
) -> Result<(), StoreError> {
    tx.execute(
        "INSERT INTO source_refresh_state
            (source_media_id, request_generation, last_attempt_at, refresh_after,
             consecutive_failures, last_error_code, last_error_at, retry_after, updated_at)
         VALUES (?1, 0, ?2, ?3, 1, ?4, ?2, ?3, ?2)
         ON CONFLICT(source_media_id) DO UPDATE SET
            last_attempt_at = excluded.last_attempt_at,
            refresh_after = excluded.refresh_after,
            consecutive_failures = source_refresh_state.consecutive_failures + 1,
            last_error_code = excluded.last_error_code,
            last_error_at = excluded.last_error_at,
            retry_after = excluded.retry_after,
            updated_at = excluded.updated_at",
        rusqlite::params![
            source_media_id.get(),
            now.get(),
            retry_after.get(),
            error_code
        ],
    )?;
    Ok(())
}

/// One active follow that is due for a refresh.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DueRow {
    pub source_media_id: SourceMediaId,
    pub anilist_id: AniListId,
    pub media_id: MediaId,
}

/// Active follows whose refresh deadline has passed, soonest first.
///
/// A `LEFT JOIN` so a follow that has never been refreshed is due immediately
/// rather than invisible: an inner join would silently never refresh anything
/// created before its first success row exists.
pub fn due_for_refresh(
    conn: &Connection,
    now: UnixTimestamp,
    limit: u32,
) -> Result<Vec<DueRow>, StoreError> {
    let mut stmt = conn.prepare(
        "SELECT sm.source_media_id, sm.source_id, sm.media_id
         FROM source_media sm
         JOIN follows f ON f.media_id = sm.media_id AND f.state = 'active'
         LEFT JOIN source_refresh_state rs ON rs.source_media_id = sm.source_media_id
         WHERE (rs.refresh_after IS NULL OR rs.refresh_after <= ?1)
           AND (rs.retry_after IS NULL OR rs.retry_after <= ?1)
         ORDER BY COALESCE(rs.refresh_after, 0) ASC, sm.source_media_id ASC
         LIMIT ?2",
    )?;
    let rows = stmt
        .query_map(rusqlite::params![now.get(), limit], |row| {
            Ok((
                row.get::<_, i64>(0)?,
                row.get::<_, i64>(1)?,
                row.get::<_, i64>(2)?,
            ))
        })?
        .collect::<Result<Vec<_>, _>>()?;

    rows.into_iter()
        .map(|(smid, sid, mid)| {
            let bad = |e: String| StoreError::Integrity(e);
            Ok(DueRow {
                source_media_id: SourceMediaId::new(smid).map_err(|e| bad(e.to_string()))?,
                anilist_id: AniListId::new(sid).map_err(|e| bad(e.to_string()))?,
                media_id: MediaId::new(mid).map_err(|e| bad(e.to_string()))?,
            })
        })
        .collect()
}

/// The earliest moment any active follow becomes due.
///
/// Drives the scheduler's sleep. `None` means nothing is scheduled at all,
/// which is what lets the loop block on a signal instead of polling.
pub fn earliest_due(conn: &Connection) -> Result<Option<UnixTimestamp>, StoreError> {
    conn.query_row(
        "SELECT min(COALESCE(MAX(rs.refresh_after, COALESCE(rs.retry_after, 0)), 0))
         FROM source_media sm
         JOIN follows f ON f.media_id = sm.media_id AND f.state = 'active'
         LEFT JOIN source_refresh_state rs ON rs.source_media_id = sm.source_media_id",
        [],
        |row| row.get::<_, Option<i64>>(0),
    )?
    .map(ts)
    .transpose()
}

/// Increments and returns the request generation to stamp on an in-flight call.
///
/// A response whose generation no longer matches may still be stored as
/// evidence but must not replace Silver or Gold state.
pub fn claim_generation(
    tx: &Transaction<'_>,
    source_media_id: SourceMediaId,
    now: UnixTimestamp,
) -> Result<i64, StoreError> {
    tx.execute(
        "INSERT INTO source_refresh_state
            (source_media_id, request_generation, refresh_after, updated_at)
         VALUES (?1, 1, ?2, ?2)
         ON CONFLICT(source_media_id) DO UPDATE SET
            request_generation = source_refresh_state.request_generation + 1,
            updated_at = excluded.updated_at",
        rusqlite::params![source_media_id.get(), now.get()],
    )?;
    let generation: i64 = tx.query_row(
        "SELECT request_generation FROM source_refresh_state WHERE source_media_id = ?1",
        [source_media_id.get()],
        |row| row.get(0),
    )?;
    Ok(generation)
}

/// Whether `generation` is still the newest claim for this media.
pub fn generation_is_current(
    conn: &Connection,
    source_media_id: SourceMediaId,
    generation: i64,
) -> Result<bool, StoreError> {
    let current: Option<i64> = conn
        .query_row(
            "SELECT request_generation FROM source_refresh_state WHERE source_media_id = ?1",
            [source_media_id.get()],
            |row| row.get(0),
        )
        .optional()?;
    Ok(current == Some(generation))
}

// ---------------------------------------------------------------------------
// source_runtime_state
// ---------------------------------------------------------------------------

pub fn source_blocked_until(conn: &Connection) -> Result<Option<UnixTimestamp>, StoreError> {
    conn.query_row(
        "SELECT blocked_until FROM source_runtime_state WHERE source = 'anilist'",
        [],
        |row| row.get::<_, Option<i64>>(0),
    )
    .optional()?
    .flatten()
    .map(ts)
    .transpose()
}

/// Persists the source-global throttle so a 429 outlives the process.
pub fn set_source_throttle(
    tx: &Transaction<'_>,
    blocked_until: Option<UnixTimestamp>,
    remaining: Option<u32>,
    reset_at: Option<i64>,
    now: UnixTimestamp,
) -> Result<(), StoreError> {
    tx.execute(
        "INSERT INTO source_runtime_state
            (source, blocked_until, rate_limit_remaining, rate_limit_reset_at, updated_at)
         VALUES ('anilist', ?1, ?2, ?3, ?4)
         ON CONFLICT(source) DO UPDATE SET
            blocked_until = excluded.blocked_until,
            rate_limit_remaining = excluded.rate_limit_remaining,
            rate_limit_reset_at = excluded.rate_limit_reset_at,
            updated_at = excluded.updated_at",
        rusqlite::params![
            blocked_until.map(UnixTimestamp::get),
            remaining,
            reset_at,
            now.get()
        ],
    )?;
    Ok(())
}

// ---------------------------------------------------------------------------
// evidence: fetches and observations
// ---------------------------------------------------------------------------

#[derive(Debug, Clone)]
pub struct FetchRecord<'a> {
    pub attempt_uuid: &'a str,
    pub request_kind: &'a str,
    pub request_fingerprint: &'a str,
    pub requested_at: UnixTimestamp,
    pub completed_at: UnixTimestamp,
    pub outcome: &'a str,
    pub http_status: Option<u16>,
    pub retry_after: Option<u32>,
    pub rate_limit_remaining: Option<u32>,
    pub rate_limit_reset_at: Option<i64>,
    pub body_json: Option<&'a str>,
    pub error_code: Option<&'a str>,
}

pub fn insert_fetch(tx: &Transaction<'_>, record: &FetchRecord<'_>) -> Result<FetchId, StoreError> {
    tx.execute(
        "INSERT INTO source_fetches
            (attempt_uuid, source, request_kind, request_fingerprint, requested_at,
             completed_at, outcome, http_status, retry_after, rate_limit_remaining,
             rate_limit_reset_at, body_json, byte_length, error_code)
         VALUES (?1, 'anilist', ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13)",
        rusqlite::params![
            record.attempt_uuid,
            record.request_kind,
            record.request_fingerprint,
            record.requested_at.get(),
            record.completed_at.get(),
            record.outcome,
            record.http_status,
            record.retry_after,
            record.rate_limit_remaining,
            record.rate_limit_reset_at,
            record.body_json,
            record
                .body_json
                .map(|b| i64::try_from(b.len()).unwrap_or(i64::MAX)),
            record.error_code,
        ],
    )?;
    FetchId::new(tx.last_insert_rowid()).map_err(|e| StoreError::Integrity(e.to_string()))
}

/// Appends one observation occurrence.
///
/// Never deduplicates: an identical payload observed twice is two occurrences,
/// because the second is evidence the source still asserts those facts.
pub fn insert_observation(
    tx: &Transaction<'_>,
    source_media_id: SourceMediaId,
    fetch_id: FetchId,
    observation: &MediaObservation,
    observed_at: UnixTimestamp,
) -> Result<ObservationId, StoreError> {
    tx.execute(
        "INSERT INTO source_observations
            (source_media_id, fetch_id, parser_version, observed_at, display_title,
             title_english, title_romaji, title_native, status, status_raw, format_raw,
             episode_count, season_year, next_episode, next_airing_at)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15)",
        rusqlite::params![
            source_media_id.get(),
            fetch_id.get(),
            observation.parser_version,
            observed_at.get(),
            observation.display_title.as_str(),
            observation.titles.english.as_ref().map(BoundedText::as_str),
            observation.titles.romaji.as_ref().map(BoundedText::as_str),
            observation.titles.native.as_ref().map(BoundedText::as_str),
            observation.status.as_str(),
            observation.status_raw.as_ref().map(BoundedText::as_str),
            observation.format_raw.as_ref().map(BoundedText::as_str),
            observation.episode_count.map(EpisodeNumber::get),
            observation.season_year,
            observation.next_airing.map(|n| n.episode.get()),
            observation.next_airing.map(|n| n.airing_at.get()),
        ],
    )?;
    ObservationId::new(tx.last_insert_rowid()).map_err(|e| StoreError::Integrity(e.to_string()))
}

/// Reads back the current observation for a source media.
pub fn current_observation(
    conn: &Connection,
    source_media_id: SourceMediaId,
) -> Result<Option<MediaObservation>, StoreError> {
    conn.query_row(
        "SELECT o.display_title, o.title_english, o.title_romaji, o.title_native,
                o.status, o.status_raw, o.format_raw, o.episode_count, o.season_year,
                o.next_episode, o.next_airing_at, o.parser_version, sm.source_id
         FROM source_media sm
         JOIN source_observations o ON o.observation_id = sm.current_observation_id
         WHERE sm.source_media_id = ?1",
        [source_media_id.get()],
        |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, Option<String>>(1)?,
                row.get::<_, Option<String>>(2)?,
                row.get::<_, Option<String>>(3)?,
                row.get::<_, String>(4)?,
                row.get::<_, Option<String>>(5)?,
                row.get::<_, Option<String>>(6)?,
                row.get::<_, Option<i64>>(7)?,
                row.get::<_, Option<i64>>(8)?,
                row.get::<_, Option<i64>>(9)?,
                row.get::<_, Option<i64>>(10)?,
                row.get::<_, i64>(11)?,
                row.get::<_, i64>(12)?,
            ))
        },
    )
    .optional()?
    .map(|row| {
        let (
            display_title,
            english,
            romaji,
            native,
            status,
            status_raw,
            format_raw,
            episode_count,
            season_year,
            next_episode,
            next_airing_at,
            parser_version,
            source_id,
        ) = row;

        let next_airing = match (next_episode, next_airing_at) {
            (Some(episode), Some(airing_at)) => Some(NextAiring {
                episode: EpisodeNumber::new(episode)
                    .map_err(|e| StoreError::Integrity(e.to_string()))?,
                airing_at: ts(airing_at)?,
            }),
            // The schema CHECK makes a half-populated pair unstorable, so this
            // arm is only reachable via a database written by something else.
            (None, None) => None,
            _ => return Err(StoreError::Integrity("half-populated schedule".into())),
        };

        Ok(MediaObservation {
            source_key: SourceKey::anilist(
                AniListId::new(source_id).map_err(|e| StoreError::Integrity(e.to_string()))?,
            ),
            display_title: text(display_title, MAX_TITLE_LEN)?,
            titles: TitleSet {
                english: english.map(|v| text(v, MAX_TITLE_LEN)).transpose()?,
                romaji: romaji.map(|v| text(v, MAX_TITLE_LEN)).transpose()?,
                native: native.map(|v| text(v, MAX_TITLE_LEN)).transpose()?,
            },
            status: MediaStatus::parse(&status)
                .map_err(|e| StoreError::Integrity(e.to_string()))?,
            status_raw: status_raw.map(|v| text(v, MAX_RAW_LEN)).transpose()?,
            format_raw: format_raw.map(|v| text(v, MAX_RAW_LEN)).transpose()?,
            episode_count: episode_count
                .map(EpisodeNumber::new)
                .transpose()
                .map_err(|e| StoreError::Integrity(e.to_string()))?,
            season_year: season_year.and_then(|y| i32::try_from(y).ok()),
            next_airing,
            parser_version,
        })
    })
    .transpose()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::media::PARSER_VERSION;
    use crate::store::{connection::configure, migrations};

    fn db() -> Connection {
        let mut conn = Connection::open_in_memory().expect("open");
        configure(&conn, false).expect("configure");
        migrations::apply(&mut conn, 100).expect("migrate");
        conn
    }

    fn at(seconds: i64) -> UnixTimestamp {
        UnixTimestamp::new(seconds).expect("valid timestamp")
    }

    fn id(value: i64) -> AniListId {
        AniListId::new(value).expect("valid id")
    }

    fn title(value: &str) -> BoundedText {
        BoundedText::new("t", MAX_TITLE_LEN, value).expect("valid title")
    }

    fn observation(anilist_id: i64, next: Option<(i64, i64)>) -> MediaObservation {
        MediaObservation {
            source_key: SourceKey::anilist(id(anilist_id)),
            display_title: title("One Piece"),
            titles: TitleSet {
                english: Some(title("One Piece")),
                romaji: Some(title("ONE PIECE")),
                native: None,
            },
            status: MediaStatus::Releasing,
            status_raw: None,
            format_raw: Some(title("TV")),
            episode_count: None,
            season_year: Some(1999),
            next_airing: next.map(|(episode, airing_at)| NextAiring {
                episode: EpisodeNumber::new(episode).expect("valid episode"),
                airing_at: at(airing_at),
            }),
            parser_version: PARSER_VERSION,
        }
    }

    fn fetch(tx: &Transaction<'_>, uuid: &str) -> FetchId {
        insert_fetch(
            tx,
            &FetchRecord {
                attempt_uuid: uuid,
                request_kind: "detail",
                request_fingerprint: "id=21",
                requested_at: at(100),
                completed_at: at(101),
                outcome: "success",
                http_status: Some(200),
                retry_after: None,
                rate_limit_remaining: Some(88),
                rate_limit_reset_at: None,
                body_json: Some("{}"),
                error_code: None,
            },
        )
        .expect("insert fetch")
    }

    #[test]
    fn installation_identity_is_minted_once_and_reused() {
        // Regenerating it would orphan every pending request macOS holds.
        let mut conn = db();
        let tx = conn.transaction().expect("begin");
        let first = ensure_installation(&tx, at(100)).expect("mint");
        let second = ensure_installation(&tx, at(200)).expect("reuse");
        assert_eq!(first, second);
        tx.commit().expect("commit");

        let tx = conn.transaction().expect("begin");
        assert_eq!(ensure_installation(&tx, at(300)).expect("reuse"), first);
    }

    #[test]
    fn creating_media_wires_up_both_identities() {
        let mut conn = db();
        let tx = conn.transaction().expect("begin");
        let row = create_media(&tx, id(21), &title("One Piece"), at(100)).expect("create");
        tx.commit().expect("commit");

        assert_eq!(row.anilist_id, id(21));
        assert_eq!(row.current_observation_id, None);
        let found = find_source_media(&conn, id(21))
            .expect("find")
            .expect("present");
        assert_eq!(found, row);
    }

    #[test]
    fn an_unknown_anilist_id_is_absent_rather_than_an_error() {
        let conn = db();
        assert_eq!(find_source_media(&conn, id(999)).expect("find"), None);
    }

    #[test]
    fn identical_observations_are_two_occurrences() {
        // The core Bronze/Silver guarantee: repetition is itself evidence.
        let mut conn = db();
        let tx = conn.transaction().expect("begin");
        let row = create_media(&tx, id(21), &title("One Piece"), at(100)).expect("create");
        let f1 = fetch(&tx, "a");
        let f2 = fetch(&tx, "b");
        let o1 = insert_observation(
            &tx,
            row.source_media_id,
            f1,
            &observation(21, Some((5, 5000))),
            at(101),
        )
        .expect("first");
        let o2 = insert_observation(
            &tx,
            row.source_media_id,
            f2,
            &observation(21, Some((5, 5000))),
            at(102),
        )
        .expect("second");
        tx.commit().expect("commit");

        assert_ne!(o1, o2);
        let count: i64 = conn
            .query_row("SELECT count(*) FROM source_observations", [], |r| r.get(0))
            .expect("count");
        assert_eq!(count, 2);
    }

    #[test]
    fn the_current_observation_round_trips() {
        let mut conn = db();
        let tx = conn.transaction().expect("begin");
        let row = create_media(&tx, id(21), &title("One Piece"), at(100)).expect("create");
        let f = fetch(&tx, "a");
        let original = observation(21, Some((1169, 1_783_865_760)));
        let obs =
            insert_observation(&tx, row.source_media_id, f, &original, at(101)).expect("insert");
        set_current_observation(&tx, row.source_media_id, obs, at(101)).expect("point");
        tx.commit().expect("commit");

        let read = current_observation(&conn, row.source_media_id)
            .expect("read")
            .expect("present");
        assert_eq!(read, original);
    }

    #[test]
    fn an_observation_without_a_schedule_round_trips_as_none() {
        let mut conn = db();
        let tx = conn.transaction().expect("begin");
        let row = create_media(&tx, id(21), &title("One Piece"), at(100)).expect("create");
        let f = fetch(&tx, "a");
        let original = observation(21, None);
        let obs =
            insert_observation(&tx, row.source_media_id, f, &original, at(101)).expect("insert");
        set_current_observation(&tx, row.source_media_id, obs, at(101)).expect("point");
        tx.commit().expect("commit");

        let read = current_observation(&conn, row.source_media_id)
            .expect("read")
            .expect("present");
        assert_eq!(read.next_airing, None);
    }

    #[test]
    fn follow_state_toggles_without_duplicating_rows() {
        let mut conn = db();
        let tx = conn.transaction().expect("begin");
        let row = create_media(&tx, id(21), &title("One Piece"), at(100)).expect("create");
        set_follow(&tx, row.media_id, FollowState::Active, at(100)).expect("follow");
        set_follow(&tx, row.media_id, FollowState::Dropped, at(200)).expect("drop");
        set_follow(&tx, row.media_id, FollowState::Active, at(300)).expect("refollow");
        tx.commit().expect("commit");

        assert_eq!(
            follow_state(&conn, row.media_id).expect("read"),
            Some(FollowState::Active)
        );
        let count: i64 = conn
            .query_row("SELECT count(*) FROM follows", [], |r| r.get(0))
            .expect("count");
        assert_eq!(count, 1);
        assert_eq!(active_follow_count(&conn).expect("count"), 1);
    }

    #[test]
    fn dropped_follows_do_not_count_as_active() {
        let mut conn = db();
        let tx = conn.transaction().expect("begin");
        let row = create_media(&tx, id(21), &title("One Piece"), at(100)).expect("create");
        set_follow(&tx, row.media_id, FollowState::Dropped, at(100)).expect("drop");
        tx.commit().expect("commit");
        assert_eq!(active_follow_count(&conn).expect("count"), 0);
    }

    #[test]
    fn generations_increase_monotonically() {
        let mut conn = db();
        let tx = conn.transaction().expect("begin");
        let row = create_media(&tx, id(21), &title("One Piece"), at(100)).expect("create");
        let first = claim_generation(&tx, row.source_media_id, at(100)).expect("claim");
        let second = claim_generation(&tx, row.source_media_id, at(101)).expect("claim");
        tx.commit().expect("commit");

        assert_eq!(first, 1);
        assert_eq!(second, 2);
    }

    #[test]
    fn a_superseded_generation_is_no_longer_current() {
        // This is what stops a slow response from rolling state backward.
        let mut conn = db();
        let tx = conn.transaction().expect("begin");
        let row = create_media(&tx, id(21), &title("One Piece"), at(100)).expect("create");
        let stale = claim_generation(&tx, row.source_media_id, at(100)).expect("claim");
        let fresh = claim_generation(&tx, row.source_media_id, at(101)).expect("claim");
        tx.commit().expect("commit");

        assert!(!generation_is_current(&conn, row.source_media_id, stale).expect("check"));
        assert!(generation_is_current(&conn, row.source_media_id, fresh).expect("check"));
    }

    #[test]
    fn failures_accumulate_and_success_resets_them() {
        let mut conn = db();
        let tx = conn.transaction().expect("begin");
        let row = create_media(&tx, id(21), &title("One Piece"), at(100)).expect("create");
        record_refresh_failure(&tx, row.source_media_id, at(100), "http_503", at(160))
            .expect("fail");
        record_refresh_failure(&tx, row.source_media_id, at(200), "http_503", at(500))
            .expect("fail");
        tx.commit().expect("commit");

        let state = refresh_state(&conn, row.source_media_id)
            .expect("read")
            .expect("present");
        assert_eq!(state.consecutive_failures, 2);
        assert_eq!(state.last_success_at, None);
        assert_eq!(state.retry_after, Some(at(500)));

        let tx = conn.transaction().expect("begin");
        record_refresh_success(&tx, row.source_media_id, at(300), at(3900)).expect("succeed");
        tx.commit().expect("commit");

        let state = refresh_state(&conn, row.source_media_id)
            .expect("read")
            .expect("present");
        assert_eq!(state.consecutive_failures, 0);
        assert_eq!(state.last_success_at, Some(at(300)));
        assert_eq!(state.retry_after, None);
    }

    #[test]
    fn a_failure_preserves_the_previous_success() {
        // Section 3: failure changes only durable failure state.
        let mut conn = db();
        let tx = conn.transaction().expect("begin");
        let row = create_media(&tx, id(21), &title("One Piece"), at(100)).expect("create");
        record_refresh_success(&tx, row.source_media_id, at(100), at(3700)).expect("succeed");
        record_refresh_failure(&tx, row.source_media_id, at(200), "timeout", at(260))
            .expect("fail");
        tx.commit().expect("commit");

        let state = refresh_state(&conn, row.source_media_id)
            .expect("read")
            .expect("present");
        assert_eq!(state.last_success_at, Some(at(100)));
        assert_eq!(state.consecutive_failures, 1);
    }

    #[test]
    fn the_source_throttle_survives_a_reopen() {
        let mut conn = db();
        let tx = conn.transaction().expect("begin");
        set_source_throttle(&tx, Some(at(9_000)), Some(0), Some(9_000), at(100)).expect("throttle");
        tx.commit().expect("commit");

        assert_eq!(source_blocked_until(&conn).expect("read"), Some(at(9_000)));
    }

    #[test]
    fn no_throttle_reads_as_absent() {
        let conn = db();
        assert_eq!(source_blocked_until(&conn).expect("read"), None);
    }
}
