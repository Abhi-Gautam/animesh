//! Watchlist table repository.
//!
//! [`Entry`] is composition of domain models — not a parallel field dump:
//! - [`SearchHit`] identity
//! - [`NextAiring`] schedule (optional)

use anyhow::{Context, Result};
use turso::Connection;

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
pub(crate) async fn upsert(conn: &Connection, entry: &Entry) -> Result<bool> {
    let now = chrono::Utc::now().to_rfc3339();
    let hit = &entry.hit;

    let (next_episode, next_airing_at) = match entry.next {
        Some(n) => (Some(n.episode), Some(n.airing_at)),
        None => (None, None),
    };

    let mut rows = conn
        .query(
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
                updated_at     = excluded.updated_at
             RETURNING added_at = updated_at AS inserted",
            turso::params![
                hit.id,
                hit.title.as_str(),
                hit.title_english.clone(),
                hit.title_romaji.clone(),
                hit.title_native.clone(),
                hit.status.clone(),
                hit.format.clone(),
                hit.episodes,
                hit.season_year,
                next_episode,
                next_airing_at,
                now.as_str(),
                now.as_str(),
            ],
        )
        .await
        .context("upsert watchlist row")?;

    let row = rows
        .next()
        .await
        .context("read upsert result row")?
        .context("upsert returned no row")?;
    let inserted = *row.get_value(0)?.as_integer().context("inserted flag")?;
    Ok(inserted != 0)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db;
    use crate::models::{NextAiring, SearchHit};
    use tempfile::TempDir;
    use turso::Value;

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
        let dir = TempDir::new().expect("tempdir");
        let conn = db::open_path(&dir.path().join("animesh.db"))
            .await
            .expect("open");
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
    }

    #[tokio::test]
    async fn upsert_allows_null_next_airing() {
        let dir = TempDir::new().expect("tempdir");
        let conn = db::open_path(&dir.path().join("animesh.db"))
            .await
            .expect("open");
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
    }
}
