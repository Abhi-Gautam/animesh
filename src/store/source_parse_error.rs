use anyhow::{Context, Result};
use rusqlite::params;

use super::Db;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SourceParseError {
    pub raw_payload_id: String,
    pub source: String,
    pub endpoint: String,
    pub error: String,
    pub occurred_at: i64,
}

impl Db {
    #[allow(dead_code)]
    pub fn insert_source_parse_error(&self, err: &SourceParseError) -> Result<()> {
        self.conn()
            .execute(
                "INSERT INTO source_parse_error
                    (raw_payload_id, source, endpoint, error, occurred_at)
                 VALUES (?1,?2,?3,?4,?5)",
                params![
                    err.raw_payload_id,
                    err.source,
                    err.endpoint,
                    err.error,
                    err.occurred_at,
                ],
            )
            .context("insert source_parse_error")?;
        Ok(())
    }
}
