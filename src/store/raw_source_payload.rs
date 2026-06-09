use anyhow::{Context, Result};
use rusqlite::params;

#[cfg(test)]
use crate::ingest::HttpMethod;
use crate::ingest::RawSourcePayload;

use super::Db;

impl Db {
    pub fn upsert_raw_source_payload(&self, payload: &RawSourcePayload) -> Result<()> {
        self.conn()
            .execute(
                "INSERT INTO raw_source_payload (
                    id, source, endpoint, method, request_key, request_hash, request_json,
                    http_status, response_hash, response_json, fetched_at, expires_at, created_at
                 ) VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11,?12,?13)
                 ON CONFLICT(source, request_hash, response_hash) DO NOTHING",
                params![
                    payload.id,
                    payload.source,
                    payload.endpoint,
                    payload.method.as_str(),
                    payload.request_key,
                    payload.request_hash,
                    payload.request_json,
                    payload.http_status,
                    payload.response_hash,
                    payload.response_json,
                    payload.fetched_at,
                    payload.expires_at,
                    payload.created_at,
                ],
            )
            .context("upsert raw_source_payload")?;
        Ok(())
    }

    #[allow(dead_code)]
    #[cfg(test)]
    pub fn get_raw_source_payload(&self, id: &str) -> Result<Option<RawSourcePayload>> {
        use rusqlite::OptionalExtension;
        self.conn()
            .query_row(
                "SELECT id, source, endpoint, method, request_key, request_hash, request_json,
                        http_status, response_hash, response_json, fetched_at, expires_at, created_at
                 FROM raw_source_payload WHERE id = ?1",
                params![id],
                |row| {
                    let method: String = row.get(3)?;
                    Ok(RawSourcePayload {
                        id: row.get(0)?,
                        source: row.get(1)?,
                        endpoint: row.get(2)?,
                        method: if method == "POST" { HttpMethod::Post } else { HttpMethod::Get },
                        request_key: row.get(4)?,
                        request_hash: row.get(5)?,
                        request_json: row.get(6)?,
                        http_status: row.get(7)?,
                        response_hash: row.get(8)?,
                        response_json: row.get(9)?,
                        fetched_at: row.get(10)?,
                        expires_at: row.get(11)?,
                        created_at: row.get(12)?,
                    })
                },
            )
            .optional()
            .context("get raw_source_payload")
    }
}
