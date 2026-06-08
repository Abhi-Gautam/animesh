//! Background sync — metadata refresh + link-delta verification.
//!
//! Two responsibilities, sharply separated:
//!
//!   * **Refresh** (planned for task #15): TTL-based pull of source
//!     metadata, written to `metadata_cache`. Runs on a cadence.
//!   * **Verify** ([`verify::detect_new_streaming`]): pure function
//!     that diffs the previous metadata snapshot against the new one,
//!     emitting a [`VerifiedRelease`] for every newly-appearing
//!     streaming link that matches the user's subscriptions.
//!
//! The verify step is what makes "we will trust the link they
//! provide" the v0.5 moat: when an AniList or TMDB refresh adds a
//! streaming URL for a subscribed streamer that wasn't there before,
//! that's the moment the title becomes playable. No HTML, no probing,
//! no heuristics — just diff and notify.

pub mod engine;
pub mod verify;

pub use engine::{SyncEngine, SyncReport};
pub use verify::{detect_new_streaming, VerifiedRelease};
