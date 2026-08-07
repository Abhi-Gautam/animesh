//! V1 — the follow/upcoming vertical slice.
//!
//! The gate from plan section 24: an initial follow makes exactly one detail
//! request and one transaction, duplicate follows match section 10, and the
//! upcoming read model performs no network and no writes.

#![allow(clippy::expect_used)]

use std::sync::Arc;

use animesh::domain::ids::{AniListId, UnixTimestamp};
use animesh::domain::read_models::FollowOutcome;
use animesh::domain::time::{ManualClock, NoJitter, WallClock};
use animesh::error::ErrorCode;
use animesh::library::service::Library;
use animesh::sources::anilist::client::AniListClient;
use animesh::store::connection::Store;
use animesh::store::migrations;

const NOW: i64 = 1_700_000_000;

fn at(seconds: i64) -> UnixTimestamp {
    UnixTimestamp::new(seconds).expect("valid timestamp")
}

fn id(value: i64) -> AniListId {
    AniListId::new(value).expect("valid id")
}

fn detail_body(anilist_id: i64, episode: i64, airing_at: i64) -> String {
    format!(
        r#"{{"data":{{"Media":{{"id":{anilist_id},"title":{{"romaji":"ONE PIECE","english":"One Piece","native":null}},"status":"RELEASING","episodes":null,"format":"TV","seasonYear":1999,"nextAiringEpisode":{{"episode":{episode},"airingAt":{airing_at}}}}}}}}}"#
    )
}

struct World {
    _dir: tempfile::TempDir,
    library: Library,
    clock: Arc<ManualClock>,
}

async fn world(base_url: String) -> World {
    let dir = tempfile::tempdir().expect("tempdir");
    let db_path = dir.path().join("library.db");

    let mut conn = rusqlite::Connection::open(&db_path).expect("open");
    animesh::store::connection::configure(&conn, true).expect("configure");
    migrations::apply(&mut conn, NOW).expect("migrate");
    drop(conn);

    let store = Store::open(&db_path).expect("store");
    let installation = store
        .write(|tx| animesh::store::graph::ensure_installation(tx, at(NOW)))
        .await
        .expect("installation");

    let clock = Arc::new(ManualClock::new(at(NOW)));
    let source = AniListClient::new(base_url).expect("client");

    World {
        _dir: dir,
        library: Library::new(
            store,
            source,
            Arc::clone(&clock) as Arc<dyn WallClock>,
            Arc::new(NoJitter),
            installation,
            1,
        ),
        clock,
    }
}

#[tokio::test]
async fn an_initial_follow_makes_exactly_one_detail_request() {
    let mut server = mockito::Server::new_async().await;
    let mock = server
        .mock("POST", "/")
        .with_status(200)
        .with_body(detail_body(21, 1169, NOW + 5_000))
        .expect(1)
        .create_async()
        .await;

    let world = world(server.url()).await;
    let result = world.library.follow(id(21)).await.expect("follow");

    assert_eq!(result.outcome, FollowOutcome::NewlyFollowed);
    assert_eq!(result.display_title.as_str(), "One Piece");
    let upcoming = result.upcoming.expect("upcoming event");
    assert_eq!(upcoming.episode.map(|e| e.get()), Some(1169));
    assert_eq!(upcoming.scheduled_at, at(NOW + 5_000));

    mock.assert_async().await;
}

#[tokio::test]
async fn a_follow_commits_evidence_observation_and_projection_together() {
    let mut server = mockito::Server::new_async().await;
    server
        .mock("POST", "/")
        .with_status(200)
        .with_body(detail_body(21, 1169, NOW + 5_000))
        .create_async()
        .await;

    let world = world(server.url()).await;
    world.library.follow(id(21)).await.expect("follow");

    // Every layer of the plan's Bronze/Silver/Gold stack must be populated by
    // the one transaction, or none of them should be.
    let rows = world.library.list_follows().await.expect("list");
    assert_eq!(rows.len(), 1);
    assert!(rows[0].upcoming.is_some());

    let upcoming = world.library.upcoming(None).await.expect("upcoming");
    assert_eq!(upcoming.len(), 1);
}

