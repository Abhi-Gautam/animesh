//! V3 — the reconciliation engine against a fake notification centre.
//!
//! The gates: a reschedule keeps the OS identifier while the request JSON and
//! revision move, a crash between OS acceptance and commit heals from OS truth,
//! cancellation removes the pending request, a settled plan makes no further OS
//! calls, and the full desired set is computed before anything is removed.

#![allow(clippy::expect_used)]

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

use animesh::domain::ids::{AniListId, UnixTimestamp};
use animesh::domain::notification::NativeRequest;
use animesh::domain::read_models::AuthorizationState;
use animesh::domain::time::{ManualClock, NoJitter, WallClock};
use animesh::engine::reconciler::{
    NotificationSurface, PendingRequest, Reconciler, SurfaceError, SurfaceFuture,
};
use animesh::library::service::Library;
use animesh::store::connection::Store;
use animesh::store::migrations;

const NOW: i64 = 1_700_000_000;
/// Far enough out that following does not immediately want a refresh.
const AIRTIME: i64 = NOW + 500_000;

fn at(seconds: i64) -> UnixTimestamp {
    UnixTimestamp::new(seconds).expect("valid timestamp")
}

fn id(value: i64) -> AniListId {
    AniListId::new(value).expect("valid id")
}

fn media_json(anilist_id: i64, title: &str, next: Option<(i64, i64)>) -> String {
    let airing = next.map_or_else(
        || "null".to_owned(),
        |(episode, airing_at)| format!(r#"{{"episode":{episode},"airingAt":{airing_at}}}"#),
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

/// Refresh reads the batch query, not the detail one.
fn batch_body(anilist_id: i64, title: &str, next: Option<(i64, i64)>) -> String {
    format!(
        r#"{{"data":{{"Page":{{"media":[{}]}}}}}}"#,
        media_json(anilist_id, title, next)
    )
}

// ---------------------------------------------------------------------------
// the fake centre
// ---------------------------------------------------------------------------

/// Every OS call in order, so a test can assert on sequencing and not just totals.
#[derive(Debug, Clone, PartialEq, Eq)]
enum Op {
    Add(String),
    Remove(Vec<String>),
}

#[derive(Default)]
struct CenterState {
    pending: Vec<PendingRequest>,
    delivered: Vec<String>,
    ops: Vec<Op>,
}

struct FakeCenter {
    state: Mutex<CenterState>,
    authorization: Mutex<AuthorizationState>,
    fail_adds: AtomicBool,
}

impl FakeCenter {
    fn new() -> Arc<Self> {
        Arc::new(Self {
            state: Mutex::new(CenterState::default()),
            authorization: Mutex::new(AuthorizationState::Authorized),
            fail_adds: AtomicBool::new(false),
        })
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, CenterState> {
        self.state.lock().expect("centre lock")
    }

    fn set_authorization(&self, state: AuthorizationState) {
        *self.authorization.lock().expect("auth lock") = state;
    }

    fn ops(&self) -> Vec<Op> {
        self.lock().ops.clone()
    }

    fn adds(&self) -> Vec<String> {
        self.lock()
            .ops
            .iter()
            .filter_map(|op| match op {
                Op::Add(id) => Some(id.clone()),
                Op::Remove(_) => None,
            })
            .collect()
    }

    fn clear_ops(&self) {
        self.lock().ops.clear();
    }

    fn pending_ids(&self) -> Vec<String> {
        self.lock()
            .pending
            .iter()
            .map(|r| r.os_identifier.clone())
            .collect()
    }

    fn pending_json(&self, identifier: &str) -> Option<String> {
        self.lock()
            .pending
            .iter()
            .find(|r| r.os_identifier == identifier)
            .and_then(|r| r.request_json.clone())
    }

    /// Stands in for macOS having accepted a request that Animesh never got to
    /// commit — the crash window.
    fn accept_behind_our_back(&self, request: &NativeRequest) {
        let json = request.canonical_json().expect("serialize");
        self.lock().pending.push(PendingRequest {
            os_identifier: request.os_identifier.as_str().to_owned(),
            request_json: Some(json),
        });
    }

    fn deliver(&self, identifier: &str) {
        let mut state = self.lock();
        state.pending.retain(|r| r.os_identifier != identifier);
        state.delivered.push(identifier.to_owned());
    }
}

fn ready<T: Send + 'static>(value: Result<T, SurfaceError>) -> SurfaceFuture<'static, T> {
    Box::pin(std::future::ready(value))
}

impl NotificationSurface for FakeCenter {
    fn authorization(&self) -> SurfaceFuture<'_, AuthorizationState> {
        ready(Ok(*self.authorization.lock().expect("auth lock")))
    }

    fn pending(&self) -> SurfaceFuture<'_, Vec<PendingRequest>> {
        ready(Ok(self.lock().pending.clone()))
    }

    fn delivered(&self) -> SurfaceFuture<'_, Vec<String>> {
        ready(Ok(self.lock().delivered.clone()))
    }

    fn add<'a>(&'a self, request: &'a NativeRequest) -> SurfaceFuture<'a, ()> {
        let identifier = request.os_identifier.as_str().to_owned();
        let json = request.canonical_json().expect("serialize");

        let mut state = self.lock();
        state.ops.push(Op::Add(identifier.clone()));

        if self.fail_adds.load(Ordering::SeqCst) {
            return ready(Err(SurfaceError::Transient("centre is busy".to_owned())));
        }

        // Adding under an existing identifier replaces it, which is the
        // behaviour D1 has to confirm on the real centre.
        state.pending.retain(|r| r.os_identifier != identifier);
        state.pending.push(PendingRequest {
            os_identifier: identifier,
            request_json: Some(json),
        });
        ready(Ok(()))
    }

    fn remove(&self, identifiers: Vec<String>) -> SurfaceFuture<'_, ()> {
        let mut state = self.lock();
        state.ops.push(Op::Remove(identifiers.clone()));
        state
            .pending
            .retain(|r| !identifiers.contains(&r.os_identifier));
        ready(Ok(()))
    }
}

