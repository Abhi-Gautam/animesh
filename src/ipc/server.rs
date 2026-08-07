//! The concurrent request/reply server.

use std::future::Future;
use std::sync::Arc;
use std::time::Duration;

use tokio::io::{AsyncRead, AsyncWrite};
use tokio::sync::{watch, Semaphore};

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

    loop {
        tokio::select! {
            biased;

            changed = shutdown.changed() => {
                if changed.is_err() || *shutdown.borrow() {
                    break;
                }
            }

            accepted = endpoint.listener().accept() => {
                let Ok((stream, _)) = accepted else { continue };
                let Ok(permit) = Arc::clone(&permits).acquire_owned().await else { break };

                let handler = handler.clone();
                let instance_id = Arc::clone(&instance_id);
                // Spawned immediately, so the accept loop never runs at a
                // client's pace.
                tokio::spawn(async move {
                    let _permit = permit;
                    serve_connection(stream, &instance_id, handler).await;
                });
            }
        }
    }
}

async fn serve_connection<S, H, F>(mut stream: S, instance_id: &str, handler: H)
where
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

    let reply = build_reply(&frame, instance_id, handler).await;

    if let Ok(bytes) = serde_json::to_vec(&reply) {
        let _ = tokio::time::timeout(FRAME_TIMEOUT, write_frame(&mut stream, &bytes)).await;
    }
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
