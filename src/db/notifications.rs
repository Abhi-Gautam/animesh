//! Notifications table repository — the notifier daemon's ledger.
//!
//! Two operations back the daemon loop:
//! - [`next_due`]   — the earliest un-sent airing across the watchlist.
//! - [`mark_sent`]  — record that an airing's notification fired (dedup key).
//!
//! Timestamps are unix seconds UTC (see [`migrations/003_notifications.sql`]).

use anyhow::{Context, Result};
use turso::Connection;

/// The next airing the daemon should notify for: a watchlist row whose
/// `next_episode` airing has not yet been sent.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct Due {
    pub anilist_id: i64,
    pub episode: i64,
    pub airing_at: i64,
}

/// Earliest watchlist airing that has no `sent` notification yet, or `None` if
/// nothing is scheduled. This is the daemon's "next time to notify".
///
/// The `NOT EXISTS` is the dedup guard: once [`mark_sent`] records `(id, ep)`,
/// the same airing stops being returned even though the watchlist row still
/// points at it (until the next sync advances `next_episode`).
pub(crate) async fn next_due(conn: &Connection) -> Result<Option<Due>> {
    let mut rows = conn
        .query(
            "SELECT w.anilist_id, w.next_episode, w.next_airing_at
             FROM watchlist w
             WHERE w.next_airing_at IS NOT NULL
               AND w.next_episode  IS NOT NULL
               AND NOT EXISTS (
                   SELECT 1 FROM notifications n
                   WHERE n.anilist_id = w.anilist_id
                     AND n.episode    = w.next_episode
                     AND n.sent = 1
               )
             ORDER BY w.next_airing_at ASC
             LIMIT 1",
            (),
        )
        .await
        .context("query next due notification")?;

    let Some(row) = rows.next().await.context("read next due row")? else {
        return Ok(None);
    };

    let anilist_id = *row.get_value(0)?.as_integer().context("anilist_id")?;
    let episode = *row.get_value(1)?.as_integer().context("next_episode")?;
    let airing_at = *row.get_value(2)?.as_integer().context("next_airing_at")?;

    Ok(Some(Due {
        anilist_id,
        episode,
        airing_at,
    }))
}

/// Record that the notification for `(anilist_id, episode)` fired. Idempotent:
/// re-firing the same airing is a no-op update, and the `PRIMARY KEY` makes the
/// dedup lookup in [`next_due`] a point read.
pub(crate) async fn mark_sent(
    conn: &Connection,
    anilist_id: i64,
    episode: i64,
    airing_at: i64,
) -> Result<()> {
    let now = chrono::Utc::now().timestamp();

    conn.execute(
        "INSERT INTO notifications (anilist_id, episode, airing_at, sent, notified_at)
         VALUES (?1, ?2, ?3, 1, ?4)
         ON CONFLICT(anilist_id, episode) DO UPDATE SET
             airing_at   = excluded.airing_at,
             sent        = 1,
             notified_at = excluded.notified_at",
        turso::params![anilist_id, episode, airing_at, now],
    )
    .await
    .context("mark notification sent")?;

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::{self, watchlist};
    use crate::models::{NextAiring, SearchHit};
    use tempfile::TempDir;

    fn entry(id: i64, episode: i64, airing_at: i64) -> watchlist::Entry {
        watchlist::Entry {
            hit: SearchHit {
                id,
                title: format!("anime {id}"),
                title_english: None,
                title_romaji: None,
                title_native: None,
                status: Some("RELEASING".into()),
                format: Some("TV".into()),
                episodes: None,
                season_year: Some(2026),
            },
            next: Some(NextAiring {
                episode,
                airing_at,
                time_until_airing: None,
            }),
        }
    }

    #[tokio::test]
    async fn next_due_returns_earliest_unsent() {
        let dir = TempDir::new().unwrap();
        let conn = db::open_path(&dir.path().join("animesh.db")).await.unwrap();

        watchlist::upsert(&conn, &entry(1, 5, 2_000)).await.unwrap();
        watchlist::upsert(&conn, &entry(2, 3, 1_000)).await.unwrap();

        let due = next_due(&conn).await.unwrap().expect("a due airing");
        assert_eq!(
            due,
            Due {
                anilist_id: 2,
                episode: 3,
                airing_at: 1_000
            }
        );
    }

    #[tokio::test]
    async fn mark_sent_excludes_from_next_due() {
        let dir = TempDir::new().unwrap();
        let conn = db::open_path(&dir.path().join("animesh.db")).await.unwrap();

        watchlist::upsert(&conn, &entry(1, 5, 1_000)).await.unwrap();
        assert!(next_due(&conn).await.unwrap().is_some());

        mark_sent(&conn, 1, 5, 1_000).await.unwrap();
        assert!(next_due(&conn).await.unwrap().is_none());
    }
}
