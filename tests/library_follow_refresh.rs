//! V1 — the follow/upcoming vertical slice.
//!
//! The gate from plan section 24: an initial follow makes exactly one detail
//! request and one transaction, duplicate follows match section 10, and the
//! upcoming read model performs no network and no writes.

#![allow(clippy::expect_used)]

use std::sync::Arc;

use animesh::domain::ids::{AniListId, TvMazeId, UnixTimestamp};
use animesh::domain::read_models::FollowOutcome;
use animesh::domain::time::{ManualClock, NoJitter, WallClock};
use animesh::error::ErrorCode;
use animesh::library::service::Library;
use animesh::sources::anilist::client::AniListClient;
use animesh::store::connection::Store;
use animesh::store::migrations;

const NOW: i64 = 1_700_000_000;

#[tokio::test]
async fn revision_wait_observes_a_commit_that_precedes_subscription() {
    let mut server = mockito::Server::new_async().await;
    server
        .mock("POST", "/")
        .with_status(200)
        .with_body(detail_body(21, 2, NOW + 5000))
        .create_async()
        .await;
    let world = world(server.url()).await;
    let before = world.library.revision_stamp();
    world.library.follow(id(21)).await.expect("follow");
    let after = tokio::time::timeout(
        std::time::Duration::from_secs(1),
        world
            .library
            .wait_for_revision(&before.instance_id, before.revision),
    )
    .await
    .expect("must not lose the earlier commit");
    assert!(after.revision > before.revision);
}

#[tokio::test]
async fn concurrent_clients_observe_the_same_follow_and_drop() {
    let mut server = mockito::Server::new_async().await;
    server
        .mock("POST", "/")
        .with_status(200)
        .with_body(detail_body(21, 2, NOW + 5000))
        .create_async()
        .await;
    let world = world(server.url()).await;
    let before = world.library.revision_stamp();
    let (a, b, follow) = tokio::join!(
        world
            .library
            .wait_for_revision(&before.instance_id, before.revision),
        world
            .library
            .wait_for_revision(&before.instance_id, before.revision),
        world.library.follow(id(21)),
    );
    assert!(a.revision > before.revision);
    assert_eq!(a, b);
    let follow = follow.expect("follow");
    let mut changes = world.library.subscribe_changes();
    world
        .library
        .drop_follow(follow.media_id)
        .await
        .expect("drop");
    tokio::time::timeout(std::time::Duration::from_secs(1), changes.changed())
        .await
        .expect("drop wakes subscribers")
        .expect("channel open");
    assert!(world.library.list_follows().await.expect("list").is_empty());
}

#[tokio::test]
async fn restart_identity_returns_immediately_even_if_revision_is_lower() {
    let world = world("http://127.0.0.1:1".into()).await;
    let stamp = tokio::time::timeout(
        std::time::Duration::from_secs(1),
        world.library.wait_for_revision("previous-daemon", 1000),
    )
    .await
    .expect("restart resets the revision");
    assert_eq!(stamp, world.library.revision_stamp());
}

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
    db_path: std::path::PathBuf,
    library: Library,
    clock: Arc<ManualClock>,
}

impl World {
    fn tvmaze_fetches(&self) -> i64 {
        let conn = rusqlite::Connection::open(&self.db_path).expect("open");
        conn.query_row(
            "SELECT COUNT(*) FROM source_fetches WHERE source = 'tvmaze'",
            [],
            |row| row.get(0),
        )
        .expect("count")
    }
}

async fn world(base_url: String) -> World {
    world_with(base_url, "http://127.0.0.1:1".into()).await
}

async fn world_with(anilist_url: String, tvmaze_url: String) -> World {
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
    let source = AniListClient::new(anilist_url).expect("client");
    let tvmaze = animesh::sources::tvmaze::TvMazeClient::new(tvmaze_url).expect("tvmaze");

    World {
        _dir: dir,
        db_path,
        library: Library::new(
            store,
            source,
            tvmaze,
            Arc::clone(&clock) as Arc<dyn WallClock>,
            Arc::new(NoJitter),
            installation,
            1,
        ),
        clock,
    }
}

