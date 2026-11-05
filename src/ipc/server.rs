//! The concurrent request/reply server.

use std::future::Future;
use std::sync::Arc;
use std::time::Duration;

use std::io::Write;
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite};
use tokio::sync::{watch, Semaphore};
use tokio::task::JoinSet;

use crate::error::{AppError, ErrorCode};

use super::endpoint::{read_frame, write_frame, Endpoint};
use super::protocol::{check_version, ReplyEnvelope, Request, RequestEnvelope, Response};

/// Concurrent clients. Beyond this, accepts queue rather than fail, so a burst
/// is slow rather than broken.
pub const MAX_CLIENTS: usize = 32;

/// Deadline for reading a request frame or writing a reply.
///
/// A client that connects and then stalls must not hold a slot indefinitely;
/// this is what keeps one hung CLI from starving the others.
pub const FRAME_TIMEOUT: Duration = Duration::from_secs(2);

/// Serves until `shutdown` reports true.
pub async fn serve<H, F>(
    endpoint: &Endpoint,
    instance_id: Arc<str>,
    handler: H,
    mut shutdown: watch::Receiver<bool>,
) where
    H: Fn(Request) -> F + Clone + Send + Sync + 'static,
    F: Future<Output = Result<Response, AppError>> + Send + 'static,
{
    let permits = Arc::new(Semaphore::new(MAX_CLIENTS));
    let waits = Arc::new(Semaphore::new(8));
    let mut tasks = JoinSet::new();

    loop {
        tokio::select! {
            biased;

            changed = shutdown.changed() => {
                if changed.is_err() || *shutdown.borrow() {
                    break;
                }
            }

            Some(_) = tasks.join_next(), if !tasks.is_empty() => {},
            accepted = endpoint.listener().accept() => {
                let Ok((stream, _)) = accepted else { continue };
                let Ok(permit) = Arc::clone(&permits).try_acquire_owned() else { continue };

                let handler = handler.clone();
                let instance_id = Arc::clone(&instance_id);
                // Spawned immediately, so the accept loop never runs at a
                // client's pace.
                let waits=Arc::clone(&waits);
                tasks.spawn(async move {
                    serve_connection(stream, &instance_id, handler, permit, waits).await;
                });
            }
        }
    }
    // Completed database jobs commit independently; connection tasks cannot block shutdown.
    tasks.abort_all();
    while tasks.join_next().await.is_some() {}
}

async fn serve_connection<S, H, F>(
    mut stream: S,
    instance_id: &str,
    handler: H,
    permit: tokio::sync::OwnedSemaphorePermit,
    waits: Arc<Semaphore>,
) where
    S: AsyncRead + AsyncWrite + Unpin,
    H: Fn(Request) -> F,
    F: Future<Output = Result<Response, AppError>>,
{
    let frame = match tokio::time::timeout(FRAME_TIMEOUT, read_frame(&mut stream)).await {
        Ok(Ok(frame)) => frame,
        // Timed out, disconnected, or over the cap. The connection is already
        // unusable and there is nowhere to report it to.
        _ => return,
    };

    let waiting = serde_json::from_slice::<RequestEnvelope>(&frame)
        .is_ok_and(|envelope| matches!(envelope.body, Request::WaitForRevision { .. }));
    let _wait_permit = if waiting {
        match waits.try_acquire_owned() {
            Ok(permit) => Some(permit),
            Err(_) => return,
        }
    } else {
        None
    };
    // Wait subscriptions have their own small budget and do not consume request capacity.
    let _request_permit = if waiting {
        drop(permit);
        None
    } else {
        Some(permit)
    };
    let reply = if waiting {
        let mut extra = [0; 1];
        tokio::select! {
            reply = build_reply(&frame, instance_id, handler) => reply,
            _ = stream.read(&mut extra) => return,
        }
    } else {
        // Once started, a mutation completes even if its client disconnects.
        match tokio::time::timeout(
            Duration::from_secs(300),
            build_reply(&frame, instance_id, handler),
        )
        .await
        {
            Ok(reply) => reply,
            Err(_) => return,
        }
    };

    // The serialization itself is byte-bounded, before allocating an oversized frame.
    let mut bytes = Vec::new();
    let encoded = serde_json::to_writer(BoundedWriter(&mut bytes), &reply);
    if encoded.is_err() {
        bytes = serde_json::to_vec(&ReplyEnvelope::err(
            "oversized",
            instance_id,
            AppError::invalid_argument(
                "The reply exceeds the transport limit; use paginated reads",
            ),
        ))
        .unwrap_or_default();
        // Preserve correlation whenever the validated request ID fits the envelope cap.
        if let Ok(envelope) = serde_json::from_slice::<RequestEnvelope>(&frame) {
            if envelope.request_id.len() <= 128 {
                bytes = serde_json::to_vec(&ReplyEnvelope::err(
                    envelope.request_id,
                    instance_id,
                    AppError::invalid_argument(
                        "The reply exceeds the transport limit; use paginated reads",
                    ),
                ))
                .unwrap_or_default();
            }
        }
    }
    let _ = tokio::time::timeout(FRAME_TIMEOUT, write_frame(&mut stream, &bytes)).await;
}

async fn build_reply<H, F>(frame: &[u8], instance_id: &str, handler: H) -> ReplyEnvelope
where
    H: Fn(Request) -> F,
    F: Future<Output = Result<Response, AppError>>,
{
    let envelope: RequestEnvelope = match serde_json::from_slice(frame) {
        Ok(envelope) => envelope,
        Err(error) => {
            return ReplyEnvelope::err(
                "unknown",
                instance_id,
                AppError::invalid_argument(format!("malformed request: {error}")),
            )
        }
    };

    if envelope.request_id.is_empty() || envelope.request_id.len() > 128 {
        return ReplyEnvelope::err(
            "invalid",
            instance_id,
            AppError::invalid_argument("Invalid request identity"),
        );
    }
    if let Err(error) = check_version(envelope.protocol_version) {
        return ReplyEnvelope::err(envelope.request_id, instance_id, error);
    }

    if let Err(error) = envelope.body.validate() {
        return ReplyEnvelope::err(envelope.request_id, instance_id, error);
    }

    match handler(envelope.body).await {
        Ok(response) => ReplyEnvelope::ok(envelope.request_id, instance_id, response),
        Err(error) => ReplyEnvelope::err(envelope.request_id, instance_id, error),
    }
}

/// The error every handler except `status` returns while bootstrap has failed.
pub fn bootstrap_unavailable() -> AppError {
    AppError::new(
        ErrorCode::BootstrapUnavailable,
        "Animesh started but its database is unusable; run 'animesh status' for details",
    )
}

struct BoundedWriter<'a>(&'a mut Vec<u8>);
impl Write for BoundedWriter<'_> {
    fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
        if self.0.len() + bytes.len() > super::protocol::MAX_FRAME_BYTES {
            return Err(std::io::Error::other("reply over cap"));
        }
        self.0.extend_from_slice(bytes);
        Ok(bytes.len())
    }
    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}
