//! External-source adapters.
//!
//! Port/adapter boundary:
//! - callers see [`SourceAdapter`] + [`SourceRegistry`]
//! - each source module owns its HTTP client, request shaping, raw payload
//!   construction, and parser
//! - `reqwest::` remains confined to source modules

use std::future::Future;
use std::pin::Pin;

use anyhow::Result;

use crate::ingest::{RawSourcePayload, SourceParser};

pub mod anilist;
pub mod itunes;
pub mod jikan;
pub mod kitsu;
pub mod musicbrainz;
pub mod tvmaze;

pub type SourceFuture<'a, T> = Pin<Box<dyn Future<Output = Result<T>> + Send + 'a>>;

/// Source port used by ingestion/search orchestration. A source exposes only:
/// 1. search: query remote source when the user explicitly asks online search
/// 2. ingest: fetch source-owned information for a selected/followed source id
pub trait SourceAdapter: Send + Sync {
    fn source(&self) -> &'static str;

    fn parser(&self) -> &dyn SourceParser;

    fn search<'a>(
        &'a self,
        query: &'a str,
        limit: u32,
        now: i64,
    ) -> SourceFuture<'a, Vec<RawSourcePayload>>;

    #[allow(dead_code)]
    fn ingest<'a>(
        &'a self,
        source_id: &'a str,
        now: i64,
    ) -> SourceFuture<'a, Option<RawSourcePayload>>;
}

pub struct SourceRegistry {
    adapters: Vec<Box<dyn SourceAdapter>>,
}

impl SourceRegistry {
    pub fn new(adapters: Vec<Box<dyn SourceAdapter>>) -> Self {
        Self { adapters }
    }

    #[allow(dead_code)]
    pub fn empty() -> Self {
        Self {
            adapters: Vec::new(),
        }
    }

    pub fn production() -> Self {
        Self::new(vec![Box::new(anilist::AniListSource::new())])
    }

    pub fn adapters(&self) -> &[Box<dyn SourceAdapter>] {
        &self.adapters
    }
}

pub(crate) fn stable_hash(input: &str) -> String {
    // FNV-1a 64-bit. Deterministic identity for request/response payloads
    // without adding a crypto dependency.
    let mut hash = 0xcbf29ce484222325_u64;
    for byte in input.as_bytes() {
        hash ^= u64::from(*byte);
        hash = hash.wrapping_mul(0x100000001b3);
    }
    format!("{hash:016x}")
}