// ---------------------------------------------------------------------------
// harness
// ---------------------------------------------------------------------------

struct World {
    _dir: tempfile::TempDir,
    library: Arc<Library>,
    centre: Arc<FakeCenter>,
    reconciler: Reconciler,
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
    let library = Arc::new(Library::new(
        store,
        animesh::sources::anilist::client::AniListClient::new(base_url).expect("client"),
        Arc::clone(&clock) as Arc<dyn WallClock>,
        Arc::new(NoJitter),
        installation,
        1,
    ));
    let centre = FakeCenter::new();
    let reconciler = Reconciler::new(
        Arc::clone(&library),
        Arc::clone(&centre) as Arc<dyn NotificationSurface>,
    );

    World {
        _dir: dir,
        library,
        centre,
        reconciler,
        clock,
    }
}

/// A world with one followed title whose next episode is [`AIRTIME`].
async fn followed(server: &mut mockito::Server) -> World {
    // Exhausted after the one detail request a follow makes, so a test that
    // wants a second, different response is not served this one again.
    server
        .mock("POST", "/")
        .with_status(200)
        .with_body(detail_body(21, "One Piece", Some((5, AIRTIME))))
        .expect(1)
        .create_async()
        .await;

    let world = world(server.url()).await;
    world.library.follow(id(21)).await.expect("follow");
    world
}

// ---------------------------------------------------------------------------
// gates
// ---------------------------------------------------------------------------

#[tokio::test]
async fn a_followed_episode_is_registered_once() {
    let mut server = mockito::Server::new_async().await;
    let world = followed(&mut server).await;

    let report = world.reconciler.settle().await.expect("settle");
    assert_eq!(report.submitted, 1);
    assert_eq!(world.centre.adds().len(), 1);

    let counts = world.library.health().await.expect("health").notifications;
    assert_eq!(counts.registered, 1);
    assert_eq!(counts.desired, 0);
}

