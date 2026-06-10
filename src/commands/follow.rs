//! Follow operation — source-agnostic candidate follow.
//!
//! The TUI follow picker works with `SourceCandidateResult` rows produced by
//! ingestion. This command stays thin: Library is the mutation chokepoint.

use std::sync::Arc;

use anyhow::Result;

use crate::library::Library as Facade;
use crate::search::source_candidate::SourceCandidateResult;
use crate::store::CanonicalFollowOutcome;

#[derive(Debug)]
pub struct CandidateFollowReport {
    pub outcome: CanonicalFollowOutcome,
    pub display_title: String,
}

pub fn follow_candidate_inner(
    facade: &Arc<Facade>,
    candidate: &SourceCandidateResult,
) -> Result<CandidateFollowReport> {
    let outcome = facade.follow_source_candidate(candidate)?;
    Ok(CandidateFollowReport {
        outcome,
        display_title: candidate.display_title.clone(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ids::ReleaseKind;
    use crate::time::FixedClock;

    fn facade(now: i64) -> Arc<Facade> {
        Arc::new(Facade::open_in_memory(Arc::new(FixedClock(now))).unwrap())
    }

    #[test]
    fn follow_candidate_persists_source_scoped_canonical() {
        let facade = facade(1_000);
        let candidate = SourceCandidateResult {
            source: "jikan".into(),
            source_id: "5114".into(),
            kind: ReleaseKind::Anime,
            display_title: "Fullmetal Alchemist: Brotherhood".into(),
            search_text: "Fullmetal Alchemist Brotherhood".into(),
            rank: 0.0,
        };

        let report = follow_candidate_inner(&facade, &candidate).unwrap();
        assert_eq!(report.outcome, CanonicalFollowOutcome::NewlyFollowed);

        let list = facade.followed().unwrap();
        assert_eq!(list.len(), 1);
        assert_eq!(list[0].id.as_str(), "release:anime:legacy-jikan-5114");
        assert_eq!(list[0].display_title, "Fullmetal Alchemist: Brotherhood");
    }
}
