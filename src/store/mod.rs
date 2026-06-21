//! The durable local library + ephemeral metadata cache.
//!
//! This is the only module that imports `rusqlite`. Everything else in
//! the crate goes through the typed API below. Driver swap (to `turso`
//! once it ships 1.0) is a single-module rewrite — see
//! `docs/superpowers/specs/2026-06-06-sp1-local-library-design.md`.

pub(crate) mod canonical_release;
pub(crate) mod canonical_schedule_event;
pub(crate) mod db;
pub(crate) mod engagement;
mod id_sql;
pub(crate) mod kv;
pub(crate) mod metadata_cache;
pub(crate) mod path;
pub(crate) mod raw_source_payload;
pub(crate) mod resolved_release;
pub(crate) mod source_candidate;
pub(crate) mod source_observation;
pub(crate) mod source_parse_error;
pub(crate) mod source_ref;
pub(crate) mod source_ref_refresh_state;
pub(crate) mod source_search_cache;

pub(crate) use canonical_release::{CanonicalFollowOutcome, CanonicalRelease};
pub(crate) use canonical_schedule_event::CanonicalScheduleEvent;
pub(crate) use db::Db;
pub(crate) use engagement::{Engagement, EngagementEvent, EngagementMeta, EngagementSource};
pub(crate) use metadata_cache::CacheEntry;
pub(crate) use path::resolve_db_path;
pub(crate) use resolved_release::{CanonicalScheduleEventSummary, ResolvedRelease};
pub(crate) use source_parse_error::SourceParseError;
pub(crate) use source_ref::SourceRef;
pub(crate) use source_ref_refresh_state::SourceRefRefreshState;
pub(crate) use source_search_cache::SourceSearchCacheEntry;
