//! Regression coverage for hostile source data and transport boundary failures.
#![allow(clippy::expect_used, clippy::unwrap_used)]
use animesh::domain::ids::{
    AniListId, BoundedText, MediaId, Source, SourceNumericId, TvMazeId, UnixTimestamp,
};
use animesh::domain::read_models::{FollowSummary, Freshness};
use animesh::domain::release::FollowState;
use animesh::domain::time::{ManualClock, NoJitter};
use animesh::library::service::Library;
use animesh::sources::anilist::{client::AniListClient, decode_search, SearchDecode};
use animesh::sources::tvmaze::TvMazeClient;
use animesh::store::{connection::Store, graph, migrations};
use std::sync::Arc;
const NOW: i64 = 1_700_000_000;
fn at(n: i64) -> UnixTimestamp {
    UnixTimestamp::new(n).unwrap()
}
fn detail(title: &str) -> String {
    serde_json::json!({"data":{"Media":{"id":21,"title":{"english":title},"status":"RELEASING","nextAiringEpisode":{"episode":2,"airingAt":NOW+5000}}}}).to_string()
}
struct World {
    _dir: tempfile::TempDir,
    db: std::path::PathBuf,
    library: Library,
    clock: Arc<ManualClock>,
}
async fn world(anilist: String, tvmaze: String) -> World {
    let dir = tempfile::tempdir().unwrap();
    let paths = animesh::paths::AppPaths::under_root(dir.path());
    paths.ensure_dirs().unwrap();
    let db = paths.database();
    let store = Store::open(&db).unwrap();
    store
        .writer()
        .call(|c| migrations::apply(c, NOW))
        .await
        .unwrap();
    let installation = store
        .write(|tx| graph::ensure_installation(tx, at(NOW)))
        .await
        .unwrap();
    let clock = Arc::new(ManualClock::new(at(NOW)));
    let library = Library::new(
        store,
        AniListClient::new(anilist).unwrap(),
        TvMazeClient::new(tvmaze).unwrap(),
        clock.clone(),
        Arc::new(NoJitter),
        installation,
        migrations::supported_version(),
    );
    World {
        _dir: dir,
        db,
        library,
        clock,
    }
}
#[test]
fn source_titles_escape_terminal_control_sequences() {
    let title = "Example\u{1b}]52;c;YXVkaXQ=\u{7}\nForged status: safe";
    let body = serde_json::json!({"data":{"Page":{"media":[{"id":21,"title":{"english":title}}]}}})
        .to_string();
    let SearchDecode::Candidates(items) = decode_search(&body) else {
        panic!("decode")
    };
    let rendered = animesh::cli::render::search(&items);
    assert!(!rendered.contains('\u{1b}'));
    assert!(!rendered.contains("\nForged"));
    assert!(rendered.contains("\\u{1b}"));
}
#[tokio::test]
async fn enormous_retry_after_is_bounded_and_retry_resumes() {
    let mut server = mockito::Server::new_async().await;
    let mock = server
        .mock("POST", "/")
        .expect(2)
        .with_status(429)
        .with_header("retry-after", "4294967295")
        .with_body("slow down")
        .create_async()
        .await;
    let w = world(server.url(), "http://127.0.0.1:1".into()).await;
    assert!(w.library.search("Example").await.is_err());
    let conn = rusqlite::Connection::open(&w.db).unwrap();
    let until = graph::source_blocked_until(&conn, Source::AniList)
        .unwrap()
        .unwrap();
    assert_eq!(until.get() - NOW, 86400);
    w.clock.set(at(NOW + 86401));
    assert!(w.library.search("Example").await.is_err());
    mock.assert_async().await;
}
#[tokio::test]
async fn anilist_http_500_cannot_commit_a_follow_from_its_body() {
    let mut server = mockito::Server::new_async().await;
    server
        .mock("POST", "/")
        .with_status(500)
        .with_body(detail("Unsuccessful response"))
        .create_async()
        .await;
    let w = world(server.url(), "http://127.0.0.1:1".into()).await;
    assert!(w.library.follow(AniListId::new(21).unwrap()).await.is_err());
    assert!(w.library.list_follows().await.unwrap().is_empty());
}
#[tokio::test]
async fn tvmaze_http_500_cannot_commit_a_follow_from_its_body() {
    let mut server = mockito::Server::new_async().await;
    server
        .mock("GET", "/shows/82?embed=nextepisode")
        .with_status(500)
        .with_body(r#"{"id":82,"name":"Unsuccessful response","status":"Running"}"#)
        .create_async()
        .await;
    let w = world("http://127.0.0.1:1".into(), server.url()).await;
    assert!(w
        .library
        .follow_tv(TvMazeId::new(82).unwrap())
        .await
        .is_err());
}
#[tokio::test]
async fn transport_refuses_a_redirect_to_a_different_origin() {
    let mut first = mockito::Server::new_async().await;
    let mut second = mockito::Server::new_async().await;
    first
        .mock("GET", "/shows/82")
        .with_status(302)
        .with_header("location", &format!("{}/redirected", second.url()))
        .create_async()
        .await;
    let target = second
        .mock("GET", "/redirected")
        .expect(0)
        .with_status(200)
        .with_body(r#"{"id":82,"name":"Redirected"}"#)
        .create_async()
        .await;
    let response = TvMazeClient::new(first.url())
        .unwrap()
        .get("/shows/82", at(NOW))
        .await;
    assert_eq!(response.http_status, Some(302));
    target.assert_async().await;
}
#[tokio::test]
async fn dropping_is_distinct_from_explicit_history_erasure() {
    let mut server = mockito::Server::new_async().await;
    let mut body: serde_json::Value = serde_json::from_str(&detail("Example")).unwrap();
    body["unused"] = serde_json::Value::String("x".repeat(1024 * 1024));
    server
        .mock("POST", "/")
        .with_status(200)
        .with_body(body.to_string())
        .create_async()
        .await;
    let w = world(server.url(), "http://127.0.0.1:1".into()).await;
    let followed = w.library.follow(AniListId::new(21).unwrap()).await.unwrap();
    w.library.drop_follow(followed.media_id).await.unwrap();
    let conn = rusqlite::Connection::open(&w.db).unwrap();
    let stored: i64 = conn
        .query_row(
            "SELECT sum(length(body_json)) FROM source_fetches",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert!(stored > 1024 * 1024);
    let history: i64 = conn
        .query_row(
            "SELECT count(*) FROM follows WHERE state='dropped'",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(history, 1);
}
#[tokio::test]
async fn legacy_oversized_replies_return_a_structured_error() {
    let title = BoundedText::new("title", 512, "\u{1f680}".repeat(512)).unwrap();
    let rows = (1..=500)
        .map(|i| FollowSummary {
            media_id: MediaId::new(i).unwrap(),
            source: Source::AniList,
            source_id: SourceNumericId::new(i).unwrap(),
            display_title: title.clone(),
            state: FollowState::Active,
            upcoming: None,
            last_success_at: None,
            freshness: Freshness::Fresh,
        })
        .collect();
    let reply = animesh::ipc::protocol::ReplyEnvelope::ok(
        "request",
        "daemon",
        animesh::application::commands::Response::ListFollows(rows),
    );
    let bytes = serde_json::to_vec(&reply).unwrap();
    assert!(bytes.len() > animesh::ipc::protocol::MAX_FRAME_BYTES);
    use animesh::ipc::{client, endpoint::Endpoint, protocol::Request};
    let dir = tempfile::tempdir().unwrap();
    let paths = animesh::paths::AppPaths::under_root(dir.path());
    let endpoint = Endpoint::bind(&paths).unwrap();
    let response = match reply.result {
        animesh::ipc::protocol::ReplyResult::Ok(response) => response,
        _ => panic!("expected a successful response fixture"),
    };
    let (shutdown, rx) = tokio::sync::watch::channel(false);
    let serving = tokio::spawn(async move {
        animesh::ipc::server::serve(
            &endpoint,
            Arc::from("daemon"),
            move |_| {
                let response = response.clone();
                async move { Ok(response) }
            },
            rx,
        )
        .await;
    });
    let error = client::send(&paths, Request::ListFollows)
        .await
        .unwrap_err();
    assert_eq!(error.code, animesh::error::ErrorCode::InvalidArgument);
    assert!(error.message.contains("paginated"));
    shutdown.send(true).unwrap();
    serving.await.unwrap();
}

#[test]
fn evidence_budget_and_age_preserve_referential_integrity() {
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("library.db");
    let mut conn = animesh::store::connection::open(&path, true).unwrap();
    migrations::apply(&mut conn, NOW).unwrap();
    let body = "x".repeat(1024 * 1024);
    for i in 0..40 {
        let tx = conn.transaction().unwrap();
        graph::insert_fetch(
            &tx,
            &graph::FetchRecord {
                attempt_uuid: &format!("a{i}"),
                source: Source::AniList,
                request_kind: "detail",
                request_fingerprint: "id=21",
                requested_at: at(NOW + i),
                completed_at: at(NOW + i),
                outcome: "success",
                http_status: Some(200),
                retry_after: None,
                rate_limit_remaining: None,
                rate_limit_reset_at: None,
                body_json: Some(&body),
                error_code: None,
            },
        )
        .unwrap();
        tx.commit().unwrap();
    }
    let bytes: i64 = conn
        .query_row(
            "SELECT SUM(COALESCE(byte_length,0)) FROM source_fetches",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert!(bytes <= animesh::store::maintenance::EVIDENCE_BUDGET);
    let tx = conn.transaction().unwrap();
    animesh::store::maintenance::prune(&tx, NOW + 40 * 86400).unwrap();
    tx.commit().unwrap();
    let count: i64 = conn
        .query_row(
            "SELECT count(*) FROM source_fetches WHERE body_json IS NOT NULL",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(count, 0);
    animesh::store::connection::verify_integrity(&conn).unwrap();
}

#[test]
fn backup_restores_a_verified_snapshot_even_if_original_is_corrupt() {
    use animesh::store::recovery;
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("library.db");
    let mut conn = animesh::store::connection::open(&path, true).unwrap();
    migrations::apply(&mut conn, NOW).unwrap();
    let tx = conn.transaction().unwrap();
    graph::create_media(
        &tx,
        animesh::domain::ids::SourceKey::anilist(AniListId::new(21).unwrap()),
        &BoundedText::new("title", 512, "Snapshot title").unwrap(),
        at(NOW),
    )
    .unwrap();
    tx.commit().unwrap();
    recovery::snapshot(&conn, &path, "manual").unwrap();
    drop(conn);
    std::fs::write(&path, b"corrupt original").unwrap();
    recovery::restore(&path, "manual").unwrap();
    let restored = animesh::store::connection::open(&path, false).unwrap();
    let title: String = restored
        .query_row("SELECT display_title FROM media", [], |r| r.get(0))
        .unwrap();
    assert_eq!(title, "Snapshot title");
    assert_eq!(
        std::fs::read(temp.path().join("recovery-original/library.db")).unwrap(),
        b"corrupt original"
    );
    assert_eq!(
        std::fs::metadata(temp.path().join("backups/manual.db"))
            .unwrap()
            .permissions()
            .mode()
            & 0o777,
        0o600
    );
}

use std::os::unix::fs::PermissionsExt;
#[test]
fn corrupt_backup_is_rejected_without_changing_original() {
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("library.db");
    let mut conn = animesh::store::connection::open(&path, true).unwrap();
    migrations::apply(&mut conn, NOW).unwrap();
    animesh::store::recovery::snapshot(&conn, &path, "manual").unwrap();
    drop(conn);
    let before = std::fs::read(&path).unwrap();
    std::fs::write(temp.path().join("backups/manual.db"), b"corrupt backup").unwrap();
    assert!(animesh::store::recovery::restore(&path, "manual").is_err());
    assert_eq!(std::fs::read(&path).unwrap(), before);
}

#[test]
fn filesystem_links_cannot_redirect_private_writes_or_chmod() {
    use animesh::paths;
    let temp = tempfile::tempdir().unwrap();
    let target = temp.path().join("victim");
    std::fs::create_dir(&target).unwrap();
    std::fs::set_permissions(&target, std::fs::Permissions::from_mode(0o755)).unwrap();
    let link = temp.path().join("linked-dir");
    std::os::unix::fs::symlink(&target, &link).unwrap();
    assert!(paths::private_dir(&link).is_err());
    assert_eq!(
        std::fs::metadata(&target).unwrap().permissions().mode() & 0o777,
        0o755
    );
    let file = target.join("private");
    std::fs::write(&file, "untouched").unwrap();
    let link = temp.path().join("linked-file");
    std::os::unix::fs::symlink(&file, &link).unwrap();
    assert!(paths::atomic_write(&link, b"changed").is_err());
    assert!(animesh::ipc::endpoint::SingletonLock::acquire(&link).is_err());
    assert!(animesh::store::connection::open(&link, true).is_err());
    assert_eq!(std::fs::read_to_string(&file).unwrap(), "untouched");
}

#[tokio::test]
async fn maximum_length_titles_are_read_without_loss_through_pages() {
    use animesh::domain::{
        command_center::{ReadKind, ReadRows},
        ids::SourceKey,
    };
    let w = world("http://127.0.0.1:1".into(), "http://127.0.0.1:1".into()).await;
    let mut conn = animesh::store::connection::open(&w.db, true).unwrap();
    let tx = conn.transaction().unwrap();
    for i in 1..=501 {
        let row = graph::create_media(
            &tx,
            SourceKey::anilist(AniListId::new(i).unwrap()),
            &BoundedText::new("title", 512, "🚀".repeat(512)).unwrap(),
            at(NOW),
        )
        .unwrap();
        graph::set_follow(&tx, row.media_id, FollowState::Active, at(NOW)).unwrap();
    }
    tx.commit().unwrap();
    let mut cursor = None;
    let mut ids = Vec::new();
    loop {
        let page = w
            .library
            .read_page(ReadKind::Library, cursor)
            .await
            .unwrap();
        assert!(serde_json::to_vec(&page).unwrap().len() < animesh::ipc::protocol::MAX_FRAME_BYTES);
        let ReadRows::Library(rows) = page.rows else {
            panic!("wrong page")
        };
        assert!(rows.len() <= 50);
        ids.extend(rows.into_iter().map(|row| row.media_id.get()));
        cursor = page.next;
        if cursor.is_none() {
            break;
        }
    }
    ids.sort();
    ids.dedup();
    assert_eq!(ids.len(), 501);
}

#[test]
fn forgetting_a_title_erases_raw_batches_and_preserves_other_follows() {
    use animesh::domain::ids::SourceKey;
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("library.db");
    let mut conn = animesh::store::connection::open(&path, true).unwrap();
    migrations::apply(&mut conn, NOW).unwrap();
    let tx = conn.transaction().unwrap();
    let body = r#"{"titles":["Title 1","Title 2"]}"#;
    let fetch = graph::insert_fetch(
        &tx,
        &graph::FetchRecord {
            attempt_uuid: "shared-batch",
            source: Source::AniList,
            request_kind: "batch",
            request_fingerprint: "id_in=2",
            requested_at: at(NOW),
            completed_at: at(NOW),
            outcome: "success",
            http_status: Some(200),
            retry_after: None,
            rate_limit_remaining: None,
            rate_limit_reset_at: None,
            body_json: Some(body),
            error_code: None,
        },
    )
    .unwrap();
    for i in 1..=2 {
        let row = graph::create_media(
            &tx,
            SourceKey::anilist(AniListId::new(i).unwrap()),
            &BoundedText::new("title", 512, format!("Title {i}")).unwrap(),
            at(NOW),
        )
        .unwrap();
        let observation = animesh::domain::media::MediaObservation {
            source_key: row.source_key,
            display_title: BoundedText::new("title", 512, format!("Title {i}")).unwrap(),
            titles: Default::default(),
            status: animesh::domain::media::MediaStatus::Releasing,
            status_raw: None,
            format_raw: None,
            episode_count: None,
            season_year: None,
            next_airing: None,
            parser_version: 1,
        };
        let obs = graph::insert_observation(&tx, row.source_media_id, fetch, &observation, at(NOW))
            .unwrap();
        graph::set_current_observation(&tx, row.source_media_id, obs, at(NOW)).unwrap();
        graph::set_follow(&tx, row.media_id, FollowState::Active, at(NOW)).unwrap();
    }
    assert!(animesh::store::maintenance::forget(&tx, 1).unwrap());
    tx.commit().unwrap();
    let left: i64 = conn
        .query_row("SELECT COUNT(*) FROM follows", [], |r| r.get(0))
        .unwrap();
    assert_eq!(left, 1);
    let title: String = conn
        .query_row("SELECT display_title FROM media", [], |r| r.get(0))
        .unwrap();
    assert_eq!(title, "Title 2");
    let bodies: i64 = conn
        .query_row(
            "SELECT count(*) FROM source_fetches WHERE body_json IS NOT NULL",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(bodies, 0);
    let observations: i64 = conn
        .query_row("SELECT count(*) FROM source_observations", [], |r| r.get(0))
        .unwrap();
    assert_eq!(observations, 1);
    animesh::store::connection::verify_integrity(&conn).unwrap();
}

// Exhaust the small state space: every subset, in either deletion order.
// Rollback must preserve all titles; committed erasure must be idempotent and
// independent of order, while retention preserves every surviving follow.
#[test]
fn forgetting_subsets_preserves_survivors_under_rollback_and_retention() {
    use animesh::domain::ids::SourceKey;
    use animesh::store::{connection, maintenance};
    for mask in 0_u32..16 {
        for reverse in [false, true] {
            let temp = tempfile::tempdir().unwrap();
            let mut conn = connection::open(&temp.path().join("library.db"), true).unwrap();
            migrations::apply(&mut conn, NOW).unwrap();
            let tx = conn.transaction().unwrap();
            for id in 1..=4 {
                let title = BoundedText::new("title", 512, format!("Title {id}")).unwrap();
                let row = graph::create_media(
                    &tx,
                    SourceKey::anilist(AniListId::new(id).unwrap()),
                    &title,
                    at(NOW),
                )
                .unwrap();
                graph::set_follow(&tx, row.media_id, FollowState::Active, at(NOW)).unwrap();
            }
            tx.commit().unwrap();
            let mut deleted: Vec<i64> = (1..=4).filter(|id| mask & (1 << (id - 1)) != 0).collect();
            if reverse {
                deleted.reverse();
            }
            {
                let tx = conn.transaction().unwrap();
                for id in &deleted {
                    assert!(maintenance::forget(&tx, *id).unwrap());
                }
                // Simulates interruption before commit.
            }
            let original: i64 = conn
                .query_row("SELECT count(*) FROM follows", [], |r| r.get(0))
                .unwrap();
            assert_eq!(original, 4, "rollback mask={mask}, reverse={reverse}");
            let tx = conn.transaction().unwrap();
            for id in &deleted {
                assert!(maintenance::forget(&tx, *id).unwrap());
                assert!(!maintenance::forget(&tx, *id).unwrap());
            }
            maintenance::prune(&tx, NOW + 100 * 86400).unwrap();
            maintenance::prune(&tx, NOW + 100 * 86400).unwrap();
            tx.commit().unwrap();
            let mut statement = conn
                .prepare("SELECT m.media_id, m.display_title FROM media m JOIN follows f USING(media_id) ORDER BY m.media_id")
                .unwrap();
            let survivors: Vec<(i64, String)> = statement
                .query_map([], |r| Ok((r.get(0)?, r.get(1)?)))
                .unwrap()
                .collect::<Result<_, _>>()
                .unwrap();
            let expected: Vec<_> = (1..=4)
                .filter(|id| !deleted.contains(id))
                .map(|id| (id, format!("Title {id}")))
                .collect();
            assert_eq!(survivors, expected, "mask={mask}, reverse={reverse}");
            connection::verify_integrity(&conn).unwrap();
        }
    }
}

#[test]
fn bounded_text_is_validated_when_received_over_ipc() {
    assert!(
        serde_json::from_str::<BoundedText>(&serde_json::to_string(&"x".repeat(513)).unwrap())
            .is_err()
    );
    assert!(serde_json::from_str::<BoundedText>("\"\"").is_err());
}

#[tokio::test]
async fn unsuccessful_batch_preserves_current_facts() {
    let mut server = mockito::Server::new_async().await;
    server
        .mock("POST", "/")
        .with_status(200)
        .with_body(detail("Original title"))
        .create_async()
        .await;
    let w = world(server.url(), "http://127.0.0.1:1".into()).await;
    w.library.follow(AniListId::new(21).unwrap()).await.unwrap();
    server.reset();
    server.mock("POST","/").with_status(500).with_body(serde_json::json!({"data":{"Page":{"media":[{"id":21,"title":{"english":"Failed HTTP replacement"},"status":"FINISHED","nextAiringEpisode":null}]}}}).to_string()).create_async().await;
    w.clock.set(at(NOW + 20000));
    let pass = w.library.refresh_due(25).await.unwrap();
    assert_eq!(pass.failed, 1);
    let conn = rusqlite::Connection::open(&w.db).unwrap();
    let title: String = conn
        .query_row("SELECT display_title FROM media", [], |r| r.get(0))
        .unwrap();
    assert_eq!(title, "Original title");
    let outcome: String = conn
        .query_row(
            "SELECT outcome FROM source_fetches ORDER BY fetch_id DESC LIMIT 1",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(outcome, "http_error");
}

#[test]
fn storage_full_rolls_back_a_write_without_losing_existing_library() {
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("library.db");
    let mut conn = animesh::store::connection::open(&path, true).unwrap();
    migrations::apply(&mut conn, NOW).unwrap();
    let used: i64 = conn
        .query_row("PRAGMA page_count", [], |r| r.get(0))
        .unwrap();
    conn.pragma_update(None, "max_page_count", used + 4)
        .unwrap();
    let before: i64 = conn
        .query_row("SELECT count(*) FROM source_fetches", [], |r| r.get(0))
        .unwrap();
    {
        let tx = conn.transaction().unwrap();
        let result=tx.execute("INSERT INTO source_fetches (attempt_uuid,source,request_kind,request_fingerprint,requested_at,completed_at,outcome,body_json,byte_length) VALUES ('full','anilist','detail','id=21',1,1,'success',?1,?2)",rusqlite::params!["x".repeat(1024*1024),1024*1024]);
        assert!(result.is_err());
    }
    let after: i64 = conn
        .query_row("SELECT count(*) FROM source_fetches", [], |r| r.get(0))
        .unwrap();
    assert_eq!(before, after);
    animesh::store::connection::verify_integrity(&conn).unwrap();
}

#[tokio::test]
async fn backup_failure_is_visible_without_disabling_local_reads() {
    let w = world("http://127.0.0.1:1".into(), "http://127.0.0.1:1".into()).await;
    let directory = w.db.parent().unwrap().join("backups");
    std::fs::create_dir(&directory).unwrap();
    let victim = w.db.parent().unwrap().join("not-a-backup");
    std::fs::write(&victim, "untouched").unwrap();
    std::os::unix::fs::symlink(&victim, directory.join("daily.db")).unwrap();
    assert!(w.library.maintain_storage().await.is_err());
    let health = w.library.health().await.unwrap();
    assert!(health
        .degraded
        .contains(&animesh::domain::read_models::DegradedReason::BackupUnavailable));
    assert!(w.library.list_follows().await.unwrap().is_empty());
    assert_eq!(std::fs::read_to_string(victim).unwrap(), "untouched");
}
