//! Local fuzzy search over the FTS5 index.
//!
//! The picker's hot path lives here. Sub-millisecond on libraries of
//! tens of thousands. Network only on commit, never on keystroke —
//! see spec §7.

use anyhow::{Context, Result};
use rusqlite::params;

use super::Db;

/// One match from `search_fuzzy`. Mirrors the FTS row contents minus
/// the relevance rank, which is implicit in result ordering.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SearchHit {
    pub source: String,
    pub source_id: String,
    pub display_title: Option<String>,
    pub title_english: Option<String>,
    pub title_native: Option<String>,
}

/// Strip FTS5 query-parser metacharacters and turn each remaining token
/// into a prefix match. `*` after each token gives the type-as-you-search
/// feel: "naru" → "naru*" → matches "Naruto", "Narutaki", etc.
///
/// Returns an empty string for input that resolves to no tokens; the
/// caller short-circuits to an empty result rather than running an
/// empty MATCH (which FTS5 rejects).
fn build_fts_query(input: &str) -> String {
    let cleaned: String = input
        .chars()
        .map(|c| match c {
            '*' | '(' | ')' | '"' | '^' | ':' | '-' | '+' | '~' | '&' | '|' | '\\' => ' ',
            _ => c,
        })
        .collect();
    cleaned
        .split_whitespace()
        .filter(|t| !t.is_empty())
        .map(|t| format!("{t}*"))
        .collect::<Vec<_>>()
        .join(" ")
}

impl Db {
    /// Fuzzy-search cached titles. Returns up to `limit` hits ranked
    /// by FTS5's BM25 (most relevant first). Empty input returns
    /// empty.
    pub fn search_fuzzy(&self, query: &str, limit: usize) -> Result<Vec<SearchHit>> {
        let fts_query = build_fts_query(query);
        if fts_query.is_empty() {
            return Ok(Vec::new());
        }
        let conn = self.conn();
        let mut stmt = conn
            .prepare(
                "SELECT source, source_id, display_title, title_english, title_native \
                 FROM search_fts \
                 WHERE search_fts MATCH ?1 \
                 ORDER BY bm25(search_fts) ASC \
                 LIMIT ?2",
            )
            .context("prepare search_fuzzy")?;
        let rows = stmt
            .query_map(params![fts_query, limit as i64], |row| {
                Ok(SearchHit {
                    source: row.get(0)?,
                    source_id: row.get(1)?,
                    display_title: row.get(2)?,
                    title_english: row.get(3)?,
                    title_native: row.get(4)?,
                })
            })
            .context("query search_fuzzy")?;
        rows.collect::<rusqlite::Result<Vec<_>>>()
            .context("collect search_fuzzy")
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::store::metadata_cache::CacheEntry;

    fn fresh() -> Db {
        Db::open_in_memory().unwrap()
    }

    fn seed(db: &Db, id: &str, display: &str, english: Option<&str>, native: Option<&str>) {
        db.upsert_cache(&CacheEntry {
            source: "anilist".into(),
            source_id: id.into(),
            display_title: Some(display.into()),
            title_english: english.map(|s| s.into()),
            title_native: native.map(|s| s.into()),
            status: Some("RELEASING".into()),
            total_episodes: None,
            format: Some("TV".into()),
            next_episode_number: None,
            next_episode_airs_at: None,
            fetched_at: 0,
            expires_at: i64::MAX,
        })
        .unwrap();
    }

    #[test]
    fn empty_query_returns_empty() {
        let db = fresh();
        seed(&db, "1", "Naruto", None, None);
        assert!(db.search_fuzzy("", 10).unwrap().is_empty());
        assert!(db.search_fuzzy("   ", 10).unwrap().is_empty());
    }

    #[test]
    fn prefix_match_finds_partial_token() {
        let db = fresh();
        seed(&db, "1", "Naruto", None, None);
        seed(&db, "2", "Bleach", None, None);
        let hits = db.search_fuzzy("naru", 10).unwrap();
        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0].source_id, "1");
    }

    #[test]
    fn multi_token_query_is_and() {
        let db = fresh();
        seed(&db, "1", "One Piece", None, None);
        seed(&db, "2", "One Punch Man", None, None);
        seed(&db, "3", "Piece of Cake", None, None);
        let hits = db.search_fuzzy("one piece", 10).unwrap();
        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0].source_id, "1");
    }

    #[test]
    fn case_insensitive() {
        let db = fresh();
        seed(&db, "1", "Naruto", None, None);
        assert_eq!(db.search_fuzzy("NaRu", 10).unwrap().len(), 1);
    }

    #[test]
    fn diacritics_folded() {
        // unicode61 with remove_diacritics 2 — "Cafe" should match "Café".
        let db = fresh();
        seed(&db, "1", "Café Romantique", None, None);
        let hits = db.search_fuzzy("cafe", 10).unwrap();
        assert_eq!(hits.len(), 1);
    }

    #[test]
    fn english_and_native_fields_are_searchable() {
        let db = fresh();
        seed(
            &db,
            "1",
            "Shingeki no Kyojin",
            Some("Attack on Titan"),
            Some("進撃の巨人"),
        );
        assert_eq!(db.search_fuzzy("attack", 10).unwrap().len(), 1);
        assert_eq!(db.search_fuzzy("shingeki", 10).unwrap().len(), 1);
        // Native (CJK) — unicode61 splits on script changes, so a
        // single ideograph search still resolves.
        assert_eq!(db.search_fuzzy("進撃", 10).unwrap().len(), 1);
    }

    #[test]
    fn limit_is_respected() {
        let db = fresh();
        for i in 0..5 {
            seed(&db, &i.to_string(), &format!("Show {i}"), None, None);
        }
        let hits = db.search_fuzzy("show", 3).unwrap();
        assert_eq!(hits.len(), 3);
    }

    #[test]
    fn punctuation_does_not_break_parser() {
        // These would all blow up an unsanitized FTS5 query.
        let db = fresh();
        seed(&db, "1", "Re:Zero", None, None);
        let inputs = ["re:zero", "re-zero", "re*zero", "re(zero", "re+zero"];
        for q in inputs {
            let hits = db
                .search_fuzzy(q, 10)
                .unwrap_or_else(|e| panic!("query {q:?} errored: {e}"));
            assert!(
                hits.iter().any(|h| h.source_id == "1"),
                "expected Re:Zero from query {q:?}, got {hits:?}"
            );
        }
    }

    #[test]
    fn build_fts_query_is_robust_against_garbage() {
        assert_eq!(build_fts_query(""), "");
        assert_eq!(build_fts_query("***"), "");
        assert_eq!(build_fts_query("a"), "a*");
        assert_eq!(build_fts_query("one piece"), "one* piece*");
        assert_eq!(build_fts_query("re:zero"), "re* zero*");
    }
}
