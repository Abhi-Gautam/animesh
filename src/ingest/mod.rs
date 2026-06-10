//! Source ingestion domain types.
//!
//! This module owns the normalized shapes for the raw -> observation ->
//! candidate ETL path. Parsers are pure functions that produce these
//! structs; `Library` persists them through `store`.

pub(crate) mod budget;
pub(crate) mod follow;
pub(crate) mod observation;
pub(crate) mod parser;
pub(crate) mod refresh;
pub(crate) mod request;
pub(crate) mod service;

pub(crate) use observation::{
    AliasObservation, ExternalIdObservation, ImageObservation, LinkObservation,
    ReleaseEventObservation, SourceObservation, TimePrecision,
};
pub(crate) use parser::SourceParser;
pub(crate) use request::{HttpMethod, RawSourcePayload};
