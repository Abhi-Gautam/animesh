use anyhow::{Context, Result};
use rusqlite::{params, OptionalExtension};

use super::Db;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SourceSearchCacheEntry {
    pub source: String,
    pub query_key: String,
    pub last_success_at: Option<i64>,
    pub next_due_at: Option<i64>,
}

impl Db {
    pub fn upsert_source_search_cache(&self, entry: &SourceSearchCacheEntry) -> Result<()> {
        self.conn()
            .execute(
                "INSERT INTO source_search_cache
                    (source, query_key, last_success_at, next_due_at)
                 VALUES (?1,?2,?3,?4)
                 ON CONFLICT(source, query_key) DO UPDATE SET
                    last_success_at = excluded.last_success_at,
                    next_due_at = excluded.next_due_at",
                params![
                    entry.source,
                    entry.query_key,
                    entry.last_success_at,
                    entry.next_due_at,
                ],
            )
            .context("upsert source_search_cache")?;
        Ok(())
    }

    pub fn get_source_search_cache(
        &self,
        source: &str,
        query_key: &str,
    ) -> Result<Option<SourceSearchCacheEntry>> {
        self.conn()
            .query_row(
                "SELECT source, query_key, last_success_at, next_due_at
                 FROM source_search_cache
                 WHERE source = ?1 AND query_key = ?2",
                params![source, query_key],
                |row| {
                    Ok(SourceSearchCacheEntry {
                        source: row.get(0)?,
                        query_key: row.get(1)?,
                        last_success_at: row.get(2)?,
                        next_due_at: row.get(3)?,
                    })
                },
            )
            .optional()
            .context("get source_search_cache")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn search_cache_round_trips() {
        let db = Db::open_in_memory().unwrap();
        let entry = SourceSearchCacheEntry {
            source: "anilist".into(),
            query_key: "frieren".into(),
            last_success_at: Some(1_000),
            next_due_at: Some(87_400),
        };
        db.upsert_source_search_cache(&entry).unwrap();
        assert_eq!(
            db.get_source_search_cache("anilist", "frieren").unwrap(),
            Some(entry)
        );
    }
}
