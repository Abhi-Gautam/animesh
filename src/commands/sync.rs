//! `:sync` — refresh metadata_cache for every active follow.
//!
//! Explicit, observable, partial-success-tolerant. Honors per-item
//! failures (logs them but continues) and stamps the kv with last
//! attempt/success/error so `:doctor` can surface sync health.
//!
//! v0.5: iterates the canonical graph via [`Facade::followed`] and
//! uses the canonical's primary source_ref to drive each refresh.
//! Only the `anilist` source is wired today — other sources fall
//! through silently until their adapters land in the sync engine.

use std::sync::Arc;

use anyhow::{Context, Result};

use crate::library::Library as Facade;
use crate::sources::anilist::AniListClient;
use crate::store::{CacheEntry, TtlConfig};

const KV_LAST_ATTEMPT: &str = "sync.last_attempt_at";
const KV_LAST_SUCCESS: &str = "sync.last_success_at";
const KV_LAST_ERROR: &str = "sync.last_error";

/// Per-item outcome of one sync pass.
#[derive(Debug)]
pub struct SyncReport {
    pub total: usize,
    pub succeeded: usize,
    /// (source_id, reason)
    pub failures: Vec<(String, String)>,
}

/// Inner sync execution with all I/O dependencies injected.
pub async fn sync_inner_default(facade: &Arc<Facade>, now: i64) -> Result<SyncReport> {
    let client = AniListClient::new();
    sync_inner(facade, &client, now).await
}

pub async fn sync_inner(
    facade: &Arc<Facade>,
    client: &AniListClient,
    now: i64,
) -> Result<SyncReport> {
    facade.kv_set(KV_LAST_ATTEMPT, &now.to_string())?;

    let canonicals = facade.followed().context("followed canonicals")?;
    let total = canonicals.len();
    let ttl = TtlConfig::from_env();
    let mut succeeded = 0usize;
    let mut failures: Vec<(String, String)> = Vec::new();

    for canonical in &canonicals {
        // Take every attached source_ref so a canonical can refresh
        // across multiple sources in a single tick. For v0.5 only the
        // anilist branch produces a network call — other sources are
        // silently skipped until their adapters land here.
        let refs = match facade.source_refs_for(&canonical.id) {
            Ok(refs) => refs,
            Err(e) => {
                failures.push((canonical.id.to_string(), format!("source_refs: {e:#}")));
                continue;
            }
        };
        let Some(primary) = refs.into_iter().next() else {
            // Defensive: a followed canonical with no source_ref
            // shouldn't exist (follow_with_source always attaches one).
            // Log to the failure list so doctor surfaces it.
            failures.push((canonical.id.to_string(), "no source_ref attached".into()));
            continue;
        };
        if primary.source != "anilist" {
            continue;
        }
        let id_n: i64 = match primary.source_id.parse() {
            Ok(n) => n,
            Err(_) => {
                failures.push((primary.source_id.clone(), "non-numeric anilist id".into()));
                continue;
            }
        };
        match client.by_id(id_n).await {
            Ok(Some(media)) => {
                let entry = CacheEntry::from_media(&media, &ttl, now);
                if let Err(e) = facade.upsert_cache(&entry) {
                    failures.push((primary.source_id.clone(), format!("cache upsert: {e:#}")));
                } else {
                    succeeded += 1;
                }
            }
            Ok(None) => failures.push((primary.source_id.clone(), "not found on AniList".into())),
            Err(e) => failures.push((primary.source_id.clone(), format!("{e:#}"))),
        }
    }

    if failures.is_empty() {
        facade.kv_set(KV_LAST_SUCCESS, &now.to_string())?;
        facade.kv_delete(KV_LAST_ERROR)?;
    } else {
        let summary = format!("{}/{} failed", failures.len(), total);
        facade.kv_set(KV_LAST_ERROR, &summary)?;
    }

    Ok(SyncReport {
        total,
        succeeded,
        failures,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ids::{CanonicalId, ReleaseKind};
    use crate::time::FixedClock;

    fn body_for_id(id: i64, title: &str, status: &str) -> String {
        format!(
            r#"{{"data": {{ "Media": {{
                "id": {id},
                "title": {{"romaji": "{title}", "english": "{title}", "native": "{title}"}},
                "status": "{status}", "episodes": 12, "format": "TV",
                "nextAiringEpisode": null
            }} }} }}"#
        )
    }

    fn facade(now: i64) -> Arc<Facade> {
        Arc::new(Facade::open_in_memory(Arc::new(FixedClock(now))).unwrap())
    }

    async fn follow_one(facade: &Arc<Facade>, source_id: &str, title: &str) -> CanonicalId {
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
        cid
    }

    #[tokio::test]
    async fn sync_refreshes_cache_for_all_active_follows() {
        let mut server = mockito::Server::new_async().await;
        let _m = server
            .mock("POST", "/")
            .with_status(200)
            .with_body(body_for_id(21, "One Piece", "RELEASING"))
            .expect_at_least(1)
            .create_async()
            .await;
        let client = AniListClient::with_base_url(server.url());
        let facade = facade(100);
        follow_one(&facade, "21", "One Piece").await;

        let report = sync_inner(&facade, &client, 1_000).await.unwrap();
        assert_eq!(report.total, 1);
        assert_eq!(report.succeeded, 1);
        assert!(report.failures.is_empty());

        let cached = facade.get_cache("anilist", "21").unwrap().unwrap();
        assert_eq!(cached.status.as_deref(), Some("RELEASING"));
        assert_eq!(cached.fetched_at, 1_000);

        let last_attempt = facade.kv_get(KV_LAST_ATTEMPT).unwrap().unwrap();
        assert_eq!(last_attempt, "1000");
        let last_success = facade.kv_get(KV_LAST_SUCCESS).unwrap().unwrap();
        assert_eq!(last_success, "1000");
        assert!(facade.kv_get(KV_LAST_ERROR).unwrap().is_none());
    }

    #[tokio::test]
    async fn sync_with_anilist_5xx_records_failure_but_continues() {
        let mut server = mockito::Server::new_async().await;
        let _m = server
            .mock("POST", "/")
            .with_status(503)
            .with_body("server error")
            .create_async()
            .await;
        let client = AniListClient::with_base_url(server.url());
        let facade = facade(100);
        follow_one(&facade, "21", "One Piece").await;
        follow_one(&facade, "22", "Naruto").await;

        let report = sync_inner(&facade, &client, 1_000).await.unwrap();
        assert_eq!(report.total, 2);
        assert_eq!(report.succeeded, 0);
        assert_eq!(report.failures.len(), 2);

        let err = facade.kv_get(KV_LAST_ERROR).unwrap().unwrap();
        assert!(err.contains("2/2"));
        assert!(facade.kv_get(KV_LAST_SUCCESS).unwrap().is_none());
    }

    #[tokio::test]
    async fn sync_empty_library_succeeds_with_zero_total() {
        let mut server = mockito::Server::new_async().await;
        let _m = server
            .mock("POST", "/")
            .with_status(200)
            .with_body("{}")
            .expect(0)
            .create_async()
            .await;
        let client = AniListClient::with_base_url(server.url());
        let facade = facade(500);
        let report = sync_inner(&facade, &client, 500).await.unwrap();
        assert_eq!(report.total, 0);
        assert_eq!(report.succeeded, 0);
        assert!(facade.kv_get(KV_LAST_SUCCESS).unwrap().is_some());
    }
}
