//! Migration engine: ordered, transactional schema upgrades.

use anyhow::{Context, Result};
use turso::Connection;

const MIGRATIONS: &[(i64, &str)] = &[
    (1, include_str!("../../migrations/001_watchlist.sql")),
    (
        2,
        include_str!("../../migrations/002_watchlist_next_airing.sql"),
    ),
    (3, include_str!("../../migrations/003_notifications.sql")),
];

/// Apply any pending migrations, each in its own transaction.
pub(crate) async fn migrate(conn: &mut Connection) -> Result<()> {
    conn.execute(
        "CREATE TABLE IF NOT EXISTS schema_migrations (
            version    INTEGER PRIMARY KEY NOT NULL,
            applied_at INTEGER NOT NULL
        )",
        (),
    )
    .await
    .context("create schema_migrations")?;

    for &(version, sql) in MIGRATIONS {
        if migration_applied(conn, version).await? {
            continue;
        }

        let tx = conn
            .transaction()
            .await
            .with_context(|| format!("begin transaction for migration {version}"))?;

        for stmt in split_sql_statements(sql) {
            tx.execute(stmt, ())
                .await
                .with_context(|| format!("apply migration {version}: {stmt}"))?;
        }

        let applied_at = chrono::Utc::now().timestamp();
        tx.execute(
            "INSERT INTO schema_migrations (version, applied_at) VALUES (?1, ?2)",
            turso::params![version, applied_at],
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