#[tokio::test]
async fn a_settled_plan_makes_no_further_os_calls() {
    // The acknowledgement-loop gate: a converged plan must be free to observe.
    let mut server = mockito::Server::new_async().await;
    let world = followed(&mut server).await;

    world.reconciler.settle().await.expect("first");
    world.centre.clear_ops();

    let report = world.reconciler.settle().await.expect("second");
    assert!(report.is_quiet());
    assert_eq!(report.confirmed, 1);
    assert!(world.centre.ops().is_empty());
}

#[tokio::test]
async fn os_acceptance_without_a_commit_heals_on_the_next_pass() {
    // The crash window: macOS took the request, the process died before the
    // transaction landed. The next pass must observe, not re-add.
    let mut server = mockito::Server::new_async().await;
    let world = followed(&mut server).await;

    let plan = world.library.selected_plan().await.expect("plan");
    assert_eq!(plan.items.len(), 1);
    world.centre.accept_behind_our_back(&plan.items[0].request);

    let report = world.reconciler.settle().await.expect("settle");
    assert_eq!(report.submitted, 0);
    assert_eq!(report.confirmed, 1);
    assert!(world.centre.adds().is_empty());
    assert_eq!(
        world
            .library
            .health()
            .await
            .expect("health")
            .notifications
            .registered,
        1
    );
}

#[tokio::test]
async fn a_reschedule_replaces_under_the_same_identifier() {
    let mut server = mockito::Server::new_async().await;
    let world = followed(&mut server).await;
    world.reconciler.settle().await.expect("first");

    let before = world.library.selected_plan().await.expect("plan");
    let identifier = before.items[0].request.os_identifier.as_str().to_owned();
    let original_json = world
        .centre
        .pending_json(&identifier)
        .expect("registered json");
    assert_eq!(before.items[0].desired_revision, 1);

    // AniList moves the airtime.
    server
        .mock("POST", "/")
        .with_status(200)
        .with_body(batch_body(21, "One Piece", Some((5, AIRTIME + 3_600))))
        .create_async()
        .await;
    world.clock.set(at(AIRTIME - 1_000));
    let pass = world.library.refresh_due(10).await.expect("refresh");
    assert_eq!(pass.applied, 1, "the reschedule was not applied");

    world.centre.clear_ops();
    let report = world.reconciler.settle().await.expect("second");

    assert_eq!(report.submitted, 1);
    // Same identifier: this is a replacement, not a second banner.
    assert_eq!(world.centre.adds(), vec![identifier.clone()]);
    assert_eq!(world.centre.pending_ids(), vec![identifier.clone()]);

    let after = world.library.selected_plan().await.expect("plan");
    assert_eq!(after.items[0].desired_revision, 2);
    assert_ne!(
        world.centre.pending_json(&identifier).expect("json"),
        original_json
    );
}

#[tokio::test]
async fn dropping_a_follow_removes_its_pending_request() {
    let mut server = mockito::Server::new_async().await;
    let world = followed(&mut server).await;
    world.reconciler.settle().await.expect("first");
    assert_eq!(world.centre.pending_ids().len(), 1);

    let media_id = world.library.list_follows().await.expect("list")[0].media_id;
    world.library.drop_follow(media_id).await.expect("drop");

    let report = world.reconciler.settle().await.expect("second");
    assert_eq!(report.removed, 1);
    assert!(world.centre.pending_ids().is_empty());
    assert!(world
        .library
        .selected_plan()
        .await
        .expect("plan")
        .items
        .is_empty());
}

#[tokio::test]
async fn every_add_precedes_the_first_removal() {
    // Removing first would leave the OS holding nothing for an episode that is
    // about to air. The desired set must be fully submitted before any cleanup.
    let mut server = mockito::Server::new_async().await;
    let world = followed(&mut server).await;

    // A pending request from a previous life that nothing wants any more.
    world.centre.lock().pending.push(PendingRequest {
        os_identifier: "dev.animesh.release.old.stale".to_owned(),
        request_json: None,
    });

    world.reconciler.settle().await.expect("settle");

    let ops = world.centre.ops();
    let first_remove = ops.iter().position(|op| matches!(op, Op::Remove(_)));
    let last_add = ops.iter().rposition(|op| matches!(op, Op::Add(_)));
    assert!(
        first_remove.is_some(),
        "the stale request was never removed"
    );
    assert!(last_add.is_some(), "nothing was ever added");
    assert!(
        last_add < first_remove,
        "removal ran before an add: {ops:?}"
    );
}

