//! Local Command Center projections. No source or UI dependencies.

use rusqlite::{Connection, OptionalExtension, Transaction};

use crate::domain::command_center::*;
use crate::domain::ids::{MediaId, Source, SourceKey, SourceNumericId, UnixTimestamp};
use crate::domain::media::MediaKind;
use crate::domain::release::FollowState;

use super::connection::StoreError;
use super::{graph, read_models};

fn bad(error: impl std::fmt::Display) -> StoreError {
    StoreError::Integrity(error.to_string())
}

pub fn detail(
    conn: &Connection,
    key: SourceKey,
    now: UnixTimestamp,
) -> Result<Option<MediaDetail>, StoreError> {
    let Some(row) = graph::find_source_media(conn, key)? else {
        return Ok(None);
    };
    let Some(facts) = graph::current_observation(conn, row.source_media_id)? else {
        return Ok(None);
    };
    let (state, success, refresh, retry, revision, notification) = conn.query_row(
        "SELECT f.state, rs.last_success_at, rs.refresh_after, rs.retry_after,
                re.schedule_revision, nj.state
         FROM source_media sm
         LEFT JOIN follows f ON f.media_id = sm.media_id
         LEFT JOIN source_refresh_state rs ON rs.source_media_id = sm.source_media_id
         LEFT JOIN release_events re ON re.source_media_id = sm.source_media_id AND re.state = 'scheduled'
         LEFT JOIN notification_jobs nj ON nj.release_event_id = re.release_event_id
         WHERE sm.source_media_id = ?1",
        [row.source_media_id.get()],
        |r| Ok((r.get::<_, Option<String>>(0)?, r.get::<_, Option<i64>>(1)?, r.get::<_, Option<i64>>(2)?, r.get::<_, Option<i64>>(3)?, r.get::<_, Option<i64>>(4)?, r.get::<_, Option<String>>(5)?)),
    )?;
    Ok(Some(MediaDetail {
        media_id: row.media_id,
        facts,
        follow_state: state
            .map(|s| FollowState::parse(&s).map_err(bad))
            .transpose()?,
        last_success_at: success
            .map(|s| UnixTimestamp::new(s).map_err(bad))
            .transpose()?,
        freshness: read_models::freshness(now, refresh, retry),
        schedule_revision: revision,
        notification_state: notification,
    }))
}

pub fn follows(
    conn: &Connection,
    now: UnixTimestamp,
    kind: Option<MediaKind>,
    state: FollowState,
    sort: FollowSort,
    offset: u32,
) -> Result<Vec<MediaDetail>, StoreError> {
    let mut stmt = conn.prepare(
        "SELECT sm.source, sm.source_id
         FROM follows f JOIN media m ON m.media_id = f.media_id
         JOIN source_media sm ON sm.media_id = m.media_id
         WHERE f.state = ?1 AND (?2 IS NULL OR m.kind = ?2)
         ORDER BY CASE WHEN ?3 = 'next' THEN COALESCE((SELECT MIN(re.scheduled_at) FROM release_events re WHERE re.media_id = m.media_id AND re.state = 'scheduled' AND re.scheduled_at >= ?4), 9223372036854775807) ELSE 0 END,
                  m.display_title COLLATE NOCASE, m.media_id
         LIMIT ?5 OFFSET ?6",
    )?;
    let keys = stmt
        .query_map(
            rusqlite::params![
                state.as_str(),
                kind.map(MediaKind::as_str),
                if sort == FollowSort::NextRelease {
                    "next"
                } else {
                    "alpha"
                },
                now.get(),
                PAGE_SIZE + 1,
                offset
            ],
            |r| Ok((r.get::<_, String>(0)?, r.get::<_, i64>(1)?)),
        )?
        .collect::<Result<Vec<_>, _>>()?;
    keys.into_iter()
        .map(|(source, id)| {
            let key = SourceKey {
                source: Source::parse(&source).map_err(bad)?,
                id: SourceNumericId::new(id).map_err(bad)?,
            };
            detail(conn, key, now)?.ok_or_else(|| bad("follow has no observation"))
        })
        .collect()
}

