//! The durable local library + ephemeral metadata cache.
//!
//! This is the only module that imports `rusqlite`. Everything else in
//! the crate goes through the typed API below. Driver swap (to `turso`
//! once it ships 1.0) is a single-module rewrite — see
//! `docs/superpowers/specs/2026-06-06-sp1-local-library-design.md`.

pub mod db;
pub mod kv;
pub mod metadata_cache;
pub mod path;
pub mod search;
pub mod tracked_item;

pub use db::{Db, MAX_KNOWN_VERSION};
pub use metadata_cache::{CacheEntry, CacheStats, CacheStatus, TtlConfig};
pub use path::resolve_db_path;
pub use search::SearchHit;
pub use tracked_item::{FollowOutcome, ListFilter, TrackedItem};
