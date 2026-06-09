use crate::ids::ReleaseKind;

#[derive(Debug, Clone, PartialEq)]
pub struct SourceCandidateResult {
    pub source: String,
    pub source_id: String,
    pub kind: ReleaseKind,
    pub display_title: String,
    pub search_text: String,
    pub rank: f64,
}
