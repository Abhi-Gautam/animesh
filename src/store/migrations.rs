//! Ordered, checksummed, transactional migrations.
//!
//! Immutable after release. The runner refuses to touch a database written by a
//! newer build, and refuses to proceed if an already-applied migration's text
//! has changed, because both mean the schema in the file is not the schema this
//! binary believes it is.

use rusqlite::{Connection, Transaction};

use super::connection::StoreError;

pub struct Migration {
    pub version: i64,
    pub name: &'static str,
    pub sql: &'static str,
}

impl std::fmt::Debug for Migration {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Migration")
            .field("version", &self.version)
            .field("name", &self.name)
            .finish_non_exhaustive()
    }
}

pub const MIGRATIONS: &[Migration] = &[
    Migration {
        version: 1,
        name: "V0001__daily_driver",
        sql: include_str!("../../migrations/V0001__daily_driver.sql"),
    },
    Migration {
        version: 2,
        name: "V0002__multi_source",
        sql: include_str!("../../migrations/V0002__multi_source.sql"),
    },
    Migration {
        version: 3,
        name: "V0003__command_center",
        sql: include_str!("../../migrations/V0003__command_center.sql"),
    },
    Migration {
        version: 4,
        name: "V0004__remove_discovery",
        sql: include_str!("../../migrations/V0004__remove_discovery.sql"),
    },
];

/// The newest schema version this build can operate.
pub fn supported_version() -> i64 {
    MIGRATIONS.iter().map(|m| m.version).max().unwrap_or(0)
}

/// FNV-1a over the migration text.
///
/// Detects accidental edits to released migrations, which is the actual threat:
/// nobody is forging checksums in a single-user local database, so a
/// cryptographic hash would be a dependency bought for nothing.
fn checksum(sql: &str) -> String {
    let mut hash: u64 = 0xcbf2_9ce4_8422_2325;
    for byte in sql.as_bytes() {
        hash ^= u64::from(*byte);
        hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
    }
    format!("{hash:016x}")
}

fn ensure_ledger(conn: &Connection) -> Result<(), StoreError> {
    conn.execute_batch(
        "CREATE TABLE IF NOT EXISTS schema_migrations (
            version    INTEGER PRIMARY KEY,
            name       TEXT NOT NULL,
            checksum   TEXT NOT NULL,
            applied_at INTEGER NOT NULL
        ) STRICT",
    )?;
    Ok(())
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AppliedMigration {
    pub version: i64,
    pub name: String,
    pub checksum: String,
}

pub fn applied(conn: &Connection) -> Result<Vec<AppliedMigration>, StoreError> {
    ensure_ledger(conn)?;
    let mut stmt =
        conn.prepare("SELECT version, name, checksum FROM schema_migrations ORDER BY version")?;
    let rows = stmt
        .query_map([], |row| {
            Ok(AppliedMigration {
                version: row.get(0)?,
                name: row.get(1)?,
                checksum: row.get(2)?,
            })
        })?
        .collect::<Result<Vec<_>, _>>()?;
    Ok(rows)
}

/// The schema version currently in the file.
pub fn current_version(conn: &Connection) -> Result<i64, StoreError> {
    Ok(applied(conn)?.iter().map(|m| m.version).max().unwrap_or(0))
}

/// Brings the database up to [`supported_version`].
///
/// Returns how many migrations ran, so bootstrap knows whether to spend an
/// integrity check.
pub fn apply(conn: &mut Connection, now: i64) -> Result<usize, StoreError> {
    let already = applied(conn)?;

    let highest = already.iter().map(|m| m.version).max().unwrap_or(0);
    if highest > supported_version() {
        // Downgrading would mean running new data through old code. Refuse and
        // leave the file untouched so the newer build can still open it.
        return Err(StoreError::SchemaTooNew {
            found: highest,
            supported: supported_version(),
        });
    }

    for record in &already {
        let Some(embedded) = MIGRATIONS.iter().find(|m| m.version == record.version) else {
            continue;
        };
        let expected = checksum(embedded.sql);
        if expected != record.checksum {
            return Err(StoreError::ChecksumMismatch {
                name: record.name.clone(),
                applied: record.checksum.clone(),
                embedded: expected,
            });
        }
    }

    let mut ran = 0;
    for migration in MIGRATIONS {
        if already.iter().any(|m| m.version == migration.version) {
            continue;
        }
        // V0002 rebuilds CHECK-constrained tables. PRAGMA foreign_keys is a
        // no-op inside a transaction, so it has to flip outside.
        let rebuilds = migration.version == 2;
        if rebuilds {
            conn.pragma_update(None, "foreign_keys", "OFF")?;
        }
        let tx = conn.transaction()?;
        apply_one(&tx, migration, now)?;
        tx.commit()?;
        if rebuilds {
            conn.pragma_update(None, "foreign_keys", "ON")?;
        }
        ran += 1;
    }
    Ok(ran)
}

