//! The durable local library + ephemeral metadata cache.
//!
//! This is the only module that imports `rusqlite`. Everything else in
//! the crate goes through the typed API below. Driver swap (to `turso`
//! once it ships 1.0) is a single-module rewrite — see
//! `docs/superpowers/specs/2026-06-06-sp1-local-library-design.md`.

pub mod canonical_release;
pub mod canonical_schedule_event;
pub mod db;
pub mod engagement;
pub mod kv;
pub mod metadata_cache;
pub mod path;
pub mod raw_source_payload;
pub mod resolved_release;
pub mod source_candidate;
pub mod source_observation;
pub mod source_parse_error;
pub mod source_ref;
pub mod source_ref_refresh_state;
pub mod source_search_cache;

pub use canonical_release::{CanonicalFollowOutcome, CanonicalRelease};
pub use canonical_schedule_event::CanonicalScheduleEvent;
pub use db::Db;
pub use engagement::{Engagement, EngagementEvent, EngagementMeta, EngagementSource};
pub use metadata_cache::CacheEntry;
pub use path::resolve_db_path;
pub use resolved_release::ResolvedRelease;
pub use source_parse_error::SourceParseError;
pub use source_ref::SourceRef;
pub use source_ref_refresh_state::SourceRefRefreshState;
pub use source_search_cache::SourceSearchCacheEntry;