#[tokio::test]
async fn a_foreign_pending_request_is_never_removed() {
    let mut server = mockito::Server::new_async().await;
    let world = followed(&mut server).await;
    world.centre.lock().pending.push(PendingRequest {
        os_identifier: "com.other.app.reminder".to_owned(),
        request_json: None,
    });

    world.reconciler.settle().await.expect("settle");

    assert!(world
        .centre
        .pending_ids()
        .contains(&"com.other.app.reminder".to_owned()));
}

#[tokio::test]
async fn a_delivered_request_is_terminal() {
    let mut server = mockito::Server::new_async().await;
    let world = followed(&mut server).await;
    world.reconciler.settle().await.expect("first");

    let identifier = world.centre.pending_ids()[0].clone();
    world.centre.deliver(&identifier);
    world.centre.clear_ops();

    let report = world.reconciler.settle().await.expect("second");
    assert_eq!(report.delivered, 1);
    // Delivered leaves the plan, so the next pass has nothing to do at all.
    assert!(world
        .library
        .selected_plan()
        .await
        .expect("plan")
        .items
        .is_empty());
    assert!(world.centre.adds().is_empty());
}

#[tokio::test]
async fn denial_registers_nothing_and_records_the_state() {
    let mut server = mockito::Server::new_async().await;
    let world = followed(&mut server).await;
    world.centre.set_authorization(AuthorizationState::Denied);

    let report = world.reconciler.settle().await.expect("settle");
    assert_eq!(report.submitted, 0);
    assert!(report.blocked.is_some());
    assert!(world.centre.ops().is_empty());

    let health = world.library.health().await.expect("health");
    assert_eq!(health.authorization, AuthorizationState::Denied);
    assert!(health
        .degraded
        .contains(&animesh::domain::read_models::DegradedReason::NotificationsDenied));
    // The job survives denial: reversing the setting must register it, not
    // require the episode to be rediscovered.
    assert_eq!(health.notifications.desired, 1);
}

#[tokio::test]
async fn a_transient_failure_backs_off_rather_than_retrying_every_pass() {
    let mut server = mockito::Server::new_async().await;
    let world = followed(&mut server).await;
    world.centre.fail_adds.store(true, Ordering::SeqCst);

    let first = world.reconciler.settle().await.expect("first");
    assert_eq!(first.failed, 1);
    assert_eq!(world.centre.adds().len(), 1);

    world.centre.clear_ops();
    let second = world.reconciler.settle().await.expect("second");
    assert_eq!(second.backing_off, 1);
    assert!(
        world.centre.adds().is_empty(),
        "a failed job was retried inside its backoff"
    );

    // Past the backoff it is attempted again.
    world.clock.set(at(NOW + 3_600));
    world.centre.fail_adds.store(false, Ordering::SeqCst);
    let third = world.reconciler.settle().await.expect("third");
    assert_eq!(third.submitted, 1);
}

#[tokio::test]
async fn re_following_revives_the_cancelled_job() {
    // Dropping cancels the job. Re-following within the refresh cadence does
    // not refetch, so the observation path never runs — without a symmetric
    // revive the title is followed but silently un-notified.
    let mut server = mockito::Server::new_async().await;
    let world = followed(&mut server).await;
    world.reconciler.settle().await.expect("first");

    let media_id = world.library.list_follows().await.expect("list")[0].media_id;
    let identifier = world.centre.pending_ids()[0].clone();

    world.library.drop_follow(media_id).await.expect("drop");
    world.reconciler.settle().await.expect("after drop");
    assert!(world.centre.pending_ids().is_empty());

    world.library.follow(id(21)).await.expect("re-follow");
    let report = world.reconciler.settle().await.expect("after re-follow");

    assert_eq!(report.submitted, 1, "the revived job was never registered");
    // The same episode keeps its identifier, so this is one banner, not two.
    assert_eq!(world.centre.pending_ids(), vec![identifier]);
}
