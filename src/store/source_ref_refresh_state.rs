use anyhow::{Context, Result};
use rusqlite::{params, OptionalExtension};

use super::Db;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SourceRefRefreshState {
    pub source: String,
    pub source_id: String,
    pub last_attempt_at: Option<i64>,
    pub last_success_at: Option<i64>,
    pub last_error: Option<String>,
    pub next_due_at: Option<i64>,
    pub failure_count: i64,
}

impl Db {
    pub fn upsert_source_ref_refresh_state(&self, state: &SourceRefRefreshState) -> Result<()> {
        self.conn()
            .execute(
                "INSERT INTO source_ref_refresh_state
                    (source, source_id, last_attempt_at, last_success_at, last_error, next_due_at, failure_count)
                 VALUES (?1,?2,?3,?4,?5,?6,?7)
                 ON CONFLICT(source, source_id) DO UPDATE SET
                    last_attempt_at = excluded.last_attempt_at,
                    last_success_at = excluded.last_success_at,
                    last_error = excluded.last_error,
                    next_due_at = excluded.next_due_at,
                    failure_count = excluded.failure_count",
                params![
                    state.source,
                    state.source_id,
                    state.last_attempt_at,
                    state.last_success_at,
                    state.last_error,
                    state.next_due_at,
                    state.failure_count,
                ],
            )
            .context("upsert source_ref_refresh_state")?;
        Ok(())
    }

    pub fn get_source_ref_refresh_state(
        &self,
        source: &str,
        source_id: &str,
    ) -> Result<Option<SourceRefRefreshState>> {
        self.conn()
            .query_row(
                "SELECT source, source_id, last_attempt_at, last_success_at, last_error, next_due_at, failure_count
                 FROM source_ref_refresh_state
                 WHERE source = ?1 AND source_id = ?2",
                params![source, source_id],
                |row| state_from_row(row),
            )
            .optional()
            .context("get source_ref_refresh_state")
    }

    pub fn due_source_ref_refresh_states(
        &self,
        now: i64,
        limit: u32,
    ) -> Result<Vec<SourceRefRefreshState>> {
        let mut stmt = self
            .conn()
            .prepare(
                "SELECT source, source_id, last_attempt_at, last_success_at, last_error, next_due_at, failure_count
                 FROM source_ref_refresh_state
                 WHERE next_due_at IS NULL OR next_due_at <= ?1
                 ORDER BY COALESCE(next_due_at, 0), source, source_id
                 LIMIT ?2",
            )
            .context("prepare due source_ref_refresh_state query")?;
        let rows = stmt
            .query_map(params![now, limit], |row| state_from_row(row))
            .context("query due source_ref_refresh_state")?;
        rows.collect::<rusqlite::Result<Vec<_>>>()
            .context("collect due source_ref_refresh_state")
    }
}

fn state_from_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<SourceRefRefreshState> {
    Ok(SourceRefRefreshState {
        source: row.get(0)?,
        source_id: row.get(1)?,
        last_attempt_at: row.get(2)?,
        last_success_at: row.get(3)?,
        last_error: row.get(4)?,
        next_due_at: row.get(5)?,
        failure_count: row.get(6)?,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ids::{CanonicalId, ReleaseKind};

    fn db_with_ref() -> Db {
        let mut db = Db::open_in_memory().unwrap();
        let id = CanonicalId::new(ReleaseKind::Anime, "frieren").unwrap();
        db.upsert_canonical(&id, ReleaseKind::Anime, "Frieren", 1_000)
            .unwrap();
        db.attach_source_ref(&id, "anilist", "154587", Some("Frieren"), 1.0)
            .unwrap();
        db
    }

    #[test]
    fn refresh_state_round_trips() {
        let db = db_with_ref();
        let state = SourceRefRefreshState {
            source: "anilist".into(),
            source_id: "154587".into(),
            last_attempt_at: Some(1_000),
            last_success_at: Some(1_000),
            last_error: None,
            next_due_at: Some(2_000),
            failure_count: 0,
        };
        db.upsert_source_ref_refresh_state(&state).unwrap();
        assert_eq!(
            db.get_source_ref_refresh_state("anilist", "154587")
                .unwrap(),
            Some(state)
        );
    }

    #[test]
    fn due_refresh_states_are_limited_and_ordered() {
        let db = db_with_ref();
        db.upsert_source_ref_refresh_state(&SourceRefRefreshState {
            source: "anilist".into(),
            source_id: "154587".into(),
            last_attempt_at: None,
            last_success_at: None,
            last_error: None,
            next_due_at: Some(900),
            failure_count: 0,
        })
        .unwrap();
        let due = db.due_source_ref_refresh_states(1_000, 10).unwrap();
        assert_eq!(due.len(), 1);
        assert_eq!(due[0].source_id, "154587");
    }
}