pub fn follow_counts(conn: &Connection) -> Result<(u32, u32), StoreError> {
    Ok(conn.query_row("SELECT COALESCE(SUM(m.kind = 'anime'), 0), COALESCE(SUM(m.kind = 'tv'), 0) FROM follows f JOIN media m ON m.media_id = f.media_id WHERE f.state = 'active'", [], |r| Ok((r.get(0)?, r.get(1)?)))?)
}

pub fn interrupt_operations(tx: &Transaction<'_>, now: UnixTimestamp) -> Result<(), StoreError> {
    tx.execute("UPDATE operations SET state = 'failed', updated_at = ?1, message = 'The service restarted before this refresh completed. Retry the refresh.' WHERE state IN ('queued', 'running')", [now.get()])?;
    Ok(())
}

pub fn operations(conn: &Connection) -> Result<Vec<OperationStatus>, StoreError> {
    let mut stmt = conn.prepare("SELECT operation_id, target_json, state, updated_at, message FROM operations ORDER BY updated_at DESC, operation_id LIMIT 10")?;
    let rows = stmt
        .query_map([], |r| {
            Ok((
                r.get::<_, String>(0)?,
                r.get::<_, String>(1)?,
                r.get::<_, String>(2)?,
                r.get::<_, i64>(3)?,
                r.get::<_, Option<String>>(4)?,
            ))
        })?
        .collect::<Result<Vec<_>, _>>()?;
    rows.into_iter()
        .map(|(id, target, state, time, message)| {
            Ok(OperationStatus {
                id,
                target: serde_json::from_str(&target).map_err(bad)?,
                state: serde_json::from_value(serde_json::Value::String(state)).map_err(bad)?,
                updated_at: UnixTimestamp::new(time).map_err(bad)?,
                message,
            })
        })
        .collect()
}

pub fn set_operation(tx: &Transaction<'_>, operation: &OperationStatus) -> Result<(), StoreError> {
    let state = serde_json::to_value(operation.state).map_err(bad)?;
    tx.execute("INSERT INTO operations (operation_id, target_json, state, updated_at, message) VALUES (?1, ?2, ?3, ?4, ?5) ON CONFLICT(operation_id) DO UPDATE SET state=excluded.state, updated_at=excluded.updated_at, message=excluded.message", rusqlite::params![operation.id, serde_json::to_string(&operation.target).map_err(bad)?, state.as_str(), operation.updated_at.get(), operation.message])?;
    tx.execute("DELETE FROM operations WHERE state NOT IN ('queued', 'running') AND operation_id NOT IN (SELECT operation_id FROM operations ORDER BY updated_at DESC, operation_id LIMIT 20)", [])?;
    Ok(())
}

pub fn sources(conn: &Connection, now: UnixTimestamp) -> Result<Vec<SourceHealth>, StoreError> {
    [Source::AniList, Source::TvMaze].into_iter().map(|source| {
        let success = conn.query_row("SELECT MAX(rs.last_success_at) FROM source_refresh_state rs JOIN source_media sm ON sm.source_media_id = rs.source_media_id WHERE sm.source = ?1", [source.as_str()], |r| r.get::<_, Option<i64>>(0))?;
        Ok(SourceHealth {
            source, blocked_until: graph::source_blocked_until(conn, source)?.filter(|t| t.get() > now.get()),
            last_success_at: success.map(|s| UnixTimestamp::new(s).map_err(bad)).transpose()?,
        })
    }).collect()
}

pub fn media_key(conn: &Connection, media_id: MediaId) -> Result<Option<SourceKey>, StoreError> {
    conn.query_row(
        "SELECT source, source_id FROM source_media WHERE media_id = ?1",
        [media_id.get()],
        |r| Ok((r.get::<_, String>(0)?, r.get::<_, i64>(1)?)),
    )
    .optional()?
    .map(|(source, id)| {
        Ok(SourceKey {
            source: Source::parse(&source).map_err(bad)?,
            id: SourceNumericId::new(id).map_err(bad)?,
        })
    })
    .transpose()
}