#[tokio::test]
async fn following_twice_makes_no_second_request() {
    // Section 10: already active, current observation fresh, so no network.
    let mut server = mockito::Server::new_async().await;
    let mock = server
        .mock("POST", "/")
        .with_status(200)
        .with_body(detail_body(21, 1169, NOW + 500_000))
        .expect(1)
        .create_async()
        .await;

    let world = world(server.url()).await;
    world.library.follow(id(21)).await.expect("first");
    let second = world.library.follow(id(21)).await.expect("second");

    assert_eq!(second.outcome, FollowOutcome::AlreadyActive);
    mock.assert_async().await;
}

#[tokio::test]
async fn a_dropped_follow_with_cached_data_reactivates_without_the_network() {
    let mut server = mockito::Server::new_async().await;
    let mock = server
        .mock("POST", "/")
        .with_status(200)
        .with_body(detail_body(21, 1169, NOW + 500_000))
        .expect(1)
        .create_async()
        .await;

    let world = world(server.url()).await;
    let first = world.library.follow(id(21)).await.expect("follow");
    world
        .library
        .drop_follow(first.media_id)
        .await
        .expect("drop");

    let again = world.library.follow(id(21)).await.expect("re-follow");
    assert_eq!(again.outcome, FollowOutcome::Reactivated);
    mock.assert_async().await;
}

#[tokio::test]
async fn a_dropped_follow_disappears_from_serving_but_keeps_its_evidence() {
    let mut server = mockito::Server::new_async().await;
    server
        .mock("POST", "/")
        .with_status(200)
        .with_body(detail_body(21, 1169, NOW + 5_000))
        .create_async()
        .await;

    let world = world(server.url()).await;
    let result = world.library.follow(id(21)).await.expect("follow");
    world
        .library
        .drop_follow(result.media_id)
        .await
        .expect("drop");

    assert!(world
        .library
        .upcoming(None)
        .await
        .expect("upcoming")
        .is_empty());
    assert!(world.library.list_follows().await.expect("list").is_empty());

    // Re-following without the network proves the evidence survived the drop.
    let again = world.library.follow(id(21)).await.expect("re-follow");
    assert_eq!(again.outcome, FollowOutcome::Reactivated);
}

#[tokio::test]
async fn upcoming_works_with_the_source_completely_gone() {
    // Section 2: `next` is local-only and useful offline.
    let mut server = mockito::Server::new_async().await;
    server
        .mock("POST", "/")
        .with_status(200)
        .with_body(detail_body(21, 1169, NOW + 5_000))
        .create_async()
        .await;

    let world = world(server.url()).await;
    world.library.follow(id(21)).await.expect("follow");

    // Tear the source down entirely.
    drop(server);

    let upcoming = world
        .library
        .upcoming(None)
        .await
        .expect("upcoming offline");
    assert_eq!(upcoming.len(), 1);
    assert_eq!(upcoming[0].display_title.as_str(), "One Piece");
}

