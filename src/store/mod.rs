//! The durable local library + ephemeral metadata cache.
//!
//! This is the only module that imports `rusqlite`. Everything else in
//! the crate goes through the typed API below. Driver swap (to `turso`
//! once it ships 1.0) is a single-module rewrite — see
//! `docs/superpowers/specs/2026-06-06-sp1-local-library-design.md`.

pub mod db;
pub mod path;
pub mod tracked_item;

pub use db::{Db, MAX_KNOWN_VERSION};
pub use path::resolve_db_path;
pub use tracked_item::{FollowOutcome, ListFilter, TrackedItem};
