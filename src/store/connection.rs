//! Connection configuration, the writer thread, and the reader pool.
//!
//! WAL gives one writer and many concurrent readers, so the structure mirrors
//! that exactly: a single owned write connection on its own thread, and a
//! bounded pool of read connections. Serializing writes in Rust rather than
//! letting connections contend means `SQLITE_BUSY` is a symptom of an external
//! process, not of our own concurrency.

use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use rusqlite::Connection;
use thiserror::Error;
use tokio::sync::{mpsc, oneshot, Semaphore};

use crate::error::{AppError, ErrorCode};
use crate::paths;

/// How long SQLite waits on a locked database before returning `SQLITE_BUSY`.
pub const BUSY_TIMEOUT_MS: u32 = 5_000;

/// Concurrent read connections.
///
/// Reads are short and the only clients are one CLI process and the menu, so a
/// deep pool would buy nothing but file descriptors.
pub const READER_POOL_SIZE: usize = 4;

#[derive(Debug, Error)]
pub enum StoreError {
    #[error("sqlite: {0}")]
    Sqlite(#[from] rusqlite::Error),

    #[error("pragma {name}: set to {expected:?} but read back {actual:?}")]
    PragmaMismatch {
        name: &'static str,
        expected: String,
        actual: String,
    },

    #[error(
        "database schema is at version {found}, newer than this build understands ({supported})"
    )]
    SchemaTooNew { found: i64, supported: i64 },

    #[error(
        "migration {name} has changed since it was applied (checksum {applied} -> {embedded})"
    )]
    ChecksumMismatch {
        name: String,
        applied: String,
        embedded: String,
    },

    #[error("integrity check failed: {0}")]
    Integrity(String),

    #[error("the writer thread is gone")]
    WriterGone,

    #[error("path: {0}")]
    Path(#[from] paths::PathError),
}

impl StoreError {
    /// Maps onto the stable code a surface will see.
    pub fn code(&self) -> ErrorCode {
        match self {
            Self::SchemaTooNew { .. } => ErrorCode::SchemaTooNew,
            Self::Integrity(_) | Self::ChecksumMismatch { .. } => ErrorCode::DatabaseCorrupt,
            Self::PragmaMismatch { .. } => ErrorCode::Internal,
            Self::WriterGone | Self::Path(_) => ErrorCode::Internal,
            Self::Sqlite(error) => match error.sqlite_error_code() {
                Some(rusqlite::ErrorCode::DatabaseBusy | rusqlite::ErrorCode::DatabaseLocked) => {
                    ErrorCode::DatabaseBusy
                }
                Some(rusqlite::ErrorCode::ReadOnly) => ErrorCode::DatabaseReadOnly,
                Some(rusqlite::ErrorCode::DatabaseCorrupt | rusqlite::ErrorCode::NotADatabase) => {
                    ErrorCode::DatabaseCorrupt
                }
                _ => ErrorCode::Internal,
            },
        }
    }

    /// Whether this failure is permanent enough to hold the app in a degraded
    /// state rather than exiting for launchd to restart.
    pub fn is_permanent(&self) -> bool {
        matches!(
            self.code(),
            ErrorCode::SchemaTooNew | ErrorCode::DatabaseCorrupt | ErrorCode::DatabaseReadOnly
        )
    }
}

impl From<StoreError> for AppError {
    fn from(value: StoreError) -> Self {
        Self::new(value.code(), value.to_string())
    }
}

/// Applies the durability pragmas and proves each one took effect.
///
/// A pragma that silently fails to apply is worse than one that errors: the
/// database would look configured while quietly running without foreign keys or
/// without an fsync barrier. Every one is therefore read back.
pub fn configure(conn: &Connection, writable: bool) -> Result<(), StoreError> {
    // WAL is a property of the database file, so only the writer needs to set
    // it; readers inherit it.
    if writable {
        let mode: String = conn.query_row("PRAGMA journal_mode = WAL", [], |row| row.get(0))?;
        if !mode.eq_ignore_ascii_case("wal") {
            return Err(StoreError::PragmaMismatch {
                name: "journal_mode",
                expected: "wal".into(),
                actual: mode,
            });
        }
    }

    conn.pragma_update(None, "foreign_keys", "ON")?;
    expect_int(conn, "foreign_keys", 1)?;

    conn.pragma_update(None, "synchronous", "FULL")?;
    expect_int(conn, "synchronous", 2)?;

    // macOS-specific: without it, fsync is a hint the drive may reorder around.
    conn.pragma_update(None, "fullfsync", "ON")?;
    expect_int(conn, "fullfsync", 1)?;

    conn.busy_handler(None)?;
    conn.pragma_update(None, "busy_timeout", BUSY_TIMEOUT_MS)?;
    expect_int(conn, "busy_timeout", i64::from(BUSY_TIMEOUT_MS))?;

    Ok(())
}

