//! Domain object models returned by command/services.
//!
//! Commands return these; they do not print.
//!
//! Composition:
//! - [`SearchHit`] — identity / discovery fields
//! - [`NextAiring`] — schedule fields
//! - [`AnimeDetail`] — `SearchHit` + `NextAiring` (schedule view)
//! - watchlist [`crate::db::watchlist::Entry`] — same composition, persistence boundary

use crate::sources::anilist::{Media, NextAiringEpisode};

/// One search hit (discovery / identity fields).
#[derive(Debug, Clone)]
pub(crate) struct SearchHit {
    pub id: i64,
    pub title: String,
    pub title_english: Option<String>,
    pub title_romaji: Option<String>,
    pub title_native: Option<String>,
    pub status: Option<String>,
    pub format: Option<String>,
    pub episodes: Option<i64>,
    pub season_year: Option<i64>,
}

impl From<Media> for SearchHit {
    fn from(m: Media) -> Self {
        Self {
            id: m.id,
            title: m.display_title().to_string(),
            title_english: m.title.english,
            title_romaji: m.title.romaji,
            title_native: m.title.native,
            status: m.status,
            format: m.format,
            episodes: m.episodes,
            season_year: m.season_year,
        }
    }
}

/// Next scheduled episode (from AniList `nextAiringEpisode`).
#[derive(Debug, Clone, Copy)]
pub(crate) struct NextAiring {
    pub episode: i64,
    /// Unix seconds UTC.
    pub airing_at: i64,
    pub time_until_airing: Option<i64>,
}

impl From<NextAiringEpisode> for NextAiring {
    fn from(n: NextAiringEpisode) -> Self {
        Self {
            episode: n.episode,
            airing_at: n.airing_at,
            time_until_airing: n.time_until_airing,
        }
    }
}

/// Schedule/detail view: identity ([`SearchHit`]) + optional next airing.
#[derive(Debug, Clone)]
pub(crate) struct AnimeDetail {
    pub hit: SearchHit,
    pub next: Option<NextAiring>,
}

impl From<Media> for AnimeDetail {
    fn from(m: Media) -> Self {
        let next = m.next_airing_episode.map(NextAiring::from);
        Self {
            hit: SearchHit::from(m),
            next,
        }
    }
}

/// Result of adding/updating a watchlist row.
#[derive(Debug, Clone)]
pub(crate) struct WatchlistMutation {
    /// `true` if newly inserted; `false` if an existing row was refreshed.
    pub inserted: bool,
    pub detail: AnimeDetail,
}
