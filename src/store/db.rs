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
pub(crate) const MAX_KNOWN_VERSION: u32 = 7;

/// Owning wrapper around a rusqlite Connection. The only struct in the
/// codebase that holds a `Connection`.
pub(crate) struct Db {
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
    pub(crate) fn open(path: &Path) -> Result<Self> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)
                .with_context(|| format!("creating data directory {parent:?}"))?;
        }
        let conn = Connection::open(path)
            .with_context(|| format!("opening SQLite database at {path:?}"))?;
        Self::configure(&conn)?;
        let mut db = Self { conn };
        db.assert_compatible()?;
        db.run_migrations()?;
        Ok(db)
    }

    /// In-memory DB for tests. Always runs migrations.
    #[cfg(test)]
    pub(crate) fn open_in_memory() -> Result<Self> {
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
    pub(crate) fn run_migrations(&mut self) -> Result<()> {
        embedded::migrations::runner()
            .run(&mut self.conn)
            .map(|_report| ())
            .context("running schema migrations")
    }

    /// Highest applied migration version. `0` means a fresh DB (no
    /// refinery tracking table yet).
    pub(crate) fn schema_version(&self) -> Result<u32> {
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
        // V0005 dropped tracked_item + watch_progress. The canonical
        // substrate (V0004) + the metadata cache + kv + FTS index
        // remain.
        for required in [
            "metadata_cache",
            "search_fts",
            "kv",
            "refinery_schema_history",
            "canonical_release",
            "source_ref",
            "engagement",
            "canonicalization_cache",
            "source_search_cache",
            "source_ref_refresh_state",
            "canonical_schedule_event",
        ] {
            assert!(
                names.iter().any(|n| n == required),
                "missing table {required} in {names:?}"
            );
        }
        for forbidden in ["tracked_item", "watch_progress"] {
            assert!(
                !names.iter().any(|n| n == forbidden),
                "legacy table {forbidden} should be dropped by V0005, still in {names:?}"
            );
        }
    }

    // V0004 data-migration tests. Build a V0003-state DB by running the
    // legacy migrations directly, plant legacy rows, then run V0004 and
    // verify the canonical tables hold the expected projection. This
    // does not go through refinery for V0004 so we can isolate the
    // migration's data-shape contract from the runner.
    fn build_v3_then_run_v4() -> Connection {
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch("PRAGMA foreign_keys = ON;").unwrap();
        conn.execute_batch(include_str!("../../migrations/V0001__initial.sql"))
            .unwrap();
        conn.execute_batch(include_str!("../../migrations/V0002__sp1_5_tui.sql"))
            .unwrap();
        conn.execute_batch(include_str!("../../migrations/V0003__cover_ascii.sql"))
            .unwrap();
        conn
    }

    fn apply_v4(conn: &Connection) {
        conn.execute_batch(include_str!("../../migrations/V0004__canonical_schema.sql"))
            .unwrap();
    }

    #[test]
    fn v0004_backfills_canonical_release_from_tracked_item() {
        let conn = build_v3_then_run_v4();
        conn.execute(
            "INSERT INTO tracked_item \
             (source, source_id, kind, display_title, followed_at, dropped_at, user_note, cover_ascii, cover_color) \
             VALUES \
             ('anilist', '21', 'anime', 'One Piece', 1000, NULL, 'pirate king', 'ascii-art-1', '#ff0000'), \
             ('anilist', '99', 'anime', 'Dropped Show', 500, 800, NULL, NULL, NULL)",
            [],
        )
        .unwrap();

        apply_v4(&conn);

        let canonical_count: i64 = conn
            .query_row("SELECT COUNT(*) FROM canonical_release", [], |r| r.get(0))
            .unwrap();
        assert_eq!(canonical_count, 2);

        // Active follow preserves all fields, including cover.
        type CanonicalRow = (
            String,
            String,
            String,
            Option<String>,
            Option<String>,
            Option<i64>,
            Option<i64>,
            Option<String>,
        );
        let (id, kind, title, cover_ascii, cover_color, followed_at, dropped_at, user_note): CanonicalRow = conn
            .query_row(
                "SELECT id, kind, display_title, cover_ascii, cover_color, \
                        followed_at, dropped_at, user_note \
                 FROM canonical_release \
                 WHERE display_title = 'One Piece'",
                [],
                |r| {
                    Ok((
                        r.get(0)?,
                        r.get(1)?,
                        r.get(2)?,
                        r.get(3)?,
                        r.get(4)?,
                        r.get(5)?,
                        r.get(6)?,
                        r.get(7)?,
                    ))
                },
            )
            .unwrap();
        assert_eq!(kind, "anime");
        assert_eq!(title, "One Piece");
        assert_eq!(cover_ascii.as_deref(), Some("ascii-art-1"));
        assert_eq!(cover_color.as_deref(), Some("#ff0000"));
        assert_eq!(followed_at, Some(1000));
        assert_eq!(dropped_at, None);
        assert_eq!(user_note.as_deref(), Some("pirate king"));
        // The synthesized id must be deterministic and embed source+source_id.
        assert!(
            id.starts_with("release:anime:legacy-anilist-21"),
            "unexpected canonical id: {id}"
        );

        // Dropped row's dropped_at must come through.
        let dropped_at_for_99: Option<i64> = conn
            .query_row(
                "SELECT dropped_at FROM canonical_release \
                 WHERE id = 'release:anime:legacy-anilist-99'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(dropped_at_for_99, Some(800));
    }

    #[test]
    fn v0004_backfills_source_ref_with_full_confidence_for_legacy_rows() {
        let conn = build_v3_then_run_v4();
        conn.execute(
            "INSERT INTO tracked_item (source, source_id, kind, display_title, followed_at) \
             VALUES ('anilist', '21', 'anime', 'One Piece', 1000)",
            [],
        )
        .unwrap();
        apply_v4(&conn);

        let (canonical_id, raw_title, confidence): (String, String, f64) = conn
            .query_row(
                "SELECT canonical_id, raw_title, confidence FROM source_ref \
                 WHERE source = 'anilist' AND source_id = '21'",
                [],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
            )
            .unwrap();
        assert_eq!(canonical_id, "release:anime:legacy-anilist-21");
        assert_eq!(raw_title, "One Piece");
        // Legacy rows are user-verified by virtue of being followed.
        assert!((confidence - 1.0).abs() < f64::EPSILON);
    }

    #[test]
    fn v0004_migrates_watch_progress_into_engagement() {
        let conn = build_v3_then_run_v4();
        conn.execute(
            "INSERT INTO tracked_item (source, source_id, kind, display_title, followed_at) \
             VALUES ('anilist', '21', 'anime', 'One Piece', 1000)",
            [],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO watch_progress (source, source_id, seen, updated_at) \
             VALUES ('anilist', '21', 7, 1500)",
            [],
        )
        .unwrap();
        apply_v4(&conn);

        let (canonical_id, event, occurred_at, meta): (String, String, i64, Option<String>) = conn
            .query_row(
                "SELECT canonical_id, event, occurred_at, meta FROM engagement \
                 ORDER BY id DESC LIMIT 1",
                [],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)),
            )
            .unwrap();
        assert_eq!(canonical_id, "release:anime:legacy-anilist-21");
        assert_eq!(event, "completed");
        assert_eq!(occurred_at, 1500);
        // meta carries the original seen count as JSON.
        let meta_str = meta.expect("engagement meta should be present");
        assert!(
            meta_str.contains("\"seen\"") && meta_str.contains("7"),
            "meta should carry seen count, got: {meta_str}"
        );
    }

    #[test]
    fn v0004_orphaned_watch_progress_does_not_create_engagement_rows() {
        let conn = build_v3_then_run_v4();
        // A watch_progress row with no matching tracked_item — should be
        // silently dropped, not raise an FK error.
        conn.execute(
            "INSERT INTO watch_progress (source, source_id, seen, updated_at) \
             VALUES ('orphan', 'xyz', 3, 9999)",
            [],
        )
        .unwrap();
        apply_v4(&conn);

        let count: i64 = conn
            .query_row("SELECT COUNT(*) FROM engagement", [], |r| r.get(0))
            .unwrap();
        assert_eq!(
            count, 0,
            "orphan watch_progress must not produce engagement rows"
        );
    }

    #[test]
    fn v0004_canonical_release_kind_check_constraint_rejects_unknown_kinds() {
        let conn = build_v3_then_run_v4();
        apply_v4(&conn);
        let err = conn
            .execute(
                "INSERT INTO canonical_release (id, kind, display_title, created_at) \
                 VALUES ('release:bogus:x', 'bogus', 'Bogus', 1)",
                [],
            )
            .expect_err("CHECK constraint on kind must reject unknown values");
        let msg = format!("{err}");
        assert!(
            msg.contains("CHECK"),
            "expected CHECK violation, got: {msg}"
        );
    }

    #[test]
    fn v0004_source_ref_confidence_bounds_check_constraint() {
        let conn = build_v3_then_run_v4();
        apply_v4(&conn);
        conn.execute(
            "INSERT INTO canonical_release (id, kind, display_title, created_at) \
             VALUES ('release:tv:foo', 'tv', 'Foo', 1)",
            [],
        )
        .unwrap();
        let err = conn
            .execute(
                "INSERT INTO source_ref (canonical_id, source, source_id, raw_title, confidence) \
                 VALUES ('release:tv:foo', 'tmdb', '1', 'Foo', 1.5)",
                [],
            )
            .expect_err("CHECK constraint must reject confidence > 1.0");
        assert!(format!("{err}").contains("CHECK"));
    }

    #[test]
    fn v0004_engagement_event_check_constraint_rejects_unknown_events() {
        let conn = build_v3_then_run_v4();
        apply_v4(&conn);
        conn.execute(
            "INSERT INTO canonical_release (id, kind, display_title, created_at) \
             VALUES ('release:tv:foo', 'tv', 'Foo', 1)",
            [],
        )
        .unwrap();
        let err = conn
            .execute(
                "INSERT INTO engagement (canonical_id, event, occurred_at) \
                 VALUES ('release:tv:foo', 'bogus_event', 100)",
                [],
            )
            .expect_err("CHECK constraint must reject unknown engagement event");
        assert!(format!("{err}").contains("CHECK"));
    }

    #[test]
    fn v0004_source_ref_cascades_delete_when_canonical_release_removed() {
        let conn = build_v3_then_run_v4();
        apply_v4(&conn);
        conn.execute(
            "INSERT INTO canonical_release (id, kind, display_title, created_at) \
             VALUES ('release:tv:foo', 'tv', 'Foo', 1)",
            [],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO source_ref (canonical_id, source, source_id, raw_title, confidence) \
             VALUES ('release:tv:foo', 'tmdb', '1', 'Foo', 0.9)",
            [],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO engagement (canonical_id, event, occurred_at) \
             VALUES ('release:tv:foo', 'opened', 100)",
            [],
        )
        .unwrap();

        conn.execute(
            "DELETE FROM canonical_release WHERE id = 'release:tv:foo'",
            [],
        )
        .unwrap();

        let source_refs: i64 = conn
            .query_row("SELECT COUNT(*) FROM source_ref", [], |r| r.get(0))
            .unwrap();
        let engagements: i64 = conn
            .query_row("SELECT COUNT(*) FROM engagement", [], |r| r.get(0))
            .unwrap();
        assert_eq!(
            source_refs, 0,
            "source_ref must cascade-delete with canonical_release"
        );
        assert_eq!(
            engagements, 0,
            "engagement must cascade-delete with canonical_release"
        );
    }

    #[test]
    fn v0004_runs_cleanly_on_empty_v3_database() {
        let conn = build_v3_then_run_v4();
        apply_v4(&conn);
        for table in [
            "canonical_release",
            "source_ref",
            "engagement",
            "canonicalization_cache",
        ] {
            let count: i64 = conn
                .query_row(&format!("SELECT COUNT(*) FROM {table}"), [], |r| r.get(0))
                .unwrap();
            assert_eq!(count, 0, "{table} should be empty on fresh DB");
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