fn expect_int(conn: &Connection, name: &'static str, expected: i64) -> Result<(), StoreError> {
    let actual: i64 = conn.query_row(&format!("PRAGMA {name}"), [], |row| row.get(0))?;
    if actual == expected {
        Ok(())
    } else {
        Err(StoreError::PragmaMismatch {
            name,
            expected: expected.to_string(),
            actual: actual.to_string(),
        })
    }
}

/// Opens and configures one connection.
pub fn open(path: &Path, writable: bool) -> Result<Connection, StoreError> {
    let conn = if writable {
        Connection::open(path)?
    } else {
        Connection::open_with_flags(
            path,
            rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY | rusqlite::OpenFlags::SQLITE_OPEN_URI,
        )?
    };
    configure(&conn, writable)?;
    Ok(conn)
}

/// Runs the SQLite integrity and foreign-key checks.
///
/// O(database), so it runs after a migration or a detected unclean shutdown,
/// never on every clean start.
pub fn verify_integrity(conn: &Connection) -> Result<(), StoreError> {
    let result: String = conn.query_row("PRAGMA integrity_check", [], |row| row.get(0))?;
    if !result.eq_ignore_ascii_case("ok") {
        return Err(StoreError::Integrity(result));
    }

    let mut stmt = conn.prepare("PRAGMA foreign_key_check")?;
    let mut rows = stmt.query([])?;
    if let Some(row) = rows.next()? {
        let table: String = row.get(0).unwrap_or_else(|_| "<unknown>".into());
        return Err(StoreError::Integrity(format!(
            "foreign key violation in {table}"
        )));
    }
    Ok(())
}

type WriterJob = Box<dyn FnOnce(&mut Connection) + Send>;

/// The single write connection, owned by a dedicated thread.
#[derive(Debug)]
pub struct Writer {
    tx: mpsc::UnboundedSender<WriterJob>,
}

impl Writer {
    fn spawn(mut conn: Connection) -> Self {
        let (tx, mut rx) = mpsc::unbounded_channel::<WriterJob>();
        std::thread::Builder::new()
            .name("animesh-store-writer".into())
            .spawn(move || {
                while let Some(job) = rx.blocking_recv() {
                    job(&mut conn);
                }
                // Checkpoint on the way out so a fresh process does not have to
                // replay a long WAL before it can answer anything.
                let _ = conn.execute_batch("PRAGMA wal_checkpoint(TRUNCATE)");
            })
            .map(|_| ())
            .ok();
        Self { tx }
    }

    /// Runs `f` on the write connection and waits for its result.
    pub async fn call<T, F>(&self, f: F) -> Result<T, StoreError>
    where
        F: FnOnce(&mut Connection) -> Result<T, StoreError> + Send + 'static,
        T: Send + 'static,
    {
        let (reply_tx, reply_rx) = oneshot::channel();
        self.tx
            .send(Box::new(move |conn| {
                let _ = reply_tx.send(f(conn));
            }))
            .map_err(|_| StoreError::WriterGone)?;
        reply_rx.await.map_err(|_| StoreError::WriterGone)?
    }

    /// Runs `f` inside a transaction that commits on `Ok` and rolls back on
    /// `Err`, so a repository cannot half-apply a mutation.
    pub async fn transaction<T, F>(&self, f: F) -> Result<T, StoreError>
    where
        F: FnOnce(&rusqlite::Transaction<'_>) -> Result<T, StoreError> + Send + 'static,
        T: Send + 'static,
    {
        self.call(move |conn| {
            let tx = conn.transaction()?;
            let value = f(&tx)?;
            tx.commit()?;
            Ok(value)
        })
        .await
    }
}

#[derive(Debug)]
struct ReaderPool {
    path: PathBuf,
    permits: Arc<Semaphore>,
    idle: Arc<Mutex<Vec<Connection>>>,
}

impl ReaderPool {
    fn new(path: PathBuf) -> Self {
        Self {
            path,
            permits: Arc::new(Semaphore::new(READER_POOL_SIZE)),
            idle: Arc::new(Mutex::new(Vec::with_capacity(READER_POOL_SIZE))),
        }
    }

