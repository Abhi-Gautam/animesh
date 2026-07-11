//! Wire contract shared by the CLI client and the daemon server.
//!
//! This is the DTO layer: the only types that cross the socket. Domain models
//! (`models.rs`) are reused as payloads where their shape already matches the
//! contract; DB row types (`db/`) never appear here.

use serde::{Deserialize, Serialize};

use crate::models::WatchlistMutation;

/// A command the client asks the daemon to run. Only DB-touching commands go
/// over the wire — `search`/`schedule` are pure AniList and stay in the client.
#[derive(Debug, Serialize, Deserialize)]
pub(crate) enum Request {
    Watchlist {
        id: i64,
    },
    DevAiring {
        id: i64,
        episode: i64,
        secs_from_now: i64,
    },
}

/// The daemon's response.
#[derive(Debug, Serialize, Deserialize)]
pub(crate) enum Reply {
    Watchlist(WatchlistMutation),
    DevAiring { airing_at: i64 },
    Error(String),
}
