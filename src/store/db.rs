//! Connection handle + schema migrations.
//!
//! Owns the single point at which we touch SQLite. PRAGMA discipline
//! (WAL, NORMAL, foreign keys) is set here so every other call site
//! inherits the same crash-safety contract.

use std::path::Path;

use anyhow::{anyhow, Context, Result};
use rusqlite::Connection;

mod embedded {
    use refinery::embed_migrations;
    embed_migrations!("migrations");
}

/// Highest migration version this binary knows about. Bump alongside
/// each `Vxxxx__*.sql` file added under `migrations/`.
pub const MAX_KNOWN_VERSION: u32 = 1;

/// Owning wrapper around a rusqlite Connection. The only struct in the
/// codebase that holds a `Connection`.
pub struct Db {
    conn: Connection,
}

impl std::fmt::Debug for Db {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        // rusqlite::Connection doesn't implement Debug; report the
        // schema version instead so test failures stay readable.
        let v = self.schema_version().unwrap_or(0);
        write!(f, "Db {{ schema_version: {v} }}")
    }
}

impl Db {
    /// Open the on-disk DB. Creates parent dirs if needed. On a fresh
    /// file, runs migrations. On an existing file, refuses if the
    /// schema_version is greater than this binary supports.
    pub fn open(path: &Path) -> Result<Self> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)
                .with_context(|| format!("creating data directory {parent:?}"))?;
        }
        let conn = Connection::open(path)
            .with_context(|| format!("opening SQLite database at {path:?}"))?;
        Self::configure(&conn)?;
        let mut db = Self { conn };
        db.assert_compatible()?;
        if db.schema_version()? == 0 {
            db.run_migrations()?;
        }
        Ok(db)
    }

    /// In-memory DB for tests. Always runs migrations.
    pub fn open_in_memory() -> Result<Self> {
        let conn = Connection::open_in_memory().context("opening in-memory SQLite database")?;
        // WAL is meaningless in-memory; skip those pragmas but keep
        // foreign_keys for parity with on-disk behavior.
        conn.execute_batch("PRAGMA foreign_keys = ON;")?;
        let mut db = Self { conn };
        db.run_migrations()?;
        Ok(db)
    }

    fn configure(conn: &Connection) -> Result<()> {
        conn.execute_batch(
            "PRAGMA journal_mode = WAL;\n\
             PRAGMA synchronous = NORMAL;\n\
             PRAGMA foreign_keys = ON;",
        )
        .context("configuring SQLite pragmas")
    }

    /// Apply any pending migrations. Idempotent; safe to call multiple
    /// times.
    pub fn run_migrations(&mut self) -> Result<()> {
        embedded::migrations::runner()
            .run(&mut self.conn)
            .map(|_report| ())
            .context("running schema migrations")
    }

    /// Highest applied migration version. `0` means a fresh DB (no
    /// refinery tracking table yet).
    pub fn schema_version(&self) -> Result<u32> {
        let table_exists: bool = self.conn.query_row(
            "SELECT EXISTS(\
                SELECT 1 FROM sqlite_master \
                WHERE type = 'table' AND name = 'refinery_schema_history'\
             )",
            [],
            |row| row.get(0),
        )?;
        if !table_exists {
            return Ok(0);
        }
        let version: Option<u32> = self
            .conn
            .query_row(
                "SELECT MAX(version) FROM refinery_schema_history",
                [],
                |row| row.get(0),
            )
            .ok();
        Ok(version.unwrap_or(0))
    }

    fn assert_compatible(&self) -> Result<()> {
        let v = self.schema_version()?;
        if v > MAX_KNOWN_VERSION {
            return Err(anyhow!(
                "DB schema_version is V{v}, but this animesh binary only knows up to V{MAX_KNOWN_VERSION}. \
                 Upgrade the binary, or restore the DB from a backup. \
                 (The DB will not be modified.)"
            ));
        }
        Ok(())
    }

    /// Access for module-internal callers (CRUD modules in T13–T15).
    pub(crate) fn conn(&self) -> &Connection {
        &self.conn
    }

    pub(crate) fn conn_mut(&mut self) -> &mut Connection {
        &mut self.conn
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    #[test]
    fn in_memory_db_migrates_to_max_known_version() {
        let db = Db::open_in_memory().unwrap();
        assert_eq!(db.schema_version().unwrap(), MAX_KNOWN_VERSION);
    }

    #[test]
    fn fresh_on_disk_db_runs_migrations() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("library.db");
        let db = Db::open(&path).unwrap();
        assert_eq!(db.schema_version().unwrap(), MAX_KNOWN_VERSION);
        // The file should exist after open.
        assert!(path.exists(), "DB file not created at {path:?}");
    }

    #[test]
    fn open_creates_missing_parent_directories() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("nested").join("further").join("library.db");
        let db = Db::open(&path);
        assert!(db.is_ok(), "open() should create parent dirs: {db:?}");
        assert!(path.exists());
    }

    #[test]
    fn reopening_existing_db_is_idempotent() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("library.db");
        // First open: fresh — runs migrations.
        let v1 = Db::open(&path).unwrap().schema_version().unwrap();
        // Second open: existing — must not regress or re-migrate harmfully.
        let v2 = Db::open(&path).unwrap().schema_version().unwrap();
        assert_eq!(v1, v2);
        assert_eq!(v2, MAX_KNOWN_VERSION);
    }

    #[test]
    fn refuses_db_from_unknown_future_version() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("library.db");
        // Plant a refinery_schema_history with a version beyond what
        // we know. We mimic refinery's table shape just well enough for
        // schema_version() to read it.
        {
            let conn = Connection::open(&path).unwrap();
            conn.execute_batch(
                "CREATE TABLE refinery_schema_history (\
                    version INTEGER PRIMARY KEY,\
                    name TEXT,\
                    applied_on TEXT,\
                    checksum TEXT\
                 );\
                 INSERT INTO refinery_schema_history VALUES (999, 'fake_future', '', '');",
            )
            .unwrap();
        }
        let err = Db::open(&path).expect_err("expected refusal");
        let msg = format!("{err:#}");
        assert!(
            msg.contains("V999") && msg.contains("Upgrade the binary"),
            "error did not mention future version + remediation: {msg}"
        );
    }

    #[test]
    fn migrations_create_all_expected_tables() {
        let db = Db::open_in_memory().unwrap();
        let names: Vec<String> = {
            let conn = db.conn();
            let mut stmt = conn
                .prepare(
                    "SELECT name FROM sqlite_master \
                     WHERE type IN ('table','view') AND name NOT LIKE 'sqlite_%' \
                     ORDER BY name",
                )
                .unwrap();
            let rows = stmt.query_map([], |row| row.get::<_, String>(0)).unwrap();
            rows.collect::<Result<_, _>>().unwrap()
        };
        // Expect: tracked_item, metadata_cache, search_fts (+ its
        // FTS5 shadow tables), kv, refinery_schema_history.
        for required in [
            "tracked_item",
            "metadata_cache",
            "search_fts",
            "kv",
            "refinery_schema_history",
        ] {
            assert!(
                names.iter().any(|n| n == required),
                "missing table {required} in {names:?}"
            );
        }
    }

    #[test]
    fn fts_triggers_keep_search_fts_in_sync_with_metadata_cache() {
        let db = Db::open_in_memory().unwrap();
        let conn = db.conn();
        conn.execute(
            "INSERT INTO metadata_cache \
             (source, source_id, display_title, title_english, title_native, fetched_at, expires_at) \
             VALUES ('anilist', '21', 'One Piece', 'One Piece', 'ワンピース', 0, 0)",
            [],
        )
        .unwrap();
        let hit: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM search_fts WHERE search_fts MATCH 'piece'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(hit, 1);
        // Update should re-sync. Replace every title field so the old
        // "piece" term should disappear from the index entirely.
        conn.execute(
            "UPDATE metadata_cache \
             SET display_title = 'Wan Pisu', title_english = 'Wan Pisu', title_native = 'わんぴす' \
             WHERE source_id = '21'",
            [],
        )
        .unwrap();
        let stale: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM search_fts WHERE search_fts MATCH 'piece'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(stale, 0);
        let fresh: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM search_fts WHERE search_fts MATCH 'wan'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(fresh, 1);
        // Delete should clear.
        conn.execute("DELETE FROM metadata_cache WHERE source_id = '21'", [])
            .unwrap();
        let gone: i64 = conn
            .query_row("SELECT COUNT(*) FROM search_fts", [], |row| row.get(0))
            .unwrap();
        assert_eq!(gone, 0);
    }
}