    async fn call<T, F>(&self, f: F) -> Result<T, StoreError>
    where
        F: FnOnce(&Connection) -> Result<T, StoreError> + Send + 'static,
        T: Send + 'static,
    {
        let _permit = self
            .permits
            .acquire()
            .await
            .map_err(|_| StoreError::WriterGone)?;

        let pooled = self.idle.lock().ok().and_then(|mut idle| idle.pop());
        let conn = match pooled {
            Some(conn) => conn,
            None => open(&self.path, false)?,
        };

        let idle = Arc::clone(&self.idle);
        let handle = tokio::task::spawn_blocking(move || {
            let result = f(&conn);
            (conn, result)
        });

        let (conn, result) = handle.await.map_err(|_| StoreError::WriterGone)?;
        if let Ok(mut pool) = idle.lock() {
            if pool.len() < READER_POOL_SIZE {
                pool.push(conn);
            }
        }
        result
    }
}

/// The store: one writer, a bounded reader pool.
#[derive(Debug)]
pub struct Store {
    writer: Writer,
    readers: ReaderPool,
}

impl Store {
    /// Opens the database at `path`, configures it, and starts the writer.
    ///
    /// Does not migrate; the caller runs [`super::migrations::apply`] so
    /// bootstrap can report a migration failure distinctly from an open
    /// failure.
    pub fn open(path: &Path) -> Result<Self, StoreError> {
        let conn = open(path, true)?;
        if path != Path::new(":memory:") {
            paths::restrict_file(path)?;
        }
        Ok(Self {
            writer: Writer::spawn(conn),
            readers: ReaderPool::new(path.to_path_buf()),
        })
    }

    pub fn writer(&self) -> &Writer {
        &self.writer
    }

    pub async fn read<T, F>(&self, f: F) -> Result<T, StoreError>
    where
        F: FnOnce(&Connection) -> Result<T, StoreError> + Send + 'static,
        T: Send + 'static,
    {
        self.readers.call(f).await
    }