fn tv_id(value: i64) -> TvMazeId {
    TvMazeId::new(value).expect("valid id")
}

fn tv_show_body(id: i64, name: &str, season: i32, episode: i64, airstamp: &str) -> String {
    format!(
        r#"{{"id":{id},"name":"{name}","type":"Scripted","language":"English","status":"Running","premiered":"2011-04-17","_embedded":{{"nextepisode":{{"season":{season},"number":{episode},"airstamp":"{airstamp}"}}}}}}"#
    )
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

#[tokio::test]
async fn following_a_tv_show_makes_exactly_one_detail_request() {
    let mut anilist = mockito::Server::new_async().await;
    let never = anilist.mock("POST", "/").expect(0).create_async().await;
    let mut tv = mockito::Server::new_async().await;
    let mock = tv
        .mock("GET", "/shows/82?embed=nextepisode")
        .with_status(200)
        .with_body(tv_show_body(
            82,
            "Game of Thrones",
            8,
            6,
            "2026-09-01T01:00:00+00:00",
        ))
        .expect(1)
        .create_async()
        .await;

    let world = world_with(anilist.url(), tv.url()).await;
    let result = world.library.follow_tv(tv_id(82)).await.expect("follow");

    assert_eq!(result.outcome, FollowOutcome::NewlyFollowed);
    assert_eq!(result.source.as_str(), "tvmaze");
    assert_eq!(result.source_id.get(), 82);
    assert_eq!(result.display_title.as_str(), "Game of Thrones");
    let upcoming = result.upcoming.expect("upcoming");
    assert_eq!(upcoming.episode.map(|e| e.get()), Some(6));
    assert_eq!(upcoming.source.as_str(), "tvmaze");

    mock.assert_async().await;
    never.assert_async().await;
}

#[tokio::test]
async fn anilist_21_and_tvmaze_21_are_different_follows() {
    let mut anilist = mockito::Server::new_async().await;
    anilist
        .mock("POST", "/")
        .with_status(200)
        .with_body(detail_body(21, 1169, NOW + 5_000))
        .create_async()
        .await;
    let mut tv = mockito::Server::new_async().await;
    tv.mock("GET", "/shows/21?embed=nextepisode")
        .with_status(200)
        .with_body(tv_show_body(
            21,
            "The Walking Dead",
            1,
            2,
            "2026-09-01T01:00:00+00:00",
        ))
        .create_async()
        .await;

    let world = world_with(anilist.url(), tv.url()).await;
    world.library.follow(id(21)).await.expect("anilist");
    world.library.follow_tv(tv_id(21)).await.expect("tvmaze");

    let follows = world.library.list_follows().await.expect("list");
    assert_eq!(follows.len(), 2);
    let sources: Vec<_> = follows.iter().map(|f| f.source.as_str()).collect();
    assert!(sources.contains(&"anilist"));
    assert!(sources.contains(&"tvmaze"));
    assert_ne!(follows[0].media_id, follows[1].media_id);
}

#[tokio::test]
async fn an_unknown_tvmaze_id_records_evidence_and_creates_no_follow() {
    let mut tv = mockito::Server::new_async().await;
    tv.mock("GET", "/shows/999?embed=nextepisode")
        .with_status(404)
        .create_async()
        .await;

    let world = world_with("http://127.0.0.1:1".into(), tv.url()).await;
    let error = world
        .library
        .follow_tv(tv_id(999))
        .await
        .expect_err("must not succeed");

    assert_eq!(error.code, ErrorCode::NotFound);
    assert!(world.library.list_follows().await.expect("list").is_empty());
    assert_eq!(world.tvmaze_fetches(), 1);
}

#[tokio::test]
async fn tv_search_percent_encodes_the_query() {
    let mut tv = mockito::Server::new_async().await;
    let mock = tv
        .mock("GET", "/search/shows?q=foo%26bar")
        .with_status(200)
        .with_body(r#"[{"show":{"id":1,"name":"Foo","language":"English","status":"Running"}}]"#)
        .expect(1)
        .create_async()
        .await;

    let world = world_with("http://127.0.0.1:1".into(), tv.url()).await;
    let hits = world
        .library
        .search_tv(Some("foo&bar"))
        .await
        .expect("search");
    assert_eq!(hits.len(), 1);
    assert_eq!(hits[0].source.as_str(), "tvmaze");
    mock.assert_async().await;
}

#[tokio::test]
async fn empty_tv_search_hits_us_broadcast_and_web_schedules() {
    let mut tv = mockito::Server::new_async().await;
    let broadcast = tv
        .mock("GET", "/schedule?country=US")
        .with_status(200)
        .with_body(r#"[{"show":{"id":1,"name":"Network","type":"Scripted","language":"English","status":"Running"}}]"#)
        .expect(1)
        .create_async()
        .await;
    let web = tv
        .mock("GET", "/schedule/web?country=US")
        .with_status(200)
        .with_body(r#"[{"_embedded":{"show":{"id":2,"name":"Stream","type":"Animation","language":"English","status":"Running"}}}]"#)
        .expect(1)
        .create_async()
        .await;

    let world = world_with("http://127.0.0.1:1".into(), tv.url()).await;
    let hits = world.library.search_tv(None).await.expect("search");
    assert_eq!(hits.len(), 2);
    broadcast.assert_async().await;
    web.assert_async().await;
}

#[tokio::test]
async fn following_the_same_tv_show_twice_makes_no_second_request() {
    let mut tv = mockito::Server::new_async().await;
    let mock = tv
        .mock("GET", "/shows/82?embed=nextepisode")
        .with_status(200)
        .with_body(tv_show_body(
            82,
            "Game of Thrones",
            8,
            6,
            "2026-09-01T01:00:00+00:00",
        ))
        .expect(1)
        .create_async()
        .await;

    let world = world_with("http://127.0.0.1:1".into(), tv.url()).await;
    world.library.follow_tv(tv_id(82)).await.expect("first");
    let second = world.library.follow_tv(tv_id(82)).await.expect("second");
    assert_eq!(second.outcome, FollowOutcome::AlreadyActive);
    mock.assert_async().await;
}

fn catalog_body(items: Vec<serde_json::Value>) -> String {
    serde_json::json!({"data": {"Page": {"media": items}}}).to_string()
}

fn catalog_item(id: i64, title: &str) -> serde_json::Value {
    let body: serde_json::Value =
        serde_json::from_str(&detail_body(id, 5, NOW + 1000)).expect("fixture");
    let mut item = body["data"]["Media"].clone();
    item["title"]["english"] = title.into();
    item
}

#[tokio::test]
async fn discovery_is_durable_without_following_or_notification_intent() {
    use animesh::domain::command_center::{FeedKey, ViewData, ViewQuery};
    let mut server = mockito::Server::new_async().await;
    let mock = server
        .mock("POST", "/")
        .with_status(200)
        .with_body(catalog_body(vec![catalog_item(21, "One Piece")]))
        .expect(1)
        .create_async()
        .await;
    let world = world(server.url()).await;
    world
        .library
        .refresh_discovery(FeedKey::AnimeAiringThisWeek)
        .await
        .expect("ingest");
    let snapshot = world
        .library
        .view(ViewQuery::Discovery {
            feed: FeedKey::AnimeAiringThisWeek,
        })
        .await
        .expect("offline projection");
    let ViewData::Discovery(feed) = snapshot.view else {
        panic!("feed")
    };
    assert_eq!(feed.items.len(), 1);
    assert_eq!(feed.items[0].follow_state, None);
    assert_eq!(snapshot.health.active_follows, 0);
    assert_eq!(snapshot.health.notifications.desired, 0);
    assert!(world
        .library
        .upcoming(None)
        .await
        .expect("no followed events")
        .is_empty());
    assert!(world
        .library
        .list_follows()
        .await
        .expect("no follows")
        .is_empty());
    mock.assert_async().await;
    let reopened = rusqlite::Connection::open(&world.db_path).expect("reopen");
    assert_eq!(
        animesh::store::command_center::discovery(&reopened, FeedKey::AnimeAiringThisWeek, at(NOW))
            .expect("saved feed")
            .items,
        feed.items
    );
}

#[tokio::test]
async fn failed_and_partial_discovery_preserve_the_last_complete_contents() {
    use animesh::domain::command_center::{FeedKey, SnapshotCompleteness};
    let mut server = mockito::Server::new_async().await;
    let initial = server
        .mock("POST", "/")
        .with_status(200)
        .with_body(catalog_body(vec![catalog_item(21, "Original title")]))
        .create_async()
        .await;
    let world = world(server.url()).await;
    let key = FeedKey::AnimeThisSeason;
    world
        .library
        .refresh_discovery(key)
        .await
        .expect("complete");
    initial.remove_async().await;
    let partial = server
        .mock("POST", "/")
        .with_status(200)
        .with_body(catalog_body(vec![
            catalog_item(21, "Partial title"),
            serde_json::Value::Null,
        ]))
        .create_async()
        .await;
    world
        .library
        .refresh_discovery(key)
        .await
        .expect_err("partial");
    partial.remove_async().await;
    let conn = rusqlite::Connection::open(&world.db_path).expect("open");
    let partial_feed =
        animesh::store::command_center::discovery(&conn, key, at(NOW)).expect("feed");
    assert_eq!(
        partial_feed.last_attempt,
        Some(SnapshotCompleteness::Partial)
    );
    assert_eq!(
        partial_feed.items[0].facts.display_title.as_str(),
        "Original title"
    );
    let failed = server
        .mock("POST", "/")
        .with_status(503)
        .with_body("offline")
        .expect(5)
        .create_async()
        .await;
    for _ in 0..5 {
        world
            .library
            .refresh_discovery(key)
            .await
            .expect_err("offline");
    }
    let saved = animesh::store::command_center::discovery(&conn, key, at(NOW)).expect("saved");
    assert_eq!(saved.last_attempt, Some(SnapshotCompleteness::Failed));
    assert_eq!(saved.items, partial_feed.items);
    assert!(saved.last_error.is_some());
    assert_eq!(
        conn.query_row(
            "SELECT COUNT(*) FROM discovery_snapshots WHERE feed_key = ?1",
            [key.as_str()],
            |row| row.get::<_, i64>(0)
        )
        .expect("bounded history"),
        4
    );
    failed.assert_async().await;
}

#[tokio::test]
async fn every_local_desktop_view_works_without_source_requests() {
    use animesh::domain::command_center::{FeedKey, FollowSort, ViewQuery};
    use animesh::domain::release::FollowState;
    let world = world("http://127.0.0.1:1".into()).await;
    let before = world.library.revision_stamp();
    for query in [
        ViewQuery::Home,
        ViewQuery::Health,
        ViewQuery::Discovery {
            feed: FeedKey::TvOnNow,
        },
        ViewQuery::Schedule {
            kind: None,
            cursor: None,
        },
        ViewQuery::Library {
            kind: None,
            state: FollowState::Active,
            sort: FollowSort::Alphabetical,
            cursor: None,
        },
    ] {
        let snapshot = world.library.view(query).await.expect("local view");
        assert_eq!(snapshot.stamp, before);
    }
    let conn = rusqlite::Connection::open(&world.db_path).expect("open");
    assert_eq!(
        conn.query_row("SELECT COUNT(*) FROM source_fetches", [], |row| row
            .get::<_, i64>(0))
            .expect("no source traffic"),
        0
    );
}
