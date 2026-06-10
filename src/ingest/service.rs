//! Source-agnostic ingestion orchestration for discovery.
//!
//! The UI behavior is intentionally split:
//! - typing searches local `source_candidate_fts` only
//! - Enter asks this service to query plugged sources, persist raw payloads,
//!   parse observations, and materialize candidates through `Library`

use anyhow::{anyhow, Context, Result};

use crate::ingest::RawSourcePayload;
use crate::library::Library;
use crate::search::source_candidate::SourceCandidateResult;
use crate::sources::SourceRegistry;
use crate::store::SourceParseError;

pub struct IngestSearchService<'a> {
    library: &'a Library,
    sources: &'a SourceRegistry,
}

impl<'a> IngestSearchService<'a> {
    pub fn new(library: &'a Library, sources: &'a SourceRegistry) -> Self {
        Self { library, sources }
    }

    /// Query all plugged sources, ingest whatever succeeds, then return local
    /// FTS results for the same query. Source failures are isolated so one bad
    /// adapter does not prevent candidates from other adapters surfacing.
    pub async fn refresh_candidates(
        &self,
        query: &str,
        limit: u32,
        now: i64,
    ) -> Result<Vec<SourceCandidateResult>> {
        let mut failures = Vec::new();
        for source in self.sources.adapters() {
            match source.search(query, limit, now).await {
                Ok(payloads) => {
                    for payload in payloads {
                        if let Err(err) = self.ingest_search_payload(&payload, source.parser(), now)
                        {
                            failures.push(format!("{} ingest: {err:#}", source.source()));
                        }
                    }
                }
                Err(err) => failures.push(format!("{} search: {err:#}", source.source())),
            }
        }

        let hits = self
            .library
            .search_source_candidates(query, limit)
            .context("search source candidates after ingest")?;
        if hits.is_empty() && !failures.is_empty() {
            return Err(anyhow!(failures.join("; ")));
        }
        Ok(hits)
    }

    fn ingest_search_payload(
        &self,
        payload: &RawSourcePayload,
        parser: &dyn crate::ingest::SourceParser,
        now: i64,
    ) -> Result<()> {
        self.library
            .store_raw_source_payload(payload)
            .with_context(|| format!("store raw {} payload", payload.source))?;

        let observations = match parser.parse_search(payload) {
            Ok(observations) => observations,
            Err(err) => {
                let _ = self.library.record_source_parse_error(&SourceParseError {
                    raw_payload_id: payload.id.clone(),
                    source: payload.source.clone(),
                    endpoint: payload.endpoint.clone(),
                    error: format!("{err:#}"),
                    occurred_at: now,
                });
                return Err(err)
                    .with_context(|| format!("parse {} search payload", payload.source));
            }
        };

        for observation in observations {
            self.library
                .store_source_observation(&observation)
                .with_context(|| {
                    format!(
                        "store {} source observation {}",
                        observation.source, observation.source_id
                    )
                })?;
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;

    use crate::sources::anilist::{AniListClient, AniListSource};
    use crate::sources::SourceRegistry;
    use crate::time::FixedClock;

    #[tokio::test]
    async fn refresh_candidates_uses_plugged_sources_and_returns_local_hits() {
        let mut server = mockito::Server::new_async().await;
        let body = r#"{
            "data": {"Page": {"media": [{
                "id": 21,
                "title": {"romaji": "ONE PIECE", "english": "One Piece", "native": "ワンピース"},
                "status": "RELEASING",
                "episodes": null,
                "format": "TV",
                "nextAiringEpisode": {"episode": 1100, "airingAt": 1700000000}
            }]}}
        }"#;
        let _m = server
            .mock("POST", "/")
            .with_status(200)
            .with_body(body)
            .create_async()
            .await;
        let library = Library::open_in_memory(Arc::new(FixedClock(1_000))).unwrap();
        let registry = SourceRegistry::new(vec![Box::new(AniListSource::with_client(
            AniListClient::with_base_url(server.url()),
        ))]);
        let service = IngestSearchService::new(&library, &registry);

        let hits = service
            .refresh_candidates("one piece", 10, 1_000)
            .await
            .unwrap();
        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0].source, "anilist");
        assert_eq!(hits[0].source_id, "21");
        assert_eq!(hits[0].display_title, "One Piece");
    }
}