#[derive(Debug)]
pub struct SnapshotContext {
    pub stamp: RevisionStamp,
    pub now: UnixTimestamp,
    pub started_at: UnixTimestamp,
    pub schema_version: i64,
    pub protocol_version: u32,
}

pub fn snapshot(
    conn: &Connection,
    query: ViewQuery,
    context: SnapshotContext,
) -> Result<Option<ViewSnapshot>, StoreError> {
    let tx = conn.unchecked_transaction()?;
    let now = context.now;
    let health = read_models::health(
        &tx,
        now,
        context.stamp.instance_id.clone(),
        context.started_at,
        context.schema_version,
        context.protocol_version,
    )?;
    let view = match query {
        ViewQuery::Home => {
            let (anime, tv) = follow_counts(&tx)?;
            let mut dropped = read_models::upcoming_page(&tx, now, 12, 86400, None, 0, true)?;
            dropped.reverse();
            ViewData::Home {
                anime,
                tv,
                dropped,
                upcoming: read_models::upcoming(&tx, now, 8, 0)?,
            }
        }
        ViewQuery::Library {
            kind,
            state,
            sort,
            cursor,
        } => {
            let scope = format!("library:{kind:?}:{state:?}:{sort:?}");
            let offset = cursor.map_or(0, |c| c.offset);
            ViewData::Library(page(
                follows(&tx, now, kind, state, sort, offset)?,
                &context.stamp,
                scope,
                offset,
            ))
        }
        ViewQuery::Schedule { kind, cursor } => {
            let scope = format!("schedule:{kind:?}");
            let offset = cursor.map_or(0, |c| c.offset);
            ViewData::Schedule(page(
                read_models::upcoming_page(&tx, now, PAGE_SIZE + 1, 86400, kind, offset, false)?,
                &context.stamp,
                scope,
                offset,
            ))
        }
        ViewQuery::Detail { key } => {
            let Some(detail) = detail(&tx, key, now)? else {
                return Ok(None);
            };
            ViewData::Detail(Box::new(detail))
        }
        ViewQuery::Health => ViewData::Health {
            sources: sources(&tx, now)?,
            operations: operations(&tx)?,
        },
    };
    tx.commit()?;
    Ok(Some(ViewSnapshot {
        stamp: context.stamp,
        generated_at: now,
        health,
        view,
    }))
}

fn page<T>(mut items: Vec<T>, stamp: &RevisionStamp, scope: String, offset: u32) -> Page<T> {
    let next = (items.len() > PAGE_SIZE as usize).then(|| PageCursor {
        instance_id: stamp.instance_id.clone(),
        revision: stamp.revision,
        offset: offset.saturating_add(PAGE_SIZE),
        scope,
    });
    items.truncate(PAGE_SIZE as usize);
    Page { items, next }
}

pub fn begin_operation(
    tx: &Transaction<'_>,
    operation: OperationStatus,
) -> Result<(OperationStatus, bool), StoreError> {
    let target = serde_json::to_string(&operation.target).map_err(bad)?;
    let existing = tx.query_row("SELECT operation_id, state, updated_at FROM operations WHERE target_json = ?1 AND state IN ('queued', 'running')", [&target], |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?, r.get::<_, i64>(2)?))).optional()?;
    if let Some((id, state, time)) = existing {
        return Ok((
            OperationStatus {
                id,
                target: operation.target,
                state: serde_json::from_value(serde_json::Value::String(state)).map_err(bad)?,
                updated_at: UnixTimestamp::new(time).map_err(bad)?,
                message: None,
            },
            false,
        ));
    }
    set_operation(tx, &operation)?;
    Ok((operation, true))
}
