//! Bounded evidence retention; never discards current facts or live notification intent.
use super::connection::StoreError;
use rusqlite::Transaction;

pub const EVIDENCE_BUDGET: i64 = 32 * 1024 * 1024;
pub const EVIDENCE_AGE_SECS: i64 = 30 * 86400;

pub fn prune(tx: &Transaction<'_>, now: i64) -> Result<(), StoreError> {
    // Keep the latest event for sequence integrity, plus live/recent events.
    tx.execute(
        "DELETE FROM notification_jobs WHERE release_event_id IN (
        SELECT release_event_id FROM release_events WHERE state != 'scheduled'
        AND last_observed_at < ?1 AND release_event_id NOT IN
        (SELECT MAX(release_event_id) FROM release_events GROUP BY source_media_id))",
        [now.saturating_sub(90 * 86400)],
    )?;
    tx.execute(
        "DELETE FROM release_events WHERE state != 'scheduled'
        AND last_observed_at < ?1 AND release_event_id NOT IN
        (SELECT MAX(release_event_id) FROM release_events GROUP BY source_media_id)",
        [now.saturating_sub(90 * 86400)],
    )?;
    tx.execute(
        "DELETE FROM source_observations WHERE observation_id NOT IN (
        SELECT current_observation_id FROM source_media WHERE current_observation_id IS NOT NULL)
        AND observation_id NOT IN (SELECT last_observation_id FROM release_events)
        AND (observed_at < ?1 OR observation_id IN (
            SELECT observation_id FROM (SELECT observation_id,
              row_number() OVER (PARTITION BY source_media_id ORDER BY observation_id DESC) AS rank
              FROM source_observations) WHERE rank > 8))",
        [now.saturating_sub(EVIDENCE_AGE_SECS)],
    )?;
    tx.execute(
        "UPDATE source_fetches SET body_json = NULL, byte_length = NULL, body_pruned = 1
        WHERE body_json IS NOT NULL AND (completed_at < ?1 OR fetch_id IN (
          SELECT fetch_id FROM (SELECT fetch_id,
            SUM(COALESCE(byte_length,0)) OVER (ORDER BY fetch_id DESC) AS bytes
            FROM source_fetches) WHERE bytes > ?2))",
        rusqlite::params![now.saturating_sub(EVIDENCE_AGE_SECS), EVIDENCE_BUDGET],
    )?;
    // Keep at most 1,000 unreferenced metadata rows. Referenced facts remain valid.
    tx.execute(
        "DELETE FROM source_fetches WHERE fetch_id NOT IN
        (SELECT fetch_id FROM source_observations) AND fetch_id NOT IN
        (SELECT fetch_id FROM source_fetches ORDER BY fetch_id DESC LIMIT 1000)",
        [],
    )?;
    Ok(())
}

pub fn forget(tx: &Transaction<'_>, media_id: i64) -> Result<bool, StoreError> {
    if tx.query_row(
        "SELECT count(*) FROM media WHERE media_id=?1",
        [media_id],
        |r| r.get::<_, i64>(0),
    )? == 0
    {
        return Ok(false);
    }
    tx.execute("INSERT OR IGNORE INTO notification_removals SELECT os_identifier FROM notification_jobs WHERE release_event_id IN (SELECT release_event_id FROM release_events WHERE media_id=?1)", [media_id])?;
    tx.execute("DELETE FROM notification_jobs WHERE release_event_id IN (SELECT release_event_id FROM release_events WHERE media_id=?1)", [media_id])?;
    tx.execute("DELETE FROM release_events WHERE media_id=?1", [media_id])?;
    tx.execute(
        "UPDATE source_media SET current_observation_id=NULL WHERE media_id=?1",
        [media_id],
    )?;
    tx.execute("DELETE FROM source_refresh_state WHERE source_media_id IN (SELECT source_media_id FROM source_media WHERE media_id=?1)", [media_id])?;
    tx.execute("DELETE FROM source_observations WHERE source_media_id IN (SELECT source_media_id FROM source_media WHERE media_id=?1)", [media_id])?;
    tx.execute("DELETE FROM source_media WHERE media_id=?1", [media_id])?;
    tx.execute("DELETE FROM follows WHERE media_id=?1", [media_id])?;
    tx.execute("DELETE FROM media WHERE media_id=?1", [media_id])?;
    // Batch bodies may mention multiple titles; discard all raw bodies to avoid retaining the forgotten title.
    tx.execute(
        "UPDATE source_fetches SET body_json=NULL, byte_length=NULL, body_pruned=1",
        [],
    )?;
    tx.execute("DELETE FROM source_fetches WHERE fetch_id NOT IN (SELECT fetch_id FROM source_observations)", [])?;
    tx.execute("UPDATE operations SET message=NULL", [])?;
    Ok(true)
}
