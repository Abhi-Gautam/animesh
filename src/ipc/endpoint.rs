//! The user-private Unix socket and the singleton lock that guards it.
//!
//! Ordering matters: the lock is acquired before the stale socket is removed
//! and before the database is opened. Only the lock holder may unlink or
//! rebind, so a second instance can never delete a live endpoint out from under
//! the running one.

use std::fs::{File, OpenOptions};
use std::io;
use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};
use std::path::{Path, PathBuf};

use thiserror::Error;
use tokio::net::UnixListener;

use crate::paths::{AppPaths, PathError};

use super::protocol::MAX_FRAME_BYTES;

#[derive(Debug, Error)]
pub enum IpcError {
    #[error("another Animesh instance is already running")]
    AlreadyRunning,

    #[error("the app is not running")]
    NotRunning,

    #[error("io: {0}")]
    Io(#[from] io::Error),

    #[error("path: {0}")]
    Path(#[from] PathError),

    #[error("frame exceeds the {MAX_FRAME_BYTES}-byte limit")]
    FrameTooLarge,

    #[error("malformed frame: {0}")]
    Malformed(String),

    #[error("timed out waiting for the peer")]
    Timeout,
}

/// An exclusive advisory lock held for the life of the process.
///
/// `flock` is released by the kernel when the file descriptor closes, including
/// on `SIGKILL`, which is what lets the next owner safely reclaim a stale
/// socket without a liveness heartbeat.
#[derive(Debug)]
pub struct SingletonLock {
    _file: File,
    path: PathBuf,
}

impl SingletonLock {
    pub fn acquire(path: &Path) -> Result<Self, IpcError> {
        let file = OpenOptions::new()
            .create(true)
            .read(true)
            .write(true)
            .truncate(false)
            .mode(0o600)
            .open(path)?;

        match rustix::fs::flock(&file, rustix::fs::FlockOperation::NonBlockingLockExclusive) {
            Ok(()) => Ok(Self {
                _file: file,
                path: path.to_path_buf(),
            }),
            // EWOULDBLOCK and EAGAIN are the same errno here, so one arm covers
            // both: another process holds the lock.
            Err(rustix::io::Errno::WOULDBLOCK) => Err(IpcError::AlreadyRunning),
            Err(error) => Err(IpcError::Io(io::Error::from(error))),
        }
    }

    pub fn path(&self) -> &Path {
        &self.path
    }
}

/// A bound listener plus the lock that authorizes owning it.
#[derive(Debug)]
pub struct Endpoint {
    listener: UnixListener,
    socket_path: PathBuf,
    _lock: SingletonLock,
}

impl Endpoint {
    /// Acquires the lock, clears any stale socket, and binds.
    pub fn bind(paths: &AppPaths) -> Result<Self, IpcError> {
        paths.ensure_dirs()?;
        let lock = SingletonLock::acquire(&paths.lock())?;

        // Safe only because the lock is held: no live instance can be using it.
        let socket_path = paths.socket();
        if socket_path.exists() {
            std::fs::remove_file(&socket_path)?;
        }

        let listener = UnixListener::bind(&socket_path)?;
        std::fs::set_permissions(&socket_path, std::fs::Permissions::from_mode(0o600))?;

        Ok(Self {
            listener,
            socket_path,
            _lock: lock,
        })
    }

    pub fn listener(&self) -> &UnixListener {
        &self.listener
    }

    pub fn socket_path(&self) -> &Path {
        &self.socket_path
    }
}

impl Drop for Endpoint {
    fn drop(&mut self) {
        // Unlinking while the lock is still held keeps the window in which
        // another instance could bind to a path we are about to remove closed.
        let _ = std::fs::remove_file(&self.socket_path);
    }
}

/// Writes one length-prefixed frame.
pub async fn write_frame<W>(writer: &mut W, payload: &[u8]) -> Result<(), IpcError>
where
    W: tokio::io::AsyncWrite + Unpin,
{
    use tokio::io::AsyncWriteExt;

    let len = u32::try_from(payload.len()).map_err(|_| IpcError::FrameTooLarge)?;
    if payload.len() > MAX_FRAME_BYTES {
        return Err(IpcError::FrameTooLarge);
    }
    writer.write_all(&len.to_be_bytes()).await?;
    writer.write_all(payload).await?;
    writer.flush().await?;
    Ok(())
}

/// Reads one length-prefixed frame.
///
/// The cap is checked against the declared length *before* the body buffer is
/// allocated, so a peer claiming four gigabytes costs four bytes to reject.
pub async fn read_frame<R>(reader: &mut R) -> Result<Vec<u8>, IpcError>
where
    R: tokio::io::AsyncRead + Unpin,
{
    use tokio::io::AsyncReadExt;

    let mut length = [0u8; 4];
    reader.read_exact(&mut length).await?;
    let declared = u32::from_be_bytes(length) as usize;
    if declared > MAX_FRAME_BYTES {
        return Err(IpcError::FrameTooLarge);
    }

    let mut body = vec![0u8; declared];
    reader.read_exact(&mut body).await?;
    Ok(body)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_second_lock_attempt_is_refused() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("app.lock");

        let _first = SingletonLock::acquire(&path).expect("first lock");
        assert!(matches!(
            SingletonLock::acquire(&path),
            Err(IpcError::AlreadyRunning)
        ));
    }

    #[test]
    fn releasing_the_lock_lets_the_next_owner_take_it() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("app.lock");

        let first = SingletonLock::acquire(&path).expect("first");
        drop(first);
        let _second = SingletonLock::acquire(&path).expect("second");
    }

