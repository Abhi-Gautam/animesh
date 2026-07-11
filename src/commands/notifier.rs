//! `notifier` — the always-on daemon loop: fire a notification when a watchlist
//! entry's next episode airs. Body of `animesh daemon`.
//!
//! Two triggers, distinct jobs:
//! - **timer**  → the earliest un-sent airing is due → dispatch + record.
//! - **changed** → a watchlist row was inserted/updated → recompute the next
//!   airing. The signal is an in-process [`Notify`]: because the daemon owns the
//!   DB, whoever writes the watchlist (a command handler in this same process)
//!   pokes it directly — no CDC / polling.
//!
//! Timestamps are unix seconds UTC throughout (see [`crate::db::notifications`]).

use std::io::Write;
use std::sync::Arc;
use std::time::Duration;

use anyhow::{Context, Result};
use tokio::sync::Notify;
use turso::Connection;

use crate::db::notifications::{self, Due};

pub(crate) async fn run(conn: &Connection, changed: Arc<Notify>) -> Result<()> {
    loop {
        match notifications::next_due(conn).await? {
            Some(due) => wait_and_fire(conn, due, &changed).await?,
            None => changed.notified().await,
        }
    }
}

async fn wait_and_fire(conn: &Connection, due: Due, changed: &Notify) -> Result<()> {
    let now = chrono::Utc::now().timestamp();
    let wait = due.airing_at.saturating_sub(now).max(0) as u64;

    tokio::select! {
        _ = tokio::time::sleep(Duration::from_secs(wait)) => {
            dispatch(&due).context("dispatch notification")?;
            notifications::mark_sent(conn, due.anilist_id, due.episode, due.airing_at)
                .await
                .context("record sent notification")?;
        }
        _ = changed.notified() => {}
    }

    Ok(())
}

fn dispatch(due: &Due) -> Result<()> {
    let at = chrono::DateTime::from_timestamp(due.airing_at, 0)
        .map(|d| d.format("%Y-%m-%d %H:%M:%SZ").to_string())
        .unwrap_or_else(|| due.airing_at.to_string());
    println!(
        "🔔 NOTIFY  anilist_id={}  ep.{}  aired_at={}",
        due.anilist_id, due.episode, at
    );
    std::io::stdout().flush().ok();
    Ok(())
}
