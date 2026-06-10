//! `:sync` — source-agnostic bounded refresh for due followed source refs.
//!
//! Network stays inside source adapters. The command is thin marshalling:
//! stamp sync health keys, call [`RefreshService`], and return a compact report
//! for the TUI toast.

use std::sync::Arc;

use anyhow::Result;

use crate::ingest::budget::RequestBudget;
use crate::ingest::refresh::RefreshService;
use crate::library::Library as Facade;
use crate::sources::SourceRegistry;

const KV_LAST_ATTEMPT: &str = "sync.last_attempt_at";
const KV_LAST_SUCCESS: &str = "sync.last_success_at";
const KV_LAST_ERROR: &str = "sync.last_error";

#[derive(Debug)]
pub struct SyncReport {
    pub total: usize,
    pub succeeded: usize,
    /// (source_id, reason)
    pub failures: Vec<(String, String)>,
}

pub async fn sync_inner_default(facade: &Arc<Facade>, now: i64) -> Result<SyncReport> {
    let sources = SourceRegistry::production();
    sync_inner_with_sources(
        facade,
        &sources,
        RequestBudget::default().max_manual_sync_requests,
        now,
    )
    .await
}

pub async fn sync_inner_with_sources(
    facade: &Arc<Facade>,
    sources: &SourceRegistry,
    budget: usize,
    now: i64,
) -> Result<SyncReport> {
    facade.kv_set(KV_LAST_ATTEMPT, &now.to_string())?;

    let service = RefreshService::new(facade, sources);
    let refresh = service.refresh_due(budget, now).await?;
    let total = refresh.attempted + refresh.skipped_missing_adapter;
    let failures: Vec<(String, String)> = refresh
        .failures
        .into_iter()
        .map(|(source, source_id, error)| (format!("{source}:{source_id}"), error))
        .collect();

    if failures.is_empty() {
        facade.kv_set(KV_LAST_SUCCESS, &now.to_string())?;
        facade.kv_delete(KV_LAST_ERROR)?;
    } else {
        let summary = format!("{}/{} failed", failures.len(), total);
        facade.kv_set(KV_LAST_ERROR, &summary)?;
    }

    Ok(SyncReport {
        total,
        succeeded: refresh.succeeded,
        failures,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ids::{CanonicalId, ReleaseKind};
    use crate::sources::anilist::{AniListClient, AniListSource};
    use crate::store::SourceRefRefreshState;
    use crate::time::FixedClock;

    fn body_for_id(id: i64, title: &str, status: &str, next_episode: Option<(i64, i64)>) -> String {
        let next = match next_episode {
            Some((episode, airing_at)) => {
                format!(r#"{{"episode": {episode}, "airingAt": {airing_at}}}"#)
            }
            None => "null".into(),
        };
        format!(
            r#"{{"data": {{ "Media": {{
                "id": {id},
                "title": {{"romaji": "{title} Romaji", "english": "{title}", "native": "{title} Native"}},
                "status": "{status}", "episodes": 12, "format": "TV",
                "nextAiringEpisode": {next}
            }} }} }}"#
        )
    }

    fn facade(now: i64) -> Arc<Facade> {
        Arc::new(Facade::open_in_memory(Arc::new(FixedClock(now))).unwrap())
    }

    fn follow_due(facade: &Arc<Facade>, source_id: &str, title: &str, now: i64) -> CanonicalId {
        let cid = CanonicalId::legacy_from_source(ReleaseKind::Anime, "anilist", source_id);
        facade
            .follow_with_source(
                &cid,
                ReleaseKind::Anime,
                title,
                "anilist",
                source_id,
                Some(title),
                1.0,
            )
            .unwrap();
        facade
            .upsert_source_ref_refresh_state(&SourceRefRefreshState {
                source: "anilist".into(),
                source_id: source_id.into(),
                last_attempt_at: None,
                last_success_at: None,
                last_error: None,
                next_due_at: Some(now - 1),
                failure_count: 0,
            })
            .unwrap();
        cid
    }

    #[tokio::test]
    async fn sync_refreshes_due_source_refs_and_projects_schedule() {
        let mut server = mockito::Server::new_async().await;
        let _m = server
            .mock("POST", "/")
            .with_status(200)
            .with_body(body_for_id(
                21,
                "One Piece",
                "RELEASING",
                Some((1100, 2_000)),
            ))
            .expect_at_least(1)
            .create_async()
            .await;
        let facade = facade(1_000);
        let cid = follow_due(&facade, "21", "One Piece", 1_000);
        let sources = SourceRegistry::new(vec![Box::new(AniListSource::with_client(
            AniListClient::with_base_url(server.url()),
        ))]);

        let report = sync_inner_with_sources(&facade, &sources, 50, 1_000)
            .await
            .unwrap();
        assert_eq!(report.total, 1);
        assert_eq!(report.succeeded, 1);
        assert!(report.failures.is_empty());

        let events = facade.schedule_events_for_canonical(&cid).unwrap();
        assert_eq!(events.len(), 1);
        assert_eq!(events[0].source_event_id, "anilist:airing:21:1100");
        assert_eq!(
            facade.kv_get(KV_LAST_SUCCESS).unwrap().as_deref(),
            Some("1000")
        );
        assert!(facade.kv_get(KV_LAST_ERROR).unwrap().is_none());
    }

    #[tokio::test]
    async fn sync_with_anilist_5xx_records_failure_but_continues() {
        let mut server = mockito::Server::new_async().await;
        let _m = server
            .mock("POST", "/")
            .with_status(503)
            .with_body("server error")
            .expect_at_least(2)
            .create_async()
            .await;
        let facade = facade(1_000);
        follow_due(&facade, "21", "One Piece", 1_000);
        follow_due(&facade, "22", "Naruto", 1_000);
        let sources = SourceRegistry::new(vec![Box::new(AniListSource::with_client(
            AniListClient::with_base_url(server.url()),
        ))]);

        let report = sync_inner_with_sources(&facade, &sources, 50, 1_000)
            .await
            .unwrap();
        assert_eq!(report.total, 2);
        assert_eq!(report.succeeded, 0);
        assert_eq!(report.failures.len(), 2);

        let err = facade.kv_get(KV_LAST_ERROR).unwrap().unwrap();
        assert!(err.contains("2/2"));
        assert!(facade.kv_get(KV_LAST_SUCCESS).unwrap().is_none());
    }

    #[tokio::test]
    async fn sync_empty_due_set_succeeds_with_zero_total() {
        let mut server = mockito::Server::new_async().await;
        let _m = server
            .mock("POST", "/")
            .with_status(200)
            .with_body("{}")
            .expect(0)
            .create_async()
            .await;
        let facade = facade(500);
        let sources = SourceRegistry::new(vec![Box::new(AniListSource::with_client(
            AniListClient::with_base_url(server.url()),
        ))]);
        let report = sync_inner_with_sources(&facade, &sources, 50, 500)
            .await
            .unwrap();
        assert_eq!(report.total, 0);
        assert_eq!(report.succeeded, 0);
        assert!(facade.kv_get(KV_LAST_SUCCESS).unwrap().is_some());
    }
}
