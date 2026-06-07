//! `animesh sync` — refresh metadata_cache for every active follow.
//!
//! Explicit, observable, partial-success-tolerant. Honors per-item
//! failures (logs them but continues) and stamps the kv with last
//! attempt/success/error so `doctor` can surface sync health.

use anyhow::{Context, Result};

use crate::{
    anilist::AniListClient,
    store::{CacheEntry, Db, ListFilter, TtlConfig},
};

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
pub async fn sync_inner(
    db: &mut Db,
    client: &AniListClient,
    now: i64,
) -> Result<SyncReport> {
    db.kv_set(KV_LAST_ATTEMPT, &now.to_string(), now)?;

    let items = db.list_follows(ListFilter::Active).context("list_follows")?;
    let total = items.len();
    let ttl = TtlConfig::from_env();
    let mut succeeded = 0usize;
    let mut failures: Vec<(String, String)> = Vec::new();

    for item in &items {
        // Only the anilist source is wired in v0.3. Other sources are
        // silently skipped — SP-7 will route by `item.source`.
        if item.source != "anilist" {
            continue;
        }
        let id_n: i64 = match item.source_id.parse() {
            Ok(n) => n,
            Err(_) => {
                failures.push((item.source_id.clone(), "non-numeric anilist id".into()));
                continue;
            }
        };
        match client.by_id(id_n).await {
            Ok(Some(media)) => {
                let entry = CacheEntry::from_media(&media, &ttl, now);
                if let Err(e) = db.upsert_cache(&entry) {
                    failures.push((item.source_id.clone(), format!("cache upsert: {e:#}")));
                } else {
                    succeeded += 1;
                }
            }
            Ok(None) => failures.push((item.source_id.clone(), "not found on AniList".into())),
            Err(e) => failures.push((item.source_id.clone(), format!("{e:#}"))),
        }
    }

    if failures.is_empty() {
        db.kv_set(KV_LAST_SUCCESS, &now.to_string(), now)?;
        db.kv_delete(KV_LAST_ERROR)?;
    } else {
        let summary = format!("{}/{} failed", failures.len(), total);
        db.kv_set(KV_LAST_ERROR, &summary, now)?;
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

    #[tokio::test]
    async fn sync_refreshes_cache_for_all_active_follows() {
        let mut server = mockito::Server::new_async().await;
        // by_id returns whatever we hand back per request; we don't
        // try to match on body here since mockito matchers add
        // complexity for the same coverage.
        let _m = server
            .mock("POST", "/")
            .with_status(200)
            .with_body(body_for_id(21, "One Piece", "RELEASING"))
            .expect_at_least(1)
            .create_async()
            .await;
        let client = AniListClient::with_base_url(server.url());

        let mut db = Db::open_in_memory().unwrap();
        db.add_follow("anilist", "21", "anime", "One Piece", 100)
            .unwrap();
        let report = sync_inner(&mut db, &client, 1_000).await.unwrap();
        assert_eq!(report.total, 1);
        assert_eq!(report.succeeded, 1);
        assert!(report.failures.is_empty());

        let cached = db.get_cache("anilist", "21").unwrap().unwrap();
        assert_eq!(cached.status.as_deref(), Some("RELEASING"));
        assert_eq!(cached.fetched_at, 1_000);

        let (last_attempt, _) = db.kv_get(KV_LAST_ATTEMPT).unwrap().unwrap();
        assert_eq!(last_attempt, "1000");
        let (last_success, _) = db.kv_get(KV_LAST_SUCCESS).unwrap().unwrap();
        assert_eq!(last_success, "1000");
        assert!(db.kv_get(KV_LAST_ERROR).unwrap().is_none());
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

        let mut db = Db::open_in_memory().unwrap();
        db.add_follow("anilist", "21", "anime", "One Piece", 100)
            .unwrap();
        db.add_follow("anilist", "22", "anime", "Naruto", 100)
            .unwrap();
        let report = sync_inner(&mut db, &client, 1_000).await.unwrap();
        assert_eq!(report.total, 2);
        assert_eq!(report.succeeded, 0);
        assert_eq!(report.failures.len(), 2);

        let (err, _) = db.kv_get(KV_LAST_ERROR).unwrap().unwrap();
        assert!(err.contains("2/2"));
        // last_success is not stamped on a fully-failed run.
        assert!(db.kv_get(KV_LAST_SUCCESS).unwrap().is_none());
    }

    #[tokio::test]
    async fn sync_empty_library_succeeds_with_zero_total() {
        let mut server = mockito::Server::new_async().await;
        // Mock is intentionally unused — no follows means no calls.
        let _m = server
            .mock("POST", "/")
            .with_status(200)
            .with_body("{}")
            .expect(0)
            .create_async()
            .await;
        let client = AniListClient::with_base_url(server.url());

        let mut db = Db::open_in_memory().unwrap();
        let report = sync_inner(&mut db, &client, 500).await.unwrap();
        assert_eq!(report.total, 0);
        assert_eq!(report.succeeded, 0);
        assert!(db.kv_get(KV_LAST_SUCCESS).unwrap().is_some());
    }
}
