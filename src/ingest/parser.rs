use anyhow::Result;

use super::{RawSourcePayload, SourceObservation};

pub trait SourceParser: Send + Sync {
    #[allow(dead_code)]
    fn source(&self) -> &'static str;

    fn parse_search(&self, payload: &RawSourcePayload) -> Result<Vec<SourceObservation>>;

    fn parse_fetch(&self, payload: &RawSourcePayload) -> Result<Option<SourceObservation>>;
}