fn apply_one(tx: &Transaction<'_>, migration: &Migration, now: i64) -> Result<(), StoreError> {
    // execute_batch runs the whole file; inside a transaction, any failing
    // statement rolls the entire migration back rather than leaving half a
    // schema behind.
    tx.execute_batch(migration.sql)?;
    tx.execute(
        "INSERT INTO schema_migrations (version, name, checksum, applied_at)
         VALUES (?1, ?2, ?3, ?4)",
        rusqlite::params![
            migration.version,
            migration.name,
            checksum(migration.sql),
            now
        ],
    )?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::store::connection::configure;

    fn db() -> Connection {
        let conn = Connection::open_in_memory().expect("open");
        configure(&conn, false).expect("configure");
        conn
    }

    #[test]
    fn apply_creates_the_schema_and_records_it() {
        let mut conn = db();
        assert_eq!(apply(&mut conn, 100).expect("apply"), 4);
        assert_eq!(current_version(&conn).expect("version"), 4);

        let count: i64 = conn
            .query_row(
                "SELECT count(*) FROM sqlite_master WHERE type='table' AND name='release_events'",
                [],
                |row| row.get(0),
            )
            .expect("query");
        assert_eq!(count, 1);
        let catalogs: i64 = conn
            .query_row(
                "SELECT count(*) FROM sqlite_master WHERE name LIKE 'discovery_%'",
                [],
                |row| row.get(0),
            )
            .expect("no catalog tables on fresh install");
        assert_eq!(catalogs, 0);
    }

    #[test]
    fn apply_is_idempotent() {
        let mut conn = db();
        assert_eq!(apply(&mut conn, 100).expect("first"), 4);
        assert_eq!(apply(&mut conn, 200).expect("second"), 0);
        assert_eq!(current_version(&conn).expect("version"), 4);
    }

    #[test]
    fn catalog_removal_preserves_follows_schedules_and_independent_lookups() {
        let mut conn = db();
        ensure_ledger(&conn).expect("ledger");
        for migration in &MIGRATIONS[..3] {
            let rebuilds = migration.version == 2;
            if rebuilds {
                conn.pragma_update(None, "foreign_keys", "OFF")
                    .expect("off");
            }
            let tx = conn.transaction().expect("transaction");
            apply_one(&tx, migration, 100).expect("old migration");
            tx.commit().expect("commit");
            if rebuilds {
                conn.pragma_update(None, "foreign_keys", "ON").expect("on");
            }
        }
        conn.execute_batch(
            "INSERT INTO source_fetches
                (fetch_id, attempt_uuid, source, request_kind, request_fingerprint,
                 requested_at, completed_at, outcome, body_json, byte_length)
             VALUES (1, 'catalog', 'anilist', 'batch', 'discovery:anime_this_season:0', 100, 100, 'success', '{}', 2),
                    (2, 'detail', 'anilist', 'detail', 'id=3', 100, 100, 'success', '{}', 2),
                    (3, 'failed', 'anilist', 'batch', 'discovery:anime_this_season:0', 100, 100, 'timeout', NULL, NULL);
             INSERT INTO media VALUES (1, 'anime', 'Active', 100, 100),
                                      (2, 'anime', 'Dropped', 100, 100),
                                      (3, 'anime', 'Opened from search', 100, 100),
                                      (4, 'anime', 'Catalog only', 100, 100),
                                      (5, 'anime', 'Catalog and detail', 100, 100);
             INSERT INTO source_media (source_media_id, source, source_id, media_id, created_at, updated_at)
             SELECT media_id, 'anilist', media_id, media_id, 100, 100 FROM media;
             INSERT INTO source_observations
                (observation_id, source_media_id, fetch_id, parser_version, observed_at, display_title, status)
             SELECT media_id, media_id, CASE WHEN media_id = 3 THEN 2 ELSE 1 END, 1, 100, display_title, 'releasing' FROM media;
             INSERT INTO source_observations
                (observation_id, source_media_id, fetch_id, parser_version, observed_at, display_title, status)
             VALUES (6, 5, 2, 1, 100, 'Independent detail', 'releasing');
             UPDATE source_media SET current_observation_id = CASE WHEN media_id = 5 THEN 6 ELSE media_id END;
             INSERT INTO follows VALUES (1, 'active', 100, 100), (2, 'dropped', 100, 100);
             INSERT INTO source_refresh_state (source_media_id, refresh_after, updated_at)
             SELECT source_media_id, 1000, 100 FROM source_media;
             INSERT INTO release_events
                (release_event_id, event_uuid, media_id, source_media_id, source_event_key,
                 sequence_number, scheduled_at, state, schedule_revision, first_observed_at,
                 last_observed_at, last_observation_id)
             SELECT media_id, 'event-' || media_id, media_id, media_id, 'episode:1', 1, 1000,
                    'scheduled', 1, 100, 100, media_id FROM media WHERE media_id IN (1, 2, 4);
             INSERT INTO notification_jobs
                (notification_key, release_event_id, os_identifier, desired_at, desired_revision,
                 desired_request_json, state, updated_at)
             VALUES ('job', 1, 'os-job', 1000, 1, '{}', 'desired', 100);
             INSERT INTO discovery_snapshots (snapshot_id, feed_key, fetch_id, generated_at, expires_at, completeness)
             VALUES (1, 'anime_this_season', 1, 100, 1000, 'complete');
             INSERT INTO discovery_members SELECT 1, media_id - 1, media_id, media_id FROM media;
             UPDATE discovery_feeds SET current_snapshot_id = 1 WHERE feed_key = 'anime_this_season';
             INSERT INTO operations VALUES
                ('catalog', '{\"kind\":\"discovery\",\"feed\":\"anime_this_season\"}', 'completed', 100, NULL),
                ('library', '{\"kind\":\"library\"}', 'completed', 100, NULL);",
        ).expect("seed old catalog and user intent");

        assert_eq!(apply(&mut conn, 200).expect("upgrade"), 1);
        let count = |sql| {
            conn.query_row(sql, [], |row| row.get::<_, i64>(0))
                .expect("count")
        };
        assert_eq!(
            count("SELECT count(*) FROM sqlite_master WHERE name LIKE 'discovery_%'"),
            0
        );
        assert_eq!(count("SELECT count(*) FROM media"), 4);
        assert_eq!(count("SELECT count(*) FROM follows"), 2);
        assert_eq!(count("SELECT count(*) FROM release_events"), 2);
        assert_eq!(count("SELECT count(*) FROM notification_jobs WHERE notification_key = 'job' AND state = 'desired'"), 1);
        assert_eq!(
            count("SELECT count(*) FROM source_observations WHERE observation_id IN (4, 5)"),
            0
        );
        assert_eq!(
            count("SELECT count(*) FROM source_observations WHERE observation_id IN (1, 2, 3, 6)"),
            4
        );
        assert_eq!(
            count("SELECT count(*) FROM source_fetches WHERE fetch_id = 3"),
            0
        );
        assert_eq!(
            count("SELECT count(*) FROM operations WHERE operation_id = 'library'"),
            1
        );
        assert_eq!(
            count("SELECT count(*) FROM operations WHERE operation_id = 'catalog'"),
            0
        );
        assert_eq!(count("SELECT count(*) FROM pragma_foreign_key_check"), 0);
        assert_eq!(apply(&mut conn, 300).expect("reopen"), 0);
    }

    #[test]
    fn a_newer_schema_is_refused_and_left_alone() {
        let mut conn = db();
        apply(&mut conn, 100).expect("apply");
        conn.execute(
            "INSERT INTO schema_migrations (version, name, checksum, applied_at)
             VALUES (99, 'V0099__future', 'deadbeef', 100)",
            [],
        )
        .expect("simulate a newer build");

        let error = apply(&mut conn, 200).expect_err("must refuse");
        assert!(matches!(
            error,
            StoreError::SchemaTooNew {
                found: 99,
                supported: 4
            }
        ));

        // The future row must still be there: refusing means not touching it.
        assert_eq!(current_version(&conn).expect("version"), 99);
    }

    #[test]
    fn an_edited_released_migration_is_refused() {
        let mut conn = db();
        apply(&mut conn, 100).expect("apply");
        conn.execute(
            "UPDATE schema_migrations SET checksum = 'tampered' WHERE version = 1",
            [],
        )
        .expect("tamper");

        assert!(matches!(
            apply(&mut conn, 200),
            Err(StoreError::ChecksumMismatch { .. })
        ));
    }

    #[test]
    fn a_failing_migration_rolls_back_completely() {
        let conn = db();
        let mut conn = conn;
        let tx = conn.transaction().expect("begin");
        let broken = Migration {
            version: 42,
            name: "V0042__broken",
            sql: "CREATE TABLE ok (v INTEGER) STRICT; CREATE TABLE ok (v INTEGER) STRICT;",
        };
        assert!(apply_one(&tx, &broken, 100).is_err());
        drop(tx);

        // Neither the half-applied table nor a ledger row survives.
        let tables: i64 = conn
            .query_row(
                "SELECT count(*) FROM sqlite_master WHERE type='table' AND name='ok'",
                [],
                |row| row.get(0),
            )
            .expect("query");
        assert_eq!(tables, 0);
    }

    #[test]
    fn checksums_are_stable_and_sensitive() {
        assert_eq!(checksum("CREATE TABLE t"), checksum("CREATE TABLE t"));
        assert_ne!(checksum("CREATE TABLE t"), checksum("CREATE TABLE u"));
        // A single whitespace change must be caught: released SQL is immutable.
        assert_ne!(checksum("CREATE TABLE t"), checksum("CREATE  TABLE t"));
    }

    #[test]
    fn migrations_are_ordered_and_uniquely_versioned() {
        let versions: Vec<i64> = MIGRATIONS.iter().map(|m| m.version).collect();
        let mut sorted = versions.clone();
        sorted.sort_unstable();
        sorted.dedup();
        assert_eq!(versions, sorted, "migrations must be ordered and unique");
        assert!(versions.iter().all(|v| *v > 0));
    }

    #[test]
    fn an_empty_database_reports_version_zero() {
        let conn = db();
        assert_eq!(current_version(&conn).expect("version"), 0);
    }
}
