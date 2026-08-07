//! I1 — the IPC transport contract.
//!
//! Covers the hostile-client and singleton scenarios from plan section 23:
//! a stalled client must not starve the others, an oversized or truncated
//! frame must be contained, and a second instance must not be able to unlink a
//! live endpoint.

#![allow(clippy::expect_used)]

use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::Arc;
use std::time::Duration;

use animesh::domain::ids::UnixTimestamp;
use animesh::domain::read_models::{
    AuthorizationState, BootstrapState, HealthSnapshot, NotificationCounts, RefreshCounts,
};
use animesh::error::ErrorCode;
use animesh::ipc::client;
use animesh::ipc::endpoint::{read_frame, write_frame, Endpoint, IpcError};
use animesh::ipc::protocol::{
    Request, RequestEnvelope, Response, MAX_FRAME_BYTES, PROTOCOL_VERSION,
};
use animesh::ipc::server;
use animesh::paths::AppPaths;
use tokio::net::UnixStream;
use tokio::sync::watch;

fn snapshot() -> HealthSnapshot {
    HealthSnapshot {
        process_version: "test".into(),
        schema_version: 1,
        protocol_version: PROTOCOL_VERSION,
        app_instance_id: "instance".into(),
        started_at: UnixTimestamp::EPOCH,
        bootstrap: BootstrapState::Ready,
        database_ready: true,
        active_follows: 0,
        earliest_upcoming: None,
        last_success_at: None,
        refresh: RefreshCounts::default(),
        source_blocked_until: None,
        notifications: NotificationCounts::default(),
        last_reconciled_at: None,
        authorization: AuthorizationState::Unknown,
        authorization_observed_at: None,
        degraded: Vec::new(),
    }
}

struct Harness {
    _dir: tempfile::TempDir,
    paths: AppPaths,
    shutdown: watch::Sender<bool>,
    calls: Arc<AtomicU32>,
}

impl Harness {
    /// Starts a server whose handler sleeps `delay` before answering.
    async fn start(delay: Duration) -> Self {
        let dir = tempfile::tempdir().expect("tempdir");
        let paths = AppPaths::under_root(dir.path());
        let endpoint = Endpoint::bind(&paths).expect("bind");
        let (shutdown, rx) = watch::channel(false);
        let calls = Arc::new(AtomicU32::new(0));

        let handler_calls = Arc::clone(&calls);
        tokio::spawn(async move {
            server::serve(
                &endpoint,
                Arc::from("instance"),
                move |request: Request| {
                    let calls = Arc::clone(&handler_calls);
                    async move {
                        calls.fetch_add(1, Ordering::SeqCst);
                        if !delay.is_zero() {
                            tokio::time::sleep(delay).await;
                        }
                        Ok(match request {
                            Request::Status => Response::Status(Box::new(snapshot())),
                            Request::ListFollows => Response::ListFollows(Vec::new()),
                            Request::Upcoming { .. } => Response::Upcoming(Vec::new()),
                            _ => Response::SearchAnime(Vec::new()),
                        })
                    }
                },
                rx,
            )
            .await;
        });

        // Let the accept loop reach its first poll.
        tokio::time::sleep(Duration::from_millis(50)).await;

        Self {
            _dir: dir,
            paths,
            shutdown,
            calls,
        }
    }
}

#[tokio::test]
async fn a_request_round_trips() {
    let harness = Harness::start(Duration::ZERO).await;

    let response = client::send(&harness.paths, Request::Status)
        .await
        .expect("status");
    assert!(matches!(response, Response::Status(_)));
    assert_eq!(harness.calls.load(Ordering::SeqCst), 1);
}

#[tokio::test]
async fn a_protocol_mismatch_is_explicit() {
    let harness = Harness::start(Duration::ZERO).await;

    let mut stream = UnixStream::connect(harness.paths.socket())
        .await
        .expect("connect");
    let mut envelope = RequestEnvelope::new("req", Request::Status);
    envelope.protocol_version = PROTOCOL_VERSION + 1;
    let bytes = serde_json::to_vec(&envelope).expect("encode");
    write_frame(&mut stream, &bytes).await.expect("write");

    let frame = read_frame(&mut stream).await.expect("read");
    let reply: serde_json::Value = serde_json::from_slice(&frame).expect("decode");
    assert_eq!(reply["result"]["data"]["code"], "protocol_mismatch");

    // The handler must never have run.
    assert_eq!(harness.calls.load(Ordering::SeqCst), 0);
}

