//! Resolves where the SQLite library file lives.
//!
//! - Linux:   `$XDG_DATA_HOME/animesh/library.db` (default `~/.local/share/animesh/library.db`)
//! - macOS:   `~/Library/Application Support/animesh/library.db`
//! - Windows: `%APPDATA%\animesh\library.db`
//! - Override (any OS): `ANIMESH_DB_PATH` env var — the entire path, not a parent dir.

use std::path::PathBuf;

use anyhow::{anyhow, Result};
use directories::BaseDirs;

const ENV_OVERRIDE: &str = "ANIMESH_DB_PATH";
const APP_DIR: &str = "animesh";
const DB_FILENAME: &str = "library.db";

/// Resolve the DB path using the process environment.
pub fn resolve_db_path() -> Result<PathBuf> {
    resolve_with(std::env::var(ENV_OVERRIDE).ok())
}

/// Pure variant: takes the override explicitly. Used by tests so we don't
/// have to mutate the process env in parallel-run unit tests.
pub fn resolve_with(override_path: Option<String>) -> Result<PathBuf> {
    if let Some(p) = override_path.filter(|s| !s.is_empty()) {
        return Ok(PathBuf::from(p));
    }
    let base = BaseDirs::new()
        .ok_or_else(|| anyhow!("could not determine OS data directory (no $HOME?)"))?;
    Ok(base.data_dir().join(APP_DIR).join(DB_FILENAME))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn override_returns_exact_path() {
        let p = resolve_with(Some("/tmp/animesh-test/library.db".to_string())).unwrap();
        assert_eq!(p, PathBuf::from("/tmp/animesh-test/library.db"));
    }

    #[test]
    fn empty_override_falls_through_to_default() {
        let p = resolve_with(Some(String::new())).unwrap();
        assert!(
            p.ends_with(PathBuf::from(APP_DIR).join(DB_FILENAME)),
            "expected default suffix, got {p:?}"
        );
    }

    #[test]
    fn default_path_has_expected_shape() {
        let p = resolve_with(None).unwrap();
        // We don't assert the platform-specific prefix (it depends on the
        // host running the tests). We do assert the tail.
        assert_eq!(p.file_name().and_then(|s| s.to_str()), Some(DB_FILENAME));
        assert_eq!(
            p.parent().and_then(|p| p.file_name()).and_then(|s| s.to_str()),
            Some(APP_DIR)
        );
    }

    #[test]
    fn default_path_is_under_an_os_data_dir() {
        // Sanity check that BaseDirs is functional in the test environment.
        let p = resolve_with(None).unwrap();
        let base = BaseDirs::new().unwrap();
        assert!(
            p.starts_with(base.data_dir()),
            "expected {p:?} to be under {:?}",
            base.data_dir()
        );
    }
}
