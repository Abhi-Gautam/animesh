//! V2 — the refresh pass.
//!
//! The gates from plan section 24: nothing due means no request and no write,
//! failures preserve serving data, an omitted item preserves its projection,
//! and an unchanged post-air response cannot hot-loop.

#![allow(clippy::expect_used)]

use std::sync::Arc;

use animesh::domain::ids::{AniListId, UnixTimestamp};
use animesh::domain::read_models::Freshness;
use animesh::domain::time::{ManualClock, NoJitter, WallClock};
use animesh::library::service::Library;
use animesh::store::connection::Store;
use animesh::store::migrations;

const NOW: i64 = 1_700_000_000;

fn at(seconds: i64) -> UnixTimestamp {
    UnixTimestamp::new(seconds).expect("valid timestamp")
}

fn id(value: i64) -> AniListId {
    AniListId::new(value).expect("valid id")
}

fn media_json(anilist_id: i64, title: &str, next: Option<(i64, i64)>) -> String {
    let airing = next.map_or_else(
        || "null".to_owned(),
        |(episode, at)| format!(r#"{{"episode":{episode},"airingAt":{at}}}"#),
    );
    format!(
        r#"{{"id":{anilist_id},"title":{{"romaji":"{title}","english":"{title}","native":null}},"status":"RELEASING","episodes":null,"format":"TV","seasonYear":1999,"nextAiringEpisode":{airing}}}"#
    )
}

fn detail_body(anilist_id: i64, title: &str, next: Option<(i64, i64)>) -> String {
    format!(
        r#"{{"data":{{"Media":{}}}}}"#,
        media_json(anilist_id, title, next)
    )
}

fn batch_body(items: &[String]) -> String {
    format!(r#"{{"data":{{"Page":{{"media":[{}]}}}}}}"#, items.join(","))
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
    World {
        _dir: dir,
        library: Library::new(
            store,
            animesh::sources::anilist::client::AniListClient::new(base_url).expect("client"),
            Arc::clone(&clock) as Arc<dyn WallClock>,
            Arc::new(NoJitter),
            installation,
            1,
        ),
        clock,
    }
}

#[tokio::test]
async fn nothing_due_makes_no_request_and_no_write() {
    // The idle gate from section 22: zero network, zero writes.
    let mut server = mockito::Server::new_async().await;
    let never = server.mock("POST", "/").expect(0).create_async().await;

    let world = world(server.url()).await;
    let pass = world.library.refresh_due(10).await.expect("refresh");

    assert!(!pass.did_work());
    assert_eq!(pass.applied, 0);
    never.assert_async().await;
}

#[tokio::test]
async fn a_freshly_followed_title_is_not_immediately_due_again() {
    // Following sets a cadence; refreshing right after would be a hot loop.
    let mut server = mockito::Server::new_async().await;
    server
        .mock("POST", "/")
        .with_status(200)
        .with_body(detail_body(21, "One Piece", Some((5, NOW + 500_000))))
        .create_async()
        .await;

    let world = world(server.url()).await;
    world.library.follow(id(21)).await.expect("follow");

    let pass = world.library.refresh_due(10).await.expect("refresh");
    assert!(!pass.did_work(), "a just-followed title was due again");
}

#[tokio::test]
async fn a_due_title_refreshes_and_advances_its_episode() {
    let mut server = mockito::Server::new_async().await;
    let follow_mock = server
        .mock("POST", "/")
        .with_status(200)
        .with_body(detail_body(21, "One Piece", Some((5, NOW + 3_600))))
        .expect(1)
        .create_async()
        .await;

    let world = world(server.url()).await;
    world.library.follow(id(21)).await.expect("follow");
    follow_mock.assert_async().await;

    // Move past the airtime; AniList now reports the next episode.
    world.clock.advance(7_200);
    let later = NOW + 7_200;
    server
        .mock("POST", "/")
        .with_status(200)
        .with_body(batch_body(&[media_json(
            21,
            "One Piece",
            Some((6, later + 604_800)),
        )]))
        .create_async()
        .await;

    let pass = world.library.refresh_due(10).await.expect("refresh");
    assert_eq!(pass.applied, 1, "{pass:?}");

    let upcoming = world.library.upcoming(None).await.expect("upcoming");
    assert_eq!(
        upcoming.len(),
        1,
        "the advanced episode should be scheduled"
    );
    assert_eq!(upcoming[0].episode.map(|e| e.get()), Some(6));
    assert_eq!(upcoming[0].freshness, Freshness::Fresh);
}

#[tokio::test]
async fn an_explicit_null_withdraws_the_scheduled_event() {
    let mut server = mockito::Server::new_async().await;
    server
        .mock("POST", "/")
        .with_status(200)
        .with_body(detail_body(21, "One Piece", Some((5, NOW + 3_600))))
        .create_async()
        .await;

    let world = world(server.url()).await;
    world.library.follow(id(21)).await.expect("follow");
    assert_eq!(
        world.library.upcoming(None).await.expect("upcoming").len(),
        1
    );

    world.clock.advance(1_000);
    server
        .mock("POST", "/")
        .with_status(200)
        .with_body(batch_body(&[media_json(21, "One Piece", None)]))
        .create_async()
        .await;

    world.library.refresh_due(10).await.expect("refresh");

    // Still in the future when withdrawn, so the row disappears from serving.
    assert!(world
        .library
        .upcoming(None)
        .await
        .expect("upcoming")
        .is_empty());
}

#[tokio::test]
async fn an_omitted_item_preserves_its_projection() {
    // The distinction the outcome matrix turns on: omission is not a null.
    let mut server = mockito::Server::new_async().await;
    server
        .mock("POST", "/")
        .with_status(200)
        .with_body(detail_body(21, "One Piece", Some((5, NOW + 3_600))))
        .create_async()
        .await;

    let world = world(server.url()).await;
    world.library.follow(id(21)).await.expect("follow");

    world.clock.advance(1_000);
    // A page that simply does not contain the requested id.
    server
        .mock("POST", "/")
        .with_status(200)
        .with_body(batch_body(&[]))
        .create_async()
        .await;

    let pass = world.library.refresh_due(10).await.expect("refresh");
    assert_eq!(pass.failed, 1);
    assert_eq!(pass.applied, 0);

    let upcoming = world.library.upcoming(None).await.expect("upcoming");
    assert_eq!(upcoming.len(), 1, "omission must not withdraw the event");
    assert_eq!(upcoming[0].episode.map(|e| e.get()), Some(5));
}

#[tokio::test]
async fn a_source_failure_preserves_serving_data_and_backs_off() {
    let mut server = mockito::Server::new_async().await;
    server
        .mock("POST", "/")
        .with_status(200)
        .with_body(detail_body(21, "One Piece", Some((5, NOW + 3_600))))
        .create_async()
        .await;

    let world = world(server.url()).await;
    world.library.follow(id(21)).await.expect("follow");

    world.clock.advance(1_000);
    server
        .mock("POST", "/")
        .with_status(503)
        .with_body("upstream down")
        .create_async()
        .await;

    let pass = world.library.refresh_due(10).await.expect("refresh");
    assert_eq!(pass.failed, 1);

    let upcoming = world.library.upcoming(None).await.expect("upcoming");
    assert_eq!(upcoming.len(), 1, "a failure must not lose the schedule");
    assert_eq!(upcoming[0].freshness, Freshness::BackingOff);

    // Backoff means it is not immediately due again.
    let second = world.library.refresh_due(10).await.expect("refresh");
    assert!(!second.did_work(), "backoff did not hold: {second:?}");
}

#[tokio::test]
async fn a_throttled_source_skips_the_pass_without_requesting() {
    let mut server = mockito::Server::new_async().await;
    server
        .mock("POST", "/")
        .with_status(200)
        .with_body(detail_body(21, "One Piece", Some((5, NOW + 3_600))))
        .create_async()
        .await;

    let world = world(server.url()).await;
    world.library.follow(id(21)).await.expect("follow");
    world.clock.advance(1_000);

    // A 429 on the next call installs the durable block.
    server
        .mock("POST", "/")
        .with_status(429)
        .with_header("retry-after", "900")
        .with_body("rate limited")
        .create_async()
        .await;
    world.library.refresh_due(10).await.expect("first pass");

    let blocked = world.library.refresh_due(10).await.expect("second pass");
    assert!(blocked.throttled, "the pass should have been skipped");
    assert!(!blocked.did_work());
}

#[tokio::test]
async fn a_dropped_follow_is_not_refreshed() {
    let mut server = mockito::Server::new_async().await;
    server
        .mock("POST", "/")
        .with_status(200)
        .with_body(detail_body(21, "One Piece", Some((5, NOW + 3_600))))
        .create_async()
        .await;

    let world = world(server.url()).await;
    let result = world.library.follow(id(21)).await.expect("follow");
    world
        .library
        .drop_follow(result.media_id)
        .await
        .expect("drop");

    world.clock.advance(100_000);
    let pass = world.library.refresh_due(10).await.expect("refresh");
    assert!(!pass.did_work(), "a dropped follow was refreshed");
}

#[tokio::test]
async fn the_earliest_due_deadline_drives_the_scheduler() {
    let mut server = mockito::Server::new_async().await;
    server
        .mock("POST", "/")
        .with_status(200)
        .with_body(detail_body(21, "One Piece", Some((5, NOW + 500_000))))
        .create_async()
        .await;

    let world = world(server.url()).await;
    assert_eq!(
        world.library.earliest_due().await.expect("due"),
        None,
        "an empty library has nothing to wake for"
    );

    world.library.follow(id(21)).await.expect("follow");
    let due = world
        .library
        .earliest_due()
        .await
        .expect("due")
        .expect("some");
    assert!(due.get() > NOW, "the next deadline must be in the future");
}
