//! External-source adapters.
//!
//! One file per adapter. Each owns its own rate-limiting + retry and
//! exposes adapter-shaped types (no abstract `Source` trait today — the
//! TUI calls AniList directly via [`anilist::AniListClient`]). When a
//! second adapter lands, factor out a trait then.
//!
//! Layering rule: `reqwest::` only appears in adapter modules below.

pub mod anilist;
pub mod itunes;
pub mod jikan;
pub mod kitsu;
pub mod musicbrainz;
pub mod tvmaze;
