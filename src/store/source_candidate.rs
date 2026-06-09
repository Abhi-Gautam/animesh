use anyhow::{Context, Result};
use rusqlite::params;

use crate::ids::ReleaseKind;
use crate::search::source_candidate::SourceCandidateResult;

use super::Db;

#[derive(Debug, Clone, PartialEq)]
pub struct SourceCandidate {
    pub source: String,
    pub source_id: String,
    pub kind: ReleaseKind,
    pub display_title: String,
    pub search_text: String,
    pub first_seen_at: i64,
    pub last_seen_at: i64,
    pub expires_at: Option<i64>,
    pub score_hint: Option<f64>,
}

impl Db {
    #[allow(dead_code)]
    pub fn upsert_source_candidate(&self, c: &SourceCandidate) -> Result<()> {
        self.conn()
            .execute(
                "INSERT INTO source_candidate (
                    source, source_id, kind, display_title, search_text,
                    first_seen_at, last_seen_at, expires_at, score_hint
                 ) VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9)
                 ON CONFLICT(source, source_id) DO UPDATE SET
                    kind = excluded.kind,
                    display_title = excluded.display_title,
                    search_text = excluded.search_text,
                    last_seen_at = excluded.last_seen_at,
                    expires_at = excluded.expires_at,
                    score_hint = excluded.score_hint",
                params![
                    c.source,
                    c.source_id,
                    c.kind.as_str(),
                    c.display_title,
                    c.search_text,
                    c.first_seen_at,
                    c.last_seen_at,
                    c.expires_at,
                    c.score_hint,
                ],
            )
            .context("upsert source_candidate")?;
        Ok(())
    }

    pub fn search_source_candidates(
        &self,
        query: &str,
        limit: u32,
    ) -> Result<Vec<SourceCandidateResult>> {
        let escaped = fts_query(query);
        let sql = "SELECT sc.source, sc.source_id, sc.kind, sc.display_title, sc.search_text,
                          bm25(source_candidate_fts) AS rank
                   FROM source_candidate_fts
                   JOIN source_candidate sc
                     ON sc.source = source_candidate_fts.source
                    AND sc.source_id = source_candidate_fts.source_id
                   WHERE source_candidate_fts MATCH ?1
                   ORDER BY rank
                   LIMIT ?2";
        let mut stmt = self
            .conn()
            .prepare(sql)
            .context("prepare source candidate search")?;
        let rows = stmt
            .query_map(params![escaped, limit], |row| {
                let kind_s: String = row.get(2)?;
                let kind = kind_s.parse::<ReleaseKind>().map_err(|e| {
                    rusqlite::Error::FromSqlConversionFailure(
                        2,
                        rusqlite::types::Type::Text,
                        Box::new(std::io::Error::new(
                            std::io::ErrorKind::InvalidData,
                            e.to_string(),
                        )),
                    )
                })?;
                Ok(SourceCandidateResult {
                    source: row.get(0)?,
                    source_id: row.get(1)?,
                    kind,
                    display_title: row.get(3)?,
                    search_text: row.get(4)?,
                    rank: row.get(5)?,
                })
            })
            .context("query source candidate search")?;
        rows.collect::<rusqlite::Result<Vec<_>>>()
            .context("collect source candidate search")
    }
}

fn fts_query(query: &str) -> String {
    query
        .split_whitespace()
        .map(|term| format!("{}*", term.replace('"', "")))
        .collect::<Vec<_>>()
        .join(" ")
}
