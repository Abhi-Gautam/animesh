//! Local Turso database infrastructure: open connection + migrations.
//!
//! Table-specific repositories live in submodules (e.g. [`watchlist`]).

pub(crate) mod watchlist;

use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use directories::ProjectDirs;
use turso::{Builder, Connection};

const MIGRATIONS: &[(i64, &str)] = &[
    (1, include_str!("../../migrations/001_watchlist.sql")),
    (
        2,
        include_str!("../../migrations/002_watchlist_next_airing.sql"),
    ),
];

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
    migrate(&mut conn).await?;
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

async fn migrate(conn: &mut Connection) -> Result<()> {
    conn.execute(
        "CREATE TABLE IF NOT EXISTS schema_migrations (
            version    INTEGER PRIMARY KEY NOT NULL,
            applied_at TEXT    NOT NULL
        )",
        (),
    )
    .await
    .context("create schema_migrations")?;

    for &(version, sql) in MIGRATIONS {
        if migration_applied(conn, version).await? {
            continue;
        }

        // Whole migration (statements + bookkeeping row) commits atomically,
        // so a mid-migration crash never leaves a half-applied schema behind
        // for the next startup to collide with.
        let tx = conn
            .transaction()
            .await
            .with_context(|| format!("begin transaction for migration {version}"))?;

        // Turso/SQLite: run one statement at a time.
        for stmt in split_sql_statements(sql) {
            tx.execute(stmt, ())
                .await
                .with_context(|| format!("apply migration {version}: {stmt}"))?;
        }

        let applied_at = chrono::Utc::now().to_rfc3339();
        tx.execute(
            "INSERT INTO schema_migrations (version, applied_at) VALUES (?1, ?2)",
            turso::params![version, applied_at.as_str()],
        )
        .await
        .with_context(|| format!("record migration {version}"))?;

        tx.commit()
            .await
            .with_context(|| format!("commit migration {version}"))?;
    }

    Ok(())
}

async fn migration_applied(conn: &Connection, version: i64) -> Result<bool> {
    let mut rows = conn
        .query(
            "SELECT 1 FROM schema_migrations WHERE version = ?1 LIMIT 1",
            turso::params![version],
        )
        .await
        .context("query schema_migrations")?;

    Ok(rows.next().await.context("read migration row")?.is_some())
}

/// Split a migration file into individual SQL statements (skip comments/blank).
fn split_sql_statements(sql: &str) -> Vec<&str> {
    sql.split(';')
        .map(str::trim)
        .filter(|s| {
            !s.is_empty()
                && s.lines()
                    .any(|line| !line.trim().is_empty() && !line.trim_start().starts_with("--"))
        })
        .collect()
}
