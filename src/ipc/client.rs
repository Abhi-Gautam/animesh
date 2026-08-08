//! The CLI side of the socket.

use std::io;
use std::path::Path;
use std::time::Duration;

use tokio::net::UnixStream;

use crate::error::{AppError, ErrorCode};
use crate::paths::AppPaths;

use super::endpoint::{read_frame, write_frame, IpcError};
use super::protocol::{check_version, ReplyResult, Request, RequestEnvelope, Response};

/// How long the client waits for the app to answer.
///
/// Longer than the server's frame deadline because a follow legitimately waits
/// on an AniList round trip behind it.
pub const REQUEST_TIMEOUT: Duration = Duration::from_secs(30);

/// Sends one request and returns the app's reply.
pub async fn send(paths: &AppPaths, request: Request) -> Result<Response, AppError> {
    request.validate()?;
    send_to(&paths.socket(), request).await
}

pub async fn send_to(socket: &Path, request: Request) -> Result<Response, AppError> {
    let mut stream = connect(socket).await?;

    let envelope = RequestEnvelope::new(request_id(), request);
    let bytes = serde_json::to_vec(&envelope)
        .map_err(|e| AppError::internal(format!("encoding request: {e}")))?;

    tokio::time::timeout(REQUEST_TIMEOUT, write_frame(&mut stream, &bytes))
        .await
        .map_err(|_| timed_out())?
        .map_err(map_ipc)?;

    let frame = tokio::time::timeout(REQUEST_TIMEOUT, read_frame(&mut stream))
        .await
        .map_err(|_| timed_out())?
        .map_err(map_ipc)?;

    let reply: super::protocol::ReplyEnvelope = serde_json::from_slice(&frame)
        .map_err(|e| AppError::internal(format!("decoding reply: {e}")))?;

    check_version(reply.protocol_version)?;

    match reply.result {
        ReplyResult::Ok(response) => Ok(response),
        ReplyResult::Err(error) => Err(error),
    }
}

async fn connect(socket: &Path) -> Result<UnixStream, AppError> {
    match UnixStream::connect(socket).await {
        Ok(stream) => Ok(stream),
        // A missing or refusing socket means the app is not running, which is a
        // different problem from the app being broken, and has a different fix.
        Err(error)
            if matches!(
                error.kind(),
                io::ErrorKind::NotFound | io::ErrorKind::ConnectionRefused
            ) =>
        {
            Err(not_running())
        }
        Err(error) => Err(AppError::new(
            ErrorCode::Unavailable,
            format!("connecting to Animesh: {error}"),
        )),
    }
}

/// The one message that has to name a fix.
///
/// Everything funnels here — a stopped daemon, a first run before the service
/// was registered, a crash — so it names the single command that resolves all
/// three rather than a location that only exists on one platform.
fn not_running() -> AppError {
    AppError::new(
        ErrorCode::Unavailable,
        "Animesh is not running. Start it with 'animesh service start'.",
    )
}

fn timed_out() -> AppError {
    AppError::new(ErrorCode::Unavailable, "Animesh did not answer in time")
}

fn map_ipc(error: IpcError) -> AppError {
    match error {
        IpcError::FrameTooLarge => AppError::invalid_argument("the message was too large to send"),
        IpcError::AlreadyRunning => AppError::new(ErrorCode::AlreadyRunning, error.to_string()),
        IpcError::NotRunning => not_running(),
        IpcError::Timeout => timed_out(),
        other => AppError::new(ErrorCode::Unavailable, other.to_string()),
    }
}

fn request_id() -> String {
    uuid::Uuid::new_v4().to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn an_absent_socket_reports_that_the_app_is_not_running() {
        let dir = tempfile::tempdir().expect("tempdir");
        let error = send_to(&dir.path().join("missing.sock"), Request::Status)
            .await
            .expect_err("should fail");

        assert_eq!(error.code, ErrorCode::Unavailable);
        assert!(error.message.contains("not running"), "{}", error.message);
        // Exit 3 tells a script this is worth retrying.
        assert_eq!(error.exit_category(), crate::error::ExitCategory::Temporary);
    }

    #[tokio::test]
    async fn client_side_validation_runs_before_connecting() {
        // A bad query must not even open a socket.
        let dir = tempfile::tempdir().expect("tempdir");
        let paths = AppPaths::under_root(dir.path());
        let error = send(
            &paths,
            Request::SearchAnime {
                query: String::new(),
            },
        )
        .await
        .expect_err("should fail");

        assert_eq!(error.code, ErrorCode::InvalidArgument);
    }
}
