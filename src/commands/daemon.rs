//! `daemon` — the always-on process. It is the sole owner of the DB and runs
//! two things concurrently in one process:
//!
//! - the **notifier** loop (fires notifications when episodes air), and
//! - a **socket listener** that serves CLI clients (`animesh watchlist …`).
//!
//! Because both live here, a client's write and the `notify_one()` that wakes
//! the notifier happen in the same process on the same [`Notify`] — no CDC.

use std::sync::Arc;

use anyhow::{Context, Result};
use tokio::sync::Notify;
use turso::Database;

use crate::api::{Reply, Request};
use crate::commands::{dev, notifier, watchlist};
use crate::db;
use crate::ipc;
use crate::sources::AniListClient;

/// Open the DB, then run the notifier and the socket server until either exits.
pub(crate) async fn run() -> Result<()> {
    let database = db::open_database().await.context("open database")?;
    let notify = Arc::new(Notify::new());
    let client = AniListClient::new();

    let notifier_conn = database.connect().context("open notifier connection")?;
    let listener = ipc::bind()?;

    println!("animesh daemon started (Ctrl-C to stop)");

    tokio::select! {
        r = notifier::run(&notifier_conn, notify.clone()) => r,
        r = serve(&listener, &database, &client, &notify) => r,
    }
}

/// Accept connections one at a time and dispatch each. Sequential is fine for a
/// single-user tool — requests are quick and rare relative to the notifier.
async fn serve(
    listener: &ipc::Listener,
    database: &Database,
    client: &AniListClient,
    notify: &Notify,
) -> Result<()> {
    loop {
        let stream = ipc::accept(listener).await?;
        if let Err(e) = ipc::serve_once(stream, |req| handle(req, database, client, notify)).await {
            eprintln!("request failed: {e:#}");
        }
    }
}

/// Run one request against the DB and return its reply. Watchlist mutations poke
/// `notify` so the notifier recomputes the next airing.
async fn handle(
    req: Request,
    database: &Database,
    client: &AniListClient,
    notify: &Notify,
) -> Reply {
    match req {
        Request::Watchlist { id } => {
            let conn = match database.connect() {
                Ok(c) => c,
                Err(e) => return Reply::Error(format!("connect: {e}")),
            };
            match watchlist::run(client, &conn, id).await {
                Ok(mutation) => {
                    notify.notify_one();
                    Reply::Watchlist(mutation)
                }
                Err(e) => Reply::Error(format!("{e:#}")),
            }
        }
        Request::DevAiring {
            id,
            episode,
            secs_from_now,
        } => {
            let conn = match database.connect() {
                Ok(c) => c,
                Err(e) => return Reply::Error(format!("connect: {e}")),
            };
            match dev::airing(&conn, id, episode, secs_from_now).await {
                Ok(airing_at) => {
                    notify.notify_one();
                    Reply::DevAiring { airing_at }
                }
                Err(e) => Reply::Error(format!("{e:#}")),
            }
        }
    }
}
