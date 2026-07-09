//! Watchlist table repository.
//!
//! [`Entry`] is composition of domain models — not a parallel field dump:
//! - [`SearchHit`] identity
//! - [`NextAiring`] schedule (optional)

use anyhow::{Context, Result};
use turso::{Connection, Value};

use crate::models::{AnimeDetail, NextAiring, SearchHit};

/// Persistable watchlist row = search identity + optional next airing.
#[derive(Debug, Clone)]
pub(crate) struct Entry {
    pub hit: SearchHit,
    pub next: Option<NextAiring>,
}

impl From<AnimeDetail> for Entry {
    fn from(detail: AnimeDetail) -> Self {
        Self {
            hit: detail.hit,
            next: detail.next,
        }
    }
}

impl From<&AnimeDetail> for Entry {
    fn from(detail: &AnimeDetail) -> Self {
        Self {
            hit: detail.hit.clone(),
            next: detail.next,
        }
    }
}

/// Insert or refresh a watchlist row.
///
/// Returns `true` when this was a new insert, `false` when an existing row
/// was updated.
pub(crate) async fn upsert(conn: &Connection, entry: &Entry) -> Result<bool> {
    let now = chrono::Utc::now().to_rfc3339();
    let hit = &entry.hit;
    let existed = exists(conn, hit.id).await?;

    let (next_episode, next_airing_at) = match entry.next {
        Some(n) => (Some(n.episode), Some(n.airing_at)),
        None => (None, None),
    };

    conn.execute(
        "INSERT INTO watchlist (
            anilist_id, title, title_english, title_romaji, title_native,
            status, format, episodes, season_year,
            next_episode, next_airing_at,
            added_at, updated_at
         ) VALUES (
            ?1, ?2, ?3, ?4, ?5,
            ?6, ?7, ?8, ?9,
            ?10, ?11,
            ?12, ?13
         )
         ON CONFLICT(anilist_id) DO UPDATE SET
            title          = excluded.title,
            title_english  = excluded.title_english,
            title_romaji   = excluded.title_romaji,
            title_native   = excluded.title_native,
            status         = excluded.status,
            format         = excluded.format,
            episodes       = excluded.episodes,
            season_year    = excluded.season_year,
            next_episode   = excluded.next_episode,
            next_airing_at = excluded.next_airing_at,
            updated_at     = excluded.updated_at",
        turso::params![
            hit.id,
            hit.title.as_str(),
            opt_str(&hit.title_english),
            opt_str(&hit.title_romaji),
            opt_str(&hit.title_native),
            opt_str(&hit.status),
            opt_str(&hit.format),
            opt_i64(hit.episodes),
            opt_i64(hit.season_year),
            opt_i64(next_episode),
            opt_i64(next_airing_at),
            now.as_str(),
            now.as_str(),
        ],
    )
    .await
    .context("upsert watchlist row")?;

    Ok(!existed)
}

async fn exists(conn: &Connection, anilist_id: i64) -> Result<bool> {
    let mut rows = conn
        .query(
            "SELECT 1 FROM watchlist WHERE anilist_id = ?1 LIMIT 1",
            turso::params![anilist_id],
        )
        .await
        .context("query watchlist existence")?;
    Ok(rows.next().await.context("read existence row")?.is_some())
}

fn opt_str(v: &Option<String>) -> Value {
    match v {
        Some(s) => Value::Text(s.clone()),
        None => Value::Null,
    }
}

fn opt_i64(v: Option<i64>) -> Value {
    match v {
        Some(n) => Value::Integer(n),
        None => Value::Null,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db;
    use crate::models::{NextAiring, SearchHit};
    use std::path::PathBuf;
    use std::time::{SystemTime, UNIX_EPOCH};

    fn temp_db_path() -> PathBuf {
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        std::env::temp_dir().join(format!("animesh-watchlist-test-{nanos}.db"))
    }

    fn sample_hit(id: i64, title: &str) -> SearchHit {
        SearchHit {
            id,
            title: title.into(),
            title_english: Some(title.into()),
            title_romaji: Some(title.into()),
            title_native: None,
            status: Some("RELEASING".into()),
            format: Some("TV".into()),
            episodes: None,
            season_year: Some(1999),
        }
    }

    #[tokio::test]
    async fn upsert_stores_next_airing() {
        let path = temp_db_path();
        let _ = std::fs::remove_file(&path);

        let conn = db::open_path(&path).await.expect("open");
        let entry = Entry {
            hit: sample_hit(21, "One Piece"),
            next: Some(NextAiring {
                episode: 1169,
                airing_at: 1_783_865_760,
                time_until_airing: None,
            }),
        };

        assert!(upsert(&conn, &entry).await.expect("insert"));
        assert!(!upsert(&conn, &entry).await.expect("refresh"));

        let mut rows = conn
            .query(
                "SELECT title, next_episode, next_airing_at FROM watchlist WHERE anilist_id = 21",
                (),
            )
            .await
            .unwrap();
        let row = rows.next().await.unwrap().expect("row");
        assert_eq!(row.get_value(0).unwrap().as_text().unwrap(), "One Piece");
        assert_eq!(*row.get_value(1).unwrap().as_integer().unwrap(), 1169);
        assert_eq!(
            *row.get_value(2).unwrap().as_integer().unwrap(),
            1_783_865_760
        );

        let _ = std::fs::remove_file(&path);
    }

    #[tokio::test]
    async fn upsert_allows_null_next_airing() {
        let path = temp_db_path();
        let _ = std::fs::remove_file(&path);

        let conn = db::open_path(&path).await.expect("open");
        let entry = Entry {
            hit: SearchHit {
                id: 11061,
                title: "Hunter x Hunter (2011)".into(),
                title_english: Some("Hunter x Hunter (2011)".into()),
                title_romaji: None,
                title_native: None,
                status: Some("FINISHED".into()),
                format: Some("TV".into()),
                episodes: Some(148),
                season_year: Some(2011),
            },
            next: None,
        };

        assert!(upsert(&conn, &entry).await.expect("insert"));

        let mut rows = conn
            .query(
                "SELECT next_episode, next_airing_at FROM watchlist WHERE anilist_id = 11061",
                (),
            )
            .await
            .unwrap();
        let row = rows.next().await.unwrap().expect("row");
        assert!(matches!(row.get_value(0).unwrap(), Value::Null));
        assert!(matches!(row.get_value(1).unwrap(), Value::Null));

        let _ = std::fs::remove_file(&path);
    }
}