#[tokio::test]
async fn a_malformed_frame_gets_a_typed_error_not_a_dropped_connection() {
    let harness = Harness::start(Duration::ZERO).await;

    let mut stream = UnixStream::connect(harness.paths.socket())
        .await
        .expect("connect");
    write_frame(&mut stream, b"{not json").await.expect("write");

    let frame = read_frame(&mut stream).await.expect("read");
    let reply: serde_json::Value = serde_json::from_slice(&frame).expect("decode");
    assert_eq!(reply["result"]["data"]["code"], "invalid_argument");
    assert_eq!(harness.calls.load(Ordering::SeqCst), 0);
}

#[tokio::test]
async fn server_side_validation_rejects_a_hostile_argument() {
    // The client validates too, but a client-only check is not a check.
    let harness = Harness::start(Duration::ZERO).await;

    let mut stream = UnixStream::connect(harness.paths.socket())
        .await
        .expect("connect");
    let envelope = serde_json::json!({
        "protocol_version": PROTOCOL_VERSION,
        "request_id": "req",
        "body": { "kind": "upcoming", "data": { "limit": 100000 } }
    });
    let bytes = serde_json::to_vec(&envelope).expect("encode");
    write_frame(&mut stream, &bytes).await.expect("write");

    let frame = read_frame(&mut stream).await.expect("read");
    let reply: serde_json::Value = serde_json::from_slice(&frame).expect("decode");
    assert_eq!(reply["result"]["data"]["code"], "invalid_argument");
    assert_eq!(harness.calls.load(Ordering::SeqCst), 0);
}

#[tokio::test]
async fn an_oversized_frame_is_refused_without_disturbing_the_server() {
    let harness = Harness::start(Duration::ZERO).await;

    let mut stream = UnixStream::connect(harness.paths.socket())
        .await
        .expect("connect");
    let declared = u32::try_from(MAX_FRAME_BYTES + 1).unwrap_or(u32::MAX);
    let mut oversized = declared.to_be_bytes().to_vec();
    oversized.extend_from_slice(&[b'x'; 128]);
    use tokio::io::AsyncWriteExt;
    let _ = stream.write_all(&oversized).await;
    drop(stream);

    // The server survives and still answers a well-formed client.
    let response = client::send(&harness.paths, Request::Status)
        .await
        .expect("status after abuse");
    assert!(matches!(response, Response::Status(_)));
}

#[tokio::test]
async fn a_client_that_connects_and_stalls_does_not_block_others() {
    // Section 23 scenario 13: head-of-line blocking would make one hung CLI
    // freeze the menu.
    let harness = Harness::start(Duration::ZERO).await;

    let mut stalled = Vec::new();
    for _ in 0..8 {
        stalled.push(
            UnixStream::connect(harness.paths.socket())
                .await
                .expect("connect"),
        );
    }

    let response = tokio::time::timeout(
        Duration::from_secs(5),
        client::send(&harness.paths, Request::Status),
    )
    .await
    .expect("must not hang")
    .expect("status");
    assert!(matches!(response, Response::Status(_)));

    drop(stalled);
}

#[tokio::test]
async fn concurrent_clients_are_all_served() {
    let harness = Harness::start(Duration::from_millis(50)).await;

    let mut handles = Vec::new();
    for _ in 0..16 {
        let paths = harness.paths.clone();
        handles.push(tokio::spawn(async move {
            client::send(&paths, Request::ListFollows).await
        }));
    }

    for handle in handles {
        let response = handle.await.expect("join").expect("reply");
        assert!(matches!(response, Response::ListFollows(_)));
    }
    assert_eq!(harness.calls.load(Ordering::SeqCst), 16);
}

#[tokio::test]
async fn shutdown_stops_the_accept_loop_and_removes_the_socket() {
    let harness = Harness::start(Duration::ZERO).await;
    let socket = harness.paths.socket();
    assert!(socket.exists());

    harness.shutdown.send(true).expect("signal");
    tokio::time::sleep(Duration::from_millis(100)).await;

    // The Endpoint was moved into the serve task; once that task ends and drops
    // it, the socket is unlinked while the lock is still held.
    assert!(!socket.exists(), "socket outlived the server");
}

#[tokio::test]
async fn a_second_instance_cannot_take_a_live_endpoint() {
    let harness = Harness::start(Duration::ZERO).await;

    assert!(matches!(
        Endpoint::bind(&harness.paths),
        Err(IpcError::AlreadyRunning)
    ));

    // The original endpoint still works.
    let response = client::send(&harness.paths, Request::Status)
        .await
        .expect("status");
    assert!(matches!(response, Response::Status(_)));
}

#[tokio::test]
async fn a_client_with_no_server_reports_not_running() {
    let dir = tempfile::tempdir().expect("tempdir");
    let paths = AppPaths::under_root(dir.path());

    let error = client::send(&paths, Request::Status)
        .await
        .expect_err("nothing is listening");
    assert_eq!(error.code, ErrorCode::Unavailable);
}
