//! Filesystem locations, resolved once and passed explicitly.
//!
//! There is no global path resolver and no ambient environment lookup outside
//! [`AppPaths::production`]. Tests construct their own instance under a
//! temporary root, so two tests can never race on the same database.

use std::fs;
use std::io;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};

use thiserror::Error;

/// Directory mode for everything Animesh creates: owner-only.
const PRIVATE_DIR_MODE: u32 = 0o700;

/// File mode for the database, socket, and lock: owner read/write.
const PRIVATE_FILE_MODE: u32 = 0o600;

/// Longest usable Unix socket path, minus the terminator.
///
/// `sockaddr_un.sun_path` is 104 bytes on Darwin and 108 on Linux. Exceeding it
/// fails at `bind` with a truncation error that reads nothing like "your home
/// directory is too long", so it is checked up front.
#[cfg(target_os = "macos")]
pub const MAX_SOCKET_PATH_LEN: usize = 103;
/// Longest usable Unix socket path, minus the terminator.
#[cfg(not(target_os = "macos"))]
pub const MAX_SOCKET_PATH_LEN: usize = 107;

#[derive(Debug, Error)]
pub enum PathError {
    #[error("cannot determine the home directory")]
    NoHomeDirectory,

    #[error("socket path is {len} bytes, over the {MAX_SOCKET_PATH_LEN}-byte limit: {path}")]
    SocketPathTooLong { len: usize, path: String },

    #[error("creating {path}")]
    CreateDir {
        path: String,
        #[source]
        source: io::Error,
    },

    #[error("setting permissions on {path}")]
    SetPermissions {
        path: String,
        #[source]
        source: io::Error,
    },
}

/// Every path the app uses.
///
/// The runtime root is a field rather than a child of the data root because the
/// two belong in different places on Linux: durable state lives under XDG data,
/// while a socket is per-session and belongs in the runtime directory the system
/// clears on logout.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AppPaths {
    data_root: PathBuf,
    log_root: PathBuf,
    runtime_root: PathBuf,
}

impl AppPaths {
    /// The frozen production locations under the user's home directory.
    ///
    /// Under `test-harness`, `ANIMESH_DATA_ROOT` and `ANIMESH_LOG_ROOT` override
    /// them. That override does not exist in a release build, which is what
    /// keeps a packaged test run from writing to the real database.
    pub fn production() -> Result<Self, PathError> {
        #[cfg(feature = "test-harness")]
        if let Some(paths) = Self::from_environment() {
            return Ok(paths);
        }

        let base = directories::BaseDirs::new().ok_or(PathError::NoHomeDirectory)?;
        Ok(Self::for_platform(&base))
    }

    /// Apple's layout: capitalised, under `~/Library`, with no runtime dir of
    /// its own for a socket to live in.
    #[cfg(target_os = "macos")]
    fn for_platform(base: &directories::BaseDirs) -> Self {
        let home = base.home_dir();
        let data_root = home.join("Library/Application Support/Animesh");
        Self {
            runtime_root: data_root.join("runtime"),
            log_root: home.join("Library/Logs/Animesh"),
            data_root,
        }
    }

    /// XDG. Lowercase, because on Linux the owner sees these paths in a shell.
    ///
    /// `XDG_RUNTIME_DIR` is short (`/run/user/1000`), which matters: it is what
    /// keeps the socket inside `sun_path` no matter how deep the home directory
    /// is. It is absent on a headless box or in a container, so the data root
    /// stands in rather than failing to start.
    #[cfg(not(target_os = "macos"))]
    fn for_platform(base: &directories::BaseDirs) -> Self {
        let data_root = base.data_dir().join("animesh");
        Self {
            runtime_root: base
                .runtime_dir()
                .map_or_else(|| data_root.join("runtime"), |dir| dir.join("animesh")),
            log_root: base
                .state_dir()
                .unwrap_or_else(|| base.data_dir())
                .join("animesh"),
            data_root,
        }
    }

    #[cfg(feature = "test-harness")]
    fn from_environment() -> Option<Self> {
        let data_root = PathBuf::from(std::env::var_os("ANIMESH_DATA_ROOT")?);
        let log_root = PathBuf::from(std::env::var_os("ANIMESH_LOG_ROOT")?);
        Some(Self {
            runtime_root: data_root.join("runtime"),
            data_root,
            log_root,
        })
    }

    /// Roots everything under `root`, for tests and packaged fixtures.
    pub fn under_root(root: impl AsRef<Path>) -> Self {
        let root = root.as_ref();
        let data_root = root.join("data");
        Self {
            runtime_root: data_root.join("runtime"),
            log_root: root.join("logs"),
            data_root,
        }
    }

    pub fn data_root(&self) -> &Path {
        &self.data_root
    }

    pub fn log_root(&self) -> &Path {
        &self.log_root
    }

    pub fn database(&self) -> PathBuf {
        self.data_root.join("library.db")
    }

    pub fn runtime_root(&self) -> &Path {
        &self.runtime_root
    }

    pub fn socket(&self) -> PathBuf {
        self.runtime_root.join("app.sock")
    }

    pub fn lock(&self) -> PathBuf {
        self.runtime_root.join("app.lock")
    }

    /// Creates the directory tree with owner-only permissions.
    ///
    /// Idempotent, and re-tightens the mode on directories that already exist:
    /// a data root that became group-readable is a real leak of what the owner
    /// watches, and fixing it costs one `chmod`.
    pub fn ensure_dirs(&self) -> Result<(), PathError> {
        for dir in [&self.data_root, &self.log_root, &self.runtime_root] {
            create_private_dir(dir)?;
        }
        self.check_socket_path()
    }

