//! AniList source: transport, wire shapes, parsing, and decode classification.

pub mod client;
pub mod dto;
pub mod parser;
pub mod queries;

use std::collections::BTreeMap;

use client::FetchOutcome;
use parser::{BatchIntegrityError, DetailResult, ItemError, ItemResult};

use crate::domain::ids::AniListId;
use crate::domain::media::{MediaObservation, SearchCandidate};

/// Outcome of decoding a detail response body.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DetailDecode {
    Observed(Box<MediaObservation>),
    /// AniList answered `Media: null`. The ID does not exist; not retryable.
    NotFound,
    /// The body was not a GraphQL envelope at all.
    Decode(String),
    /// The envelope carried errors and no usable data.
    GraphQl(String),
    InvalidItem(ItemError),
    /// The response described a media other than the one requested.
    Integrity {
        requested: i64,
        returned: i64,
    },
}

impl DetailDecode {
    pub const fn outcome(&self) -> FetchOutcome {
        match self {
            Self::Observed(_) | Self::NotFound => FetchOutcome::Success,
            Self::Decode(_) => FetchOutcome::DecodeError,
            Self::GraphQl(_) => FetchOutcome::GraphQlError,
            Self::InvalidItem(_) => FetchOutcome::DecodeError,
            Self::Integrity { .. } => FetchOutcome::IntegrityError,
        }
    }
}

/// Outcome of decoding a batch response body.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum BatchDecode {
    Items(BTreeMap<AniListId, ItemResult>),
    Decode(String),
    GraphQl(String),
    Integrity(BatchIntegrityError),
}

impl BatchDecode {
    /// The stable code recorded against a pass that could not use this response.
    ///
    /// `None` for a usable page. Lives beside the enum so the outcome and the
    /// code it is filed under cannot drift apart.
    pub fn failure_code(&self) -> Option<String> {
        match self {
            Self::Items(_) => None,
            Self::Integrity(error) => Some(format!("integrity:{error}")),
            Self::Decode(_) => Some("decode".to_owned()),
            Self::GraphQl(_) => Some("graphql".to_owned()),
        }
    }

    pub const fn outcome(&self) -> FetchOutcome {
        match self {
            Self::Items(_) => FetchOutcome::Success,
            Self::Decode(_) => FetchOutcome::DecodeError,
            Self::GraphQl(_) => FetchOutcome::GraphQlError,
            Self::Integrity(_) => FetchOutcome::IntegrityError,
        }
    }
}

/// Outcome of decoding a search response body.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SearchDecode {
    Candidates(Vec<SearchCandidate>),
    Decode(String),
    GraphQl(String),
}

fn first_error(errors: &[dto::GraphQlError]) -> String {
    errors
        .first()
        .map(|e| e.message.clone())
        .unwrap_or_else(|| "unspecified GraphQL error".to_owned())
}

pub fn decode_detail(requested: AniListId, body: &str) -> DetailDecode {
    let envelope: dto::GraphQlEnvelope<dto::DetailData> = match serde_json::from_str(body) {
        Ok(envelope) => envelope,
        Err(error) => return DetailDecode::Decode(error.to_string()),
    };

    // `data: null` plus errors is a real failure. `data.Media: null` plus errors
    // is AniList's not-found, which arrives with HTTP 404 but is an answer, not
    // a fault — retrying it forever would be wrong.
    let Some(data) = envelope.data else {
        return if envelope.errors.is_empty() {
            DetailDecode::Decode("envelope had neither data nor errors".to_owned())
        } else {
            DetailDecode::GraphQl(first_error(&envelope.errors))
        };
    };

    match parser::parse_detail(requested, Some(&data)) {
        DetailResult::Observed(observation) => DetailDecode::Observed(observation),
        DetailResult::NotFound => DetailDecode::NotFound,
        DetailResult::Invalid(error) => DetailDecode::InvalidItem(error),
        DetailResult::IdMismatch {
            requested,
            returned,
        } => DetailDecode::Integrity {
            requested,
            returned,
        },
    }
}

pub fn decode_batch(requested: &[AniListId], body: &str) -> BatchDecode {
    let envelope: dto::GraphQlEnvelope<dto::PageData> = match serde_json::from_str(body) {
        Ok(envelope) => envelope,
        Err(error) => return BatchDecode::Decode(error.to_string()),
    };

    let Some(data) = envelope.data else {
        return if envelope.errors.is_empty() {
            BatchDecode::Decode("envelope had neither data nor errors".to_owned())
        } else {
            BatchDecode::GraphQl(first_error(&envelope.errors))
        };
    };

    // Data present alongside errors is the partial case: accept the items that
    // resolved, and let the omitted ones fall through as Missing.
    match parser::parse_batch(requested, Some(&data)) {
        Ok(items) => BatchDecode::Items(items),
        Err(error) => BatchDecode::Integrity(error),
    }
}

pub fn decode_search(body: &str) -> SearchDecode {
    let envelope: dto::GraphQlEnvelope<dto::PageData> = match serde_json::from_str(body) {
        Ok(envelope) => envelope,
        Err(error) => return SearchDecode::Decode(error.to_string()),
    };

    match envelope.data {
        Some(data) => SearchDecode::Candidates(parser::parse_search(Some(&data))),
        None if envelope.errors.is_empty() => {
            SearchDecode::Decode("envelope had neither data nor errors".to_owned())
        }
        None => SearchDecode::GraphQl(first_error(&envelope.errors)),
    }
}