    pub async fn write<T, F>(&self, f: F) -> Result<T, StoreError>
    where
        F: FnOnce(&rusqlite::Transaction<'_>) -> Result<T, StoreError> + Send + 'static,
        T: Send + 'static,
    {
        self.writer.transaction(f).await
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_db() -> (tempfile::TempDir, PathBuf) {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("library.db");
        (dir, path)
    }

    #[test]
    fn configure_sets_and_proves_every_durability_pragma() {
        let conn = Connection::open_in_memory().expect("open");
        // An in-memory database cannot use WAL, so only the shared pragmas.
        configure(&conn, false).expect("configure");

        let foreign_keys: i64 = conn
            .query_row("PRAGMA foreign_keys", [], |row| row.get(0))
            .expect("read");
        assert_eq!(foreign_keys, 1);

        let synchronous: i64 = conn
            .query_row("PRAGMA synchronous", [], |row| row.get(0))
            .expect("read");
        assert_eq!(synchronous, 2, "synchronous must be FULL");

        let busy: i64 = conn
            .query_row("PRAGMA busy_timeout", [], |row| row.get(0))
            .expect("read");
        assert_eq!(busy, i64::from(BUSY_TIMEOUT_MS));
    }

    #[test]
    fn a_file_database_runs_in_wal_mode() {
        let (_dir, path) = temp_db();
        let conn = open(&path, true).expect("open");
        let mode: String = conn
            .query_row("PRAGMA journal_mode", [], |row| row.get(0))
            .expect("read");
        assert!(mode.eq_ignore_ascii_case("wal"), "journal mode is {mode}");
    }

    #[test]
    fn opening_restricts_the_database_file() {
        use std::os::unix::fs::PermissionsExt;

        let (_dir, path) = temp_db();
        let _store = Store::open(&path).expect("open store");
        let mode = std::fs::metadata(&path)
            .expect("metadata")
            .permissions()
            .mode()
            & 0o777;
        assert_eq!(mode, 0o600, "database is not owner-only");
    }

    #[tokio::test]
    async fn a_committed_write_is_visible_to_readers() {
        let (_dir, path) = temp_db();
        let store = Store::open(&path).expect("open store");

        store
            .write(|tx| {
                tx.execute_batch("CREATE TABLE t (v INTEGER) STRICT; INSERT INTO t VALUES (7);")?;
                Ok(())
            })
            .await
            .expect("write");

        let value: i64 = store
            .read(|conn| Ok(conn.query_row("SELECT v FROM t", [], |row| row.get(0))?))
            .await
            .expect("read");
        assert_eq!(value, 7);
    }

    #[tokio::test]
    async fn a_failing_transaction_rolls_back_entirely() {
        let (_dir, path) = temp_db();
        let store = Store::open(&path).expect("open store");
        store
            .write(|tx| Ok(tx.execute_batch("CREATE TABLE t (v INTEGER) STRICT")?))
            .await
            .expect("create");

        let result: Result<(), StoreError> = store
            .write(|tx| {
                tx.execute_batch("INSERT INTO t VALUES (1)")?;
                tx.execute_batch("INSERT INTO t VALUES (2)")?;
                // Fail after real work has happened.
                Err(StoreError::Integrity("injected".into()))
            })
            .await;
        assert!(result.is_err());

        let count: i64 = store
            .read(|conn| Ok(conn.query_row("SELECT count(*) FROM t", [], |row| row.get(0))?))
            .await
            .expect("read");
        assert_eq!(count, 0, "a failed transaction left rows behind");
    }

    #[tokio::test]
    async fn a_constraint_violation_propagates_rather_than_being_swallowed() {
        let (_dir, path) = temp_db();
        let store = Store::open(&path).expect("open store");
        store
            .write(|tx| {
                Ok(tx
                    .execute_batch("CREATE TABLE t (v INTEGER PRIMARY KEY CHECK (v > 0)) STRICT")?)
            })
            .await
            .expect("create");

        let error = store
            .write(|tx| Ok(tx.execute_batch("INSERT INTO t VALUES (0)")?))
            .await
            .expect_err("check constraint must reject");
        assert_eq!(error.code(), ErrorCode::Internal);
    }

    #[tokio::test]
    async fn concurrent_reads_share_the_bounded_pool() {
        let (_dir, path) = temp_db();
        let store = Arc::new(Store::open(&path).expect("open store"));
        store
            .write(|tx| {
                Ok(tx
                    .execute_batch("CREATE TABLE t (v INTEGER) STRICT; INSERT INTO t VALUES (1)")?)
            })
            .await
            .expect("seed");

        let mut handles = Vec::new();
        for _ in 0..16 {
            let store = Arc::clone(&store);
            handles.push(tokio::spawn(async move {
                store
                    .read(|conn| Ok(conn.query_row("SELECT v FROM t", [], |r| r.get::<_, i64>(0))?))
                    .await
            }));
        }
        for handle in handles {
            assert_eq!(handle.await.expect("join").expect("read"), 1);
        }
    }

    #[tokio::test]
    async fn readers_do_not_block_a_writer() {
        // The WAL guarantee the whole concurrency model rests on.
        let (_dir, path) = temp_db();
        let store = Store::open(&path).expect("open store");
        store
            .write(|tx| Ok(tx.execute_batch("CREATE TABLE t (v INTEGER) STRICT")?))
            .await
            .expect("create");

        let read = store
            .read(|conn| Ok(conn.query_row("SELECT count(*) FROM t", [], |r| r.get::<_, i64>(0))?));
        let write = store.write(|tx| Ok(tx.execute_batch("INSERT INTO t VALUES (1)")?));

        let (read, write) = tokio::join!(read, write);
        read.expect("read");
        write.expect("write");
    }

    #[test]
    fn integrity_check_passes_on_a_fresh_database() {
        let conn = Connection::open_in_memory().expect("open");
        configure(&conn, false).expect("configure");
        verify_integrity(&conn).expect("integrity");
    }

    #[test]
    fn integrity_check_reports_a_dangling_foreign_key() {
        let conn = Connection::open_in_memory().expect("open");
        configure(&conn, false).expect("configure");
        conn.execute_batch(
            "CREATE TABLE parent (id INTEGER PRIMARY KEY) STRICT;
             CREATE TABLE child (id INTEGER PRIMARY KEY, p INTEGER REFERENCES parent(id)) STRICT;",
        )
        .expect("schema");
        // Insert the violation with enforcement off, the way a corrupt file or
        // an older build would have left it.
        conn.pragma_update(None, "foreign_keys", "OFF")
            .expect("disable");
        conn.execute_batch("INSERT INTO child VALUES (1, 99)")
            .expect("insert");

        assert!(matches!(
            verify_integrity(&conn),
            Err(StoreError::Integrity(_))
        ));
    }

    #[test]
    fn permanent_failures_are_distinguished_from_transient_ones() {
        // Bootstrap uses this to decide between staying up degraded and exiting
        // for launchd to restart.
        assert!(StoreError::SchemaTooNew {
            found: 2,
            supported: 1
        }
        .is_permanent());
        assert!(StoreError::Integrity("bad".into()).is_permanent());
        assert!(!StoreError::WriterGone.is_permanent());
    }
}
