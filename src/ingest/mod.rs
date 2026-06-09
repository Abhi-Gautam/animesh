//! Source ingestion domain types.
//!
//! This module owns the normalized shapes for the raw -> observation ->
//! candidate ETL path. Parsers are pure functions that produce these
//! structs; `Library` persists them through `store`.

pub mod observation;
pub mod parser;
pub mod request;
pub mod service;

pub use observation::{
    AliasObservation, ExternalIdObservation, ImageObservation, LinkObservation,
    ReleaseEventObservation, SourceObservation, TimePrecision,
};
pub use parser::SourceParser;
pub use request::{HttpMethod, RawSourcePayload};