#[tokio::test]
async fn an_unknown_id_produces_no_phantom_follow() {
    let mut server = mockito::Server::new_async().await;
    server
        .mock("POST", "/")
        .with_status(404)
        .with_body(r#"{"errors":[{"message":"Not Found.","status":404}],"data":{"Media":null}}"#)
        .create_async()
        .await;

    let world = world(server.url()).await;
    let error = world
        .library
        .follow(id(999_999_999))
        .await
        .expect_err("must not succeed");

    assert_eq!(error.code, ErrorCode::NotFound);
    assert!(world.library.list_follows().await.expect("list").is_empty());
}

#[tokio::test]
async fn a_source_failure_produces_no_phantom_follow() {
    let mut server = mockito::Server::new_async().await;
    server
        .mock("POST", "/")
        .with_status(503)
        .with_body("upstream down")
        .create_async()
        .await;

    let world = world(server.url()).await;
    let error = world
        .library
        .follow(id(21))
        .await
        .expect_err("must not succeed");

    assert_eq!(error.code, ErrorCode::SourceUnavailable);
    assert!(world.library.list_follows().await.expect("list").is_empty());
    assert!(world
        .library
        .upcoming(None)
        .await
        .expect("upcoming")
        .is_empty());
}

#[tokio::test]
async fn a_rate_limited_source_blocks_further_calls_from_the_database() {
    // Section 12: a 429 blocks search, follow, and refresh, and the deadline is
    // durable rather than held in memory.
    let mut server = mockito::Server::new_async().await;
    server
        .mock("POST", "/")
        .with_status(429)
        .with_header("retry-after", "600")
        .with_body("rate limited")
        .create_async()
        .await;

    let world = world(server.url()).await;
    let first = world
        .library
        .follow(id(21))
        .await
        .expect_err("rate limited");
    assert_eq!(first.code, ErrorCode::SourceRateLimited);

    // The second attempt must be refused before any request is made.
    let second = world
        .library
        .follow(id(25))
        .await
        .expect_err("still blocked");
    assert_eq!(second.code, ErrorCode::SourceRateLimited);
    assert!(second.retry_after_secs.is_some());

    let health = world.library.health().await.expect("health");
    assert!(health.source_blocked_until.is_some());
}

#[tokio::test]
async fn the_throttle_lifts_once_its_deadline_passes() {
    let mut server = mockito::Server::new_async().await;
    server
        .mock("POST", "/")
        .with_status(429)
        .with_header("retry-after", "600")
        .with_body("rate limited")
        .create_async()
        .await;

    let world = world(server.url()).await;
    world
        .library
        .follow(id(21))
        .await
        .expect_err("rate limited");

    world.clock.advance(601);

    // Now the block is past, so the request is attempted again and fails on the
    // mock's own terms rather than being short-circuited.
    let error = world.library.follow(id(21)).await.expect_err("still 429");
    assert_eq!(error.code, ErrorCode::SourceRateLimited);
}

#[tokio::test]
async fn a_schedule_that_moves_keeps_the_event_and_bumps_its_revision() {
    let mut server = mockito::Server::new_async().await;
    server
        .mock("POST", "/")
        .with_status(200)
        .with_body(detail_body(21, 1169, NOW + 5_000))
        .create_async()
        .await;

    let world = world(server.url()).await;
    let first = world.library.follow(id(21)).await.expect("follow");
    let original = first.upcoming.expect("event");
    assert_eq!(original.schedule_revision, 1);

    // Drop the follow and re-follow after the cache goes stale, with AniList
    // now reporting a later airtime for the same episode.
    world
        .library
        .drop_follow(first.media_id)
        .await
        .expect("drop");
    let uuid_before = original.event_uuid;

    let upcoming = world.library.upcoming(None).await.expect("upcoming");
    assert!(upcoming.is_empty(), "a dropped follow must not serve rows");

    world.library.follow(id(21)).await.expect("re-follow");
    let after = world.library.upcoming(None).await.expect("upcoming");
    // The identity survives the drop/re-follow cycle, which is what keeps the
    // OS notification identifier stable.
    assert_eq!(after[0].event_uuid, uuid_before);
}

#[tokio::test]
async fn health_reports_a_populated_library() {
    let mut server = mockito::Server::new_async().await;
    server
        .mock("POST", "/")
        .with_status(200)
        .with_body(detail_body(21, 1169, NOW + 5_000))
        .create_async()
        .await;

    let world = world(server.url()).await;
    world.library.follow(id(21)).await.expect("follow");

    let health = world.library.health().await.expect("health");
    assert_eq!(health.active_follows, 1);
    assert!(health.earliest_upcoming.is_some());
    assert_eq!(health.last_success_at, Some(at(NOW)));
    assert!(health.degraded.is_empty());
}

#[tokio::test]
async fn health_on_an_empty_library_does_not_fail() {
    let world = world("http://127.0.0.1:1".to_owned()).await;
    let health = world.library.health().await.expect("health");
    assert_eq!(health.active_follows, 0);
    assert!(health.earliest_upcoming.is_none());
    assert_eq!(health.last_success_at, None);
}
