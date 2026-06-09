//! Orchestration placeholder for the ingestion pipeline.
//!
//! Kept intentionally small for now: HTTP callers can construct a
//! `RawSourcePayload`, parsers produce `SourceObservation`, and `Library`
//! owns persistence. The async fetch orchestration will land after the
//! storage/parser contracts are proven by fixtures.