    #[test]
    fn the_lock_file_is_owner_only() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("app.lock");
        let _lock = SingletonLock::acquire(&path).expect("lock");

        let mode = std::fs::metadata(&path)
            .expect("metadata")
            .permissions()
            .mode()
            & 0o777;
        assert_eq!(mode, 0o600);
    }

    #[tokio::test]
    async fn binding_creates_an_owner_only_socket() {
        let dir = tempfile::tempdir().expect("tempdir");
        let paths = AppPaths::under_root(dir.path());
        let endpoint = Endpoint::bind(&paths).expect("bind");

        let mode = std::fs::metadata(endpoint.socket_path())
            .expect("metadata")
            .permissions()
            .mode()
            & 0o777;
        assert_eq!(mode, 0o600, "socket is not owner-only");
    }

    #[tokio::test]
    async fn a_second_instance_cannot_bind_or_unlink() {
        let dir = tempfile::tempdir().expect("tempdir");
        let paths = AppPaths::under_root(dir.path());
        let first = Endpoint::bind(&paths).expect("first");

        assert!(matches!(
            Endpoint::bind(&paths),
            Err(IpcError::AlreadyRunning)
        ));
        // The live socket must survive the rejected attempt.
        assert!(first.socket_path().exists());
    }

    #[tokio::test]
    async fn a_stale_socket_is_reclaimed_by_the_next_owner() {
        // Models SIGKILL: the socket file survives, the lock does not.
        let dir = tempfile::tempdir().expect("tempdir");
        let paths = AppPaths::under_root(dir.path());

        std::fs::create_dir_all(paths.runtime_root()).expect("mkdir");
        let orphan = UnixListener::bind(paths.socket()).expect("orphan bind");
        drop(orphan);
        assert!(paths.socket().exists());

        let endpoint = Endpoint::bind(&paths).expect("reclaim");
        assert!(endpoint.socket_path().exists());
    }

    #[tokio::test]
    async fn dropping_the_endpoint_removes_the_socket() {
        let dir = tempfile::tempdir().expect("tempdir");
        let paths = AppPaths::under_root(dir.path());
        let socket = {
            let endpoint = Endpoint::bind(&paths).expect("bind");
            endpoint.socket_path().to_path_buf()
        };
        assert!(!socket.exists(), "socket outlived the endpoint");
    }

    #[tokio::test]
    async fn an_oversized_length_prefix_is_refused_before_allocating() {
        // A peer claiming four gigabytes must cost four bytes to reject, not
        // four gigabytes of buffer.
        let declared = u32::try_from(MAX_FRAME_BYTES + 1).unwrap_or(u32::MAX);
        let mut frame = declared.to_be_bytes().to_vec();
        frame.push(b'x');

        let mut reader = frame.as_slice();
        assert!(matches!(
            read_frame(&mut reader).await,
            Err(IpcError::FrameTooLarge)
        ));
    }

    #[tokio::test]
    async fn frames_round_trip() {
        let mut buffer = Vec::new();
        write_frame(&mut buffer, b"hello").await.expect("write");

        let mut reader = buffer.as_slice();
        assert_eq!(read_frame(&mut reader).await.expect("read"), b"hello");
    }

    #[tokio::test]
    async fn a_truncated_frame_is_an_error_not_a_short_read() {
        let mut buffer = Vec::new();
        write_frame(&mut buffer, b"hello world")
            .await
            .expect("write");
        buffer.truncate(6);

        let mut reader = buffer.as_slice();
        assert!(read_frame(&mut reader).await.is_err());
    }
}
