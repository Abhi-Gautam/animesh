//! Durable storage.
//!
//! Knows nothing about AniList and nothing about macOS. Repositories take a
//! transaction handle and never open a nested one, so the Library alone decides
//! what is atomic with what.

pub mod connection;
pub mod graph;
pub mod migrations;
pub mod read_models;
pub mod releases;
