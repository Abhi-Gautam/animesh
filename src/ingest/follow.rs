use anyhow::{anyhow, Context, Result};

use crate::ingest::budget::{failure_backoff, next_refresh_due_at};
use crate::library::Library;
use crate::search::source_candidate::SourceCandidateResult;
use crate::sources::SourceRegistry;
use crate::store::{CanonicalFollowOutcome, SourceParseError};

#[derive(Debug, Clone)]
pub(crate) struct FollowIngestReport {
    pub outcome: CanonicalFollowOutcome,
    pub candidate: SourceCandidateResult,
    pub detail_ingested: bool,
    pub projected_events: usize,
    #[allow(dead_code)]
    pub next_due_at: Option<i64>,
    pub warning: Option<String>,
}

pub(crate) struct FollowIngestService<'a> {
    library: &'a Library,
    sources: &'a SourceRegistry,
}

impl<'a> FollowIngestService<'a> {
    pub(crate) fn new(library: &'a Library, sources: &'a SourceRegistry) -> Self {
        Self { library, sources }
    }

    pub(crate) async fn follow_and_ingest(
        &self,
        candidate: &SourceCandidateResult,
        now: i64,
    ) -> Result<FollowIngestReport> {
        let outcome = self
            .library
            .follow_source_candidate(candidate)
            .context("persist follow intent")?;
        let canonical_id = Library::canonical_id_for_source_candidate(candidate);

        let Some(adapter) = self.sources.adapter(&candidate.source) else {
            let warning = format!("missing source adapter {:?}", candidate.source);
            let next_due_at = now + failure_backoff(1);
            self.library.record_source_ingest_failure(
                &candidate.source,
                &candidate.source_id,
                &warning,
                next_due_at,
            )?;
            return Ok(FollowIngestReport {
                outcome,
                candidate: candidate.clone(),
                detail_ingested: false,
                projected_events: 0,
                next_due_at: Some(next_due_at),
                warning: Some(warning),
            });
        };

        let payload = match adapter.ingest(&candidate.source_id, now).await {
            Ok(Some(payload)) => payload,
            Ok(None) => {
                let warning = format!(
                    "{} returned no detail payload for {}",
                    candidate.source, candidate.source_id
                );
                let next_due_at = now + failure_backoff(1);
                self.library.record_source_ingest_failure(
                    &candidate.source,
                    &candidate.source_id,
                    &warning,
                    next_due_at,
                )?;
                return Ok(FollowIngestReport {
                    outcome,
                    candidate: candidate.clone(),
                    detail_ingested: false,
                    projected_events: 0,
                    next_due_at: Some(next_due_at),
                    warning: Some(warning),
                });
            }
            Err(err) => {
                let warning = format!("{} detail ingest: {err:#}", candidate.source);
                let next_due_at = now + failure_backoff(1);
                self.library.record_source_ingest_failure(
                    &candidate.source,
                    &candidate.source_id,
                    &warning,
                    next_due_at,
                )?;
                return Ok(FollowIngestReport {
                    outcome,
                    candidate: candidate.clone(),
                    detail_ingested: false,
                    projected_events: 0,
                    next_due_at: Some(next_due_at),
                    warning: Some(warning),
                });
            }
        };

        let observation = match adapter.parser().parse_fetch(&payload) {
            Ok(Some(observation)) => observation,
            Ok(None) => {
                self.library.store_raw_source_payload(&payload)?;
                let warning = format!(
                    "{} parser produced no observation for {}",
                    candidate.source, candidate.source_id
                );
                let next_due_at = now + failure_backoff(1);
                self.library.record_source_ingest_failure(
                    &candidate.source,
                    &candidate.source_id,
                    &warning,
                    next_due_at,
                )?;
                return Ok(FollowIngestReport {
                    outcome,
                    candidate: candidate.clone(),
                    detail_ingested: false,
                    projected_events: 0,
                    next_due_at: Some(next_due_at),
                    warning: Some(warning),
                });
            }
            Err(err) => {
                self.library.store_raw_source_payload(&payload)?;
                let _ = self.library.record_source_parse_error(&SourceParseError {
                    raw_payload_id: payload.id.clone(),
                    source: payload.source.clone(),
                    endpoint: payload.endpoint.clone(),
                    error: format!("{err:#}"),
                    occurred_at: now,
                });
                let warning = format!("{} detail parse: {err:#}", candidate.source);
                let next_due_at = now + failure_backoff(1);
                self.library.record_source_ingest_failure(
                    &candidate.source,
                    &candidate.source_id,
                    &warning,
                    next_due_at,
                )?;
                return Ok(FollowIngestReport {
                    outcome,
                    candidate: candidate.clone(),
                    detail_ingested: false,
                    projected_events: 0,
                    next_due_at: Some(next_due_at),
                    warning: Some(warning),
                });
            }
        };

        if observation.source != candidate.source || observation.source_id != candidate.source_id {
            return Err(anyhow!(
                "detail observation ({}, {}) did not match selected candidate ({}, {})",
                observation.source,
                observation.source_id,
                candidate.source,
                candidate.source_id
            ));
        }

        let next_event_at = observation
            .release_events
            .iter()
            .filter_map(|event| event.scheduled_at)
            .filter(|scheduled_at| *scheduled_at > now)
            .min();
        let next_due_at = next_refresh_due_at(
            observation.kind,
            observation.status.as_deref(),
            next_event_at,
            now,
        );
        let success = self.library.record_source_ingest_success(
            &canonical_id,
            &payload,
            &observation,
            next_due_at,
        )?;

        Ok(FollowIngestReport {
            outcome,
            candidate: candidate.clone(),
            detail_ingested: true,
            projected_events: success.projected_events,
            next_due_at: Some(next_due_at),
            warning: None,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;

    use crate::ids::ReleaseKind;
    use crate::library::Library;
    use crate::sources::anilist::{AniListClient, AniListSource};
    use crate::sources::SourceRegistry;
    use crate::store::CanonicalFollowOutcome;
    use crate::time::FixedClock;

    fn candidate() -> SourceCandidateResult {
        SourceCandidateResult {
            source: "anilist".into(),
            source_id: "21".into(),
            kind: ReleaseKind::Anime,
            display_title: "One Piece".into(),
            search_text: "One Piece".into(),
            rank: 0.0,
        }
    }

    fn detail_body() -> &'static str {
        r#"{
            "data": {"Media": {
                "id": 21,
                "title": {"romaji": "One Piece Romaji", "english": "One Piece", "native": "ワンピース"},
                "status": "RELEASING",
                "episodes": null,
                "format": "TV",
                "nextAiringEpisode": {"episode": 1100, "airingAt": 2000}
            }}
        }"#
    }

    #[tokio::test]
    async fn follow_and_ingest_projects_events_and_refresh_state() {
        let mut server = mockito::Server::new_async().await;
        let _m = server
            .mock("POST", "/")
            .with_status(200)
            .with_body(detail_body())
            .expect(1)
            .create_async()
            .await;
        let library = Library::open_in_memory(Arc::new(FixedClock(1_000))).unwrap();
        let registry = SourceRegistry::new(vec![Box::new(AniListSource::with_client(
            AniListClient::with_base_url(server.url()),
        ))]);
        let service = FollowIngestService::new(&library, &registry);

        let report = service
            .follow_and_ingest(&candidate(), 1_000)
            .await
            .unwrap();
        assert_eq!(report.outcome, CanonicalFollowOutcome::NewlyFollowed);
        assert!(report.detail_ingested);
        assert_eq!(report.projected_events, 1);

        let canonical_id = Library::canonical_id_for_source_candidate(&candidate());
        let events = library
            .schedule_events_for_canonical(&canonical_id)
            .unwrap();
        assert_eq!(events.len(), 1);
        assert_eq!(events[0].source_event_id, "anilist:airing:21:1100");
        let state = library
            .get_source_ref_refresh_state("anilist", "21")
            .unwrap()
            .unwrap();
        assert_eq!(state.last_success_at, Some(1_000));
        assert_eq!(state.failure_count, 0);
        assert!(state.next_due_at.unwrap() > 1_000);
    }

    #[tokio::test]
    async fn follow_succeeds_when_detail_ingest_fails_and_records_retry_state() {
        let mut server = mockito::Server::new_async().await;
        let _m = server
            .mock("POST", "/")
            .with_status(503)
            .with_body("server error")
            .expect(1)
            .create_async()
            .await;
        let library = Library::open_in_memory(Arc::new(FixedClock(1_000))).unwrap();
        let registry = SourceRegistry::new(vec![Box::new(AniListSource::with_client(
            AniListClient::with_base_url(server.url()),
        ))]);
        let service = FollowIngestService::new(&library, &registry);

        let report = service
            .follow_and_ingest(&candidate(), 1_000)
            .await
            .unwrap();
        assert_eq!(report.outcome, CanonicalFollowOutcome::NewlyFollowed);
        assert!(!report.detail_ingested);
        assert!(report.warning.unwrap().contains("detail ingest"));

        let canonical_id = Library::canonical_id_for_source_candidate(&candidate());
        assert!(library.find_canonical(&canonical_id).unwrap().is_some());
        let state = library
            .get_source_ref_refresh_state("anilist", "21")
            .unwrap()
            .unwrap();
        assert_eq!(state.last_attempt_at, Some(1_000));
        assert_eq!(state.last_success_at, None);
        assert_eq!(state.failure_count, 1);
        assert!(state.last_error.unwrap().contains("detail ingest"));
    }
}