    /// Rejects a socket path the platform cannot bind.
    pub fn check_socket_path(&self) -> Result<(), PathError> {
        let socket = self.socket();
        let len = socket.as_os_str().len();
        if len > MAX_SOCKET_PATH_LEN {
            return Err(PathError::SocketPathTooLong {
                len,
                path: socket.display().to_string(),
            });
        }
        Ok(())
    }
}

fn create_private_dir(path: &Path) -> Result<(), PathError> {
    fs::create_dir_all(path).map_err(|source| PathError::CreateDir {
        path: path.display().to_string(),
        source,
    })?;
    fs::set_permissions(path, fs::Permissions::from_mode(PRIVATE_DIR_MODE)).map_err(|source| {
        PathError::SetPermissions {
            path: path.display().to_string(),
            source,
        }
    })
}

/// Tightens an existing file to owner-only.
pub fn restrict_file(path: &Path) -> Result<(), PathError> {
    fs::set_permissions(path, fs::Permissions::from_mode(PRIVATE_FILE_MODE)).map_err(|source| {
        PathError::SetPermissions {
            path: path.display().to_string(),
            source,
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn mode_of(path: &Path) -> u32 {
        fs::metadata(path).expect("metadata").permissions().mode() & 0o777
    }

    #[test]
    fn layout_matches_the_frozen_decisions() {
        let paths = AppPaths::under_root("/tmp/animesh-test");
        assert_eq!(
            paths.database(),
            Path::new("/tmp/animesh-test/data/library.db")
        );
        assert_eq!(
            paths.runtime_root(),
            Path::new("/tmp/animesh-test/data/runtime")
        );
        assert_eq!(
            paths.socket(),
            Path::new("/tmp/animesh-test/data/runtime/app.sock")
        );
        assert_eq!(
            paths.lock(),
            Path::new("/tmp/animesh-test/data/runtime/app.lock")
        );
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn production_layout_is_under_library() {
        let paths = AppPaths::production().expect("home directory");
        let data = paths.data_root().display().to_string();
        assert!(
            data.ends_with("Library/Application Support/Animesh"),
            "unexpected data root: {data}"
        );
        let logs = paths.log_root().display().to_string();
        assert!(
            logs.ends_with("Library/Logs/Animesh"),
            "unexpected log root: {logs}"
        );
    }

    #[cfg(not(target_os = "macos"))]
    #[test]
    fn production_layout_follows_xdg() {
        let paths = AppPaths::production().expect("home directory");
        let data = paths.data_root().display().to_string();
        assert!(data.ends_with("/animesh"), "unexpected data root: {data}");
        // Durable state and the socket must not share a directory: one survives
        // a logout and the other must not.
        assert_ne!(paths.runtime_root(), paths.data_root());
    }

    #[cfg(not(target_os = "macos"))]
    #[test]
    fn the_socket_lives_in_the_runtime_dir_when_the_session_has_one() {
        // The whole reason runtime_root is its own field: /run/user/<uid> keeps
        // the socket inside sun_path however deep the home directory is.
        let Some(base) = directories::BaseDirs::new() else {
            return;
        };
        let Some(runtime) = base.runtime_dir() else {
            return;
        };
        let paths = AppPaths::for_platform(&base);
        assert!(
            paths.socket().starts_with(runtime),
            "socket is at {}, outside {}",
            paths.socket().display(),
            runtime.display()
        );
    }

    #[test]
    fn ensure_dirs_creates_everything_owner_only() {
        let temp = tempfile::tempdir().expect("tempdir");
        let paths = AppPaths::under_root(temp.path());
        paths.ensure_dirs().expect("create dirs");

        assert_eq!(mode_of(paths.data_root()), PRIVATE_DIR_MODE);
        assert_eq!(mode_of(paths.log_root()), PRIVATE_DIR_MODE);
        assert_eq!(mode_of(paths.runtime_root()), PRIVATE_DIR_MODE);
    }

    #[test]
    fn ensure_dirs_is_idempotent() {
        let temp = tempfile::tempdir().expect("tempdir");
        let paths = AppPaths::under_root(temp.path());
        paths.ensure_dirs().expect("first call");
        paths.ensure_dirs().expect("second call");
    }

    #[test]
    fn ensure_dirs_retightens_a_loosened_directory() {
        let temp = tempfile::tempdir().expect("tempdir");
        let paths = AppPaths::under_root(temp.path());
        paths.ensure_dirs().expect("create dirs");

        fs::set_permissions(paths.data_root(), fs::Permissions::from_mode(0o755))
            .expect("loosen mode");
        paths.ensure_dirs().expect("re-run");

        assert_eq!(mode_of(paths.data_root()), PRIVATE_DIR_MODE);
    }

    #[test]
    fn restrict_file_makes_a_file_owner_only() {
        let temp = tempfile::tempdir().expect("tempdir");
        let file = temp.path().join("library.db");
        fs::write(&file, b"").expect("write");
        fs::set_permissions(&file, fs::Permissions::from_mode(0o644)).expect("loosen");

        restrict_file(&file).expect("restrict");
        assert_eq!(mode_of(&file), PRIVATE_FILE_MODE);
    }

    #[test]
    fn overlong_socket_path_is_rejected_before_bind() {
        // A deep temp root would otherwise fail at bind() with an error that
        // says nothing about path length.
        let deep = PathBuf::from("/tmp").join("x".repeat(200));
        let paths = AppPaths::under_root(deep);
        assert!(matches!(
            paths.check_socket_path(),
            Err(PathError::SocketPathTooLong { .. })
        ));
    }

    #[test]
    fn production_socket_path_fits_the_platform_limit() {
        // Guards against the frozen layout plus a realistic home directory
        // silently exceeding sun_path.
        let paths = AppPaths::production().expect("home directory");
        paths.check_socket_path().expect("socket path fits");
    }
}
