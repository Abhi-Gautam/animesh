//! The durable local library + ephemeral metadata cache.
//!
//! This is the only module that imports `rusqlite`. Everything else in
//! the crate goes through the typed API below. Driver swap (to `turso`
//! once it ships 1.0) is a single-module rewrite — see
//! `docs/superpowers/specs/2026-06-06-sp1-local-library-design.md`.

// Submodules land in subsequent tasks (T11 path, T12 migrations, T13
// tracked_item, T14 metadata_cache, T15 search_fts). Keeping the module
// declared here so cargo check sees the scaffold.
