//! Database connection lifecycle: open + path resolution.

use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use directories::ProjectDirs;
use turso::{Builder, Connection};

use super::migrations;

/// Open the default app database and apply pending migrations.
pub(crate) async fn open_default() -> Result<Connection> {
    let path = default_db_path()?;
    open_path(&path).await
}

/// Open a database at an explicit path (also used by tests via `ANIMESH_DB_PATH`).
pub(crate) async fn open_path(path: &Path) -> Result<Connection> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)
            .with_context(|| format!("create db directory {}", parent.display()))?;
    }

    let path_str = path
        .to_str()
        .with_context(|| format!("db path is not valid UTF-8: {}", path.display()))?;

    let db = Builder::new_local(path_str)
        .build()
        .await
        .with_context(|| format!("open Turso database at {}", path.display()))?;
    let mut conn = db.connect().context("connect to Turso database")?;
    migrations::migrate(&mut conn).await?;
    Ok(conn)
}

fn default_db_path() -> Result<PathBuf> {
    if let Ok(override_path) = std::env::var("ANIMESH_DB_PATH") {
        return Ok(PathBuf::from(override_path));
    }

    let dirs = ProjectDirs::from("dev", "animesh", "animesh")
        .context("resolve platform data directory for animesh")?;
    Ok(dirs.data_dir().join("animesh.db"))
}
