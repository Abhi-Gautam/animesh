//! The durable local library + ephemeral metadata cache.
//!
//! This is the only module that imports `rusqlite`. Everything else in
//! the crate goes through the typed API below. Driver swap (to `turso`
//! once it ships 1.0) is a single-module rewrite — see
//! `docs/superpowers/specs/2026-06-06-sp1-local-library-design.md`.

pub mod canonical_release;
pub mod db;
pub mod engagement;
pub mod kv;
pub mod metadata_cache;
pub mod path;
pub mod source_ref;

pub use canonical_release::{CanonicalFollowOutcome, CanonicalRelease};
pub use db::Db;
pub use engagement::{Engagement, EngagementEvent};
pub use metadata_cache::{CacheEntry, TtlConfig};
pub use path::resolve_db_path;
pub use source_ref::SourceRef;
