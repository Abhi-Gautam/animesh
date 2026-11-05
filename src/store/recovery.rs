//! Consistent, bounded backups and offline recovery under the daemon's singleton lock.
use super::{
    connection::{verify_integrity, StoreError},
    migrations,
};
use crate::paths;
use rusqlite::{
    backup::{Backup, StepResult},
    Connection, OpenFlags,
};
use std::fs::{self, OpenOptions};
use std::os::unix::fs::OpenOptionsExt;
use std::path::Path;
use std::time::{Duration, Instant, SystemTime};

fn io(error: std::io::Error) -> StoreError {
    StoreError::Io(error)
}

fn copy_database(from: &Connection, to: &mut Connection) -> Result<(), StoreError> {
    let deadline = Instant::now() + Duration::from_secs(30);
    let backup = Backup::new(from, to)?;
    loop {
        if matches!(backup.step(128)?, StepResult::Done) {
            break;
        }
        if Instant::now() >= deadline {
            return Err(StoreError::Integrity(
                "backup timed out; original database is unchanged".into(),
            ));
        }
        std::thread::sleep(Duration::from_millis(5));
    }
    Ok(())
}

pub fn snapshot(conn: &Connection, database: &Path, slot: &str) -> Result<(), StoreError> {
    if !matches!(slot, "daily" | "manual" | "pre-upgrade" | "before-restore") {
        return Err(StoreError::Integrity("unknown backup slot".into()));
    }
    let parent = database
        .parent()
        .ok_or_else(|| StoreError::Integrity("database has no parent".into()))?;
    let directory = parent.join("backups");
    paths::private_dir(&directory).map_err(io)?;
    let directory = directory.canonicalize().map_err(io)?;
    let available = rustix::fs::statvfs(&directory).map_err(|e| io(e.into()))?;
    let pages: i64 = conn.query_row("PRAGMA page_count", [], |r| r.get(0))?;
    let page_size: i64 = conn.query_row("PRAGMA page_size", [], |r| r.get(0))?;
    let needed = u64::try_from(pages.saturating_mul(page_size))
        .unwrap_or(u64::MAX)
        .saturating_add(8 * 1024 * 1024);
    if available.f_bavail.saturating_mul(available.f_frsize) < needed {
        return Err(io(std::io::Error::other(
            "not enough free space for a consistent backup",
        )));
    }
    let target = directory.join(format!("{slot}.db"));
    paths::check_file(&target).map_err(io)?;
    let temporary = directory.join(format!(".{}.db", uuid::Uuid::new_v4()));
    let result = (|| {
        OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(0o600)
            .open(&temporary)
            .map_err(io)?;
        let mut destination = Connection::open_with_flags(
            &temporary,
            OpenFlags::SQLITE_OPEN_READ_WRITE | OpenFlags::SQLITE_OPEN_NOFOLLOW,
        )?;
        copy_database(conn, &mut destination)?;
        verify_integrity(&destination)?;
        drop(destination);
        fs::File::open(&temporary)
            .and_then(|f| f.sync_all())
            .map_err(io)?;
        fs::rename(&temporary, &target).map_err(io)?;
        fs::File::open(&directory)
            .and_then(|f| f.sync_all())
            .map_err(io)?;
        Ok(())
    })();
    if result.is_err() {
        let _ = fs::remove_file(temporary);
    }
    result
}

pub fn daily(conn: &Connection, database: &Path) -> Result<(), StoreError> {
    let Some(parent) = database.parent() else {
        return Ok(());
    };
    let file = parent.join("backups/daily.db");
    paths::check_file(&file).map_err(io)?;
    let recent = fs::metadata(&file)
        .and_then(|m| m.modified())
        .ok()
        .and_then(|at| SystemTime::now().duration_since(at).ok())
        .is_some_and(|age| age < Duration::from_secs(86400));
    if !recent {
        snapshot(conn, database, "daily")?;
    }
    Ok(())
}

/// Called only with the singleton lock held and no running writer/read pool.
pub fn restore(database: &Path, slot: &str) -> Result<(), StoreError> {
    if !matches!(slot, "daily" | "manual" | "pre-upgrade") {
        return Err(StoreError::Integrity("unknown restore slot".into()));
    }
    paths::check_parent(database)
        .and_then(|_| paths::check_file(database))
        .map_err(io)?;
    let parent = database
        .parent()
        .ok_or_else(|| StoreError::Integrity("database has no parent".into()))?
        .canonicalize()
        .map_err(io)?;
    let database = parent.join(
        database
            .file_name()
            .ok_or_else(|| std::io::Error::other("missing database filename"))?,
    );
    let source_path = parent.join("backups").join(format!("{slot}.db"));
    paths::check_parent(&source_path)
        .and_then(|_| paths::check_file(&source_path))
        .map_err(io)?;
    let source = Connection::open_with_flags(
        source_path.canonicalize().map_err(io)?,
        OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_NOFOLLOW,
    )?;
    verify_integrity(&source)?;
    let version: i64 = source.query_row("SELECT MAX(version) FROM schema_migrations", [], |r| {
        r.get(0)
    })?;
    if version > migrations::supported_version() {
        return Err(StoreError::SchemaTooNew {
            found: version,
            supported: migrations::supported_version(),
        });
    }
    let stage = parent.join(format!(".restore-{}.db", uuid::Uuid::new_v4()));
    let result = (|| {
        OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(0o600)
            .open(&stage)
            .map_err(io)?;
        let mut candidate = Connection::open_with_flags(
            &stage,
            OpenFlags::SQLITE_OPEN_READ_WRITE | OpenFlags::SQLITE_OPEN_NOFOLLOW,
        )?;
        copy_database(&source, &mut candidate)?;
        migrations::apply(&mut candidate, chrono::Utc::now().timestamp())?;
        verify_integrity(&candidate)?;
        candidate.pragma_update(None, "journal_mode", "DELETE")?;
        drop(candidate);
        fs::File::open(&stage)
            .and_then(|f| f.sync_all())
            .map_err(io)?;
        // Back up a healthy original. Damaged bytes and sidecars are quarantined
        // instead, so restoring still works when SQLite cannot open the original.
        if database.exists() {
            if let Ok(original) = super::connection::open(&database, true) {
                if verify_integrity(&original).is_ok() {
                    snapshot(&original, &database, "before-restore")?;
                }
                let busy: i64 =
                    original.query_row("PRAGMA wal_checkpoint(TRUNCATE)", [], |r| r.get(0))?;
                if busy != 0 {
                    return Err(io(std::io::Error::other(
                        "another database reader is preventing restore",
                    )));
                }
            }
        }
        let quarantine = parent.join("recovery-original");
        paths::private_dir(&quarantine).map_err(io)?;
        let basename = database
            .file_name()
            .ok_or_else(|| std::io::Error::other("missing filename"))?
            .to_string_lossy();
        for suffix in ["", "-wal", "-shm"] {
            let original = parent.join(format!("{basename}{suffix}"));
            let saved = quarantine.join(format!("{basename}{suffix}"));
            paths::check_file(&original)
                .and_then(|_| paths::check_file(&saved))
                .map_err(io)?;
            if original.exists() {
                let temp = quarantine.join(format!(".{}.tmp", uuid::Uuid::new_v4()));
                let result = (|| {
                    let mut from = OpenOptions::new()
                        .read(true)
                        .custom_flags(rustix::fs::OFlags::NOFOLLOW.bits() as i32)
                        .open(&original)?;
                    let mut to = OpenOptions::new()
                        .write(true)
                        .create_new(true)
                        .mode(0o600)
                        .open(&temp)?;
                    std::io::copy(&mut from, &mut to)?;
                    to.sync_all()?;
                    fs::rename(&temp, &saved)
                })();
                if let Err(error) = result {
                    let _ = fs::remove_file(temp);
                    return Err(io(error));
                }
            }
        }
        fs::File::open(&quarantine)
            .and_then(|f| f.sync_all())
            .map_err(io)?;
        // A checkpointed healthy original stays in place until the atomic swap.
        // Sidecars from a corrupt original are retained in recovery-original.
        for suffix in ["-wal", "-shm"] {
            let sidecar = parent.join(format!("{basename}{suffix}"));
            match fs::remove_file(sidecar) {
                Ok(()) => {}
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
                Err(e) => return Err(io(e)),
            }
        }
        fs::rename(&stage, &database).map_err(io)?;
        fs::File::open(&parent)
            .and_then(|f| f.sync_all())
            .map_err(io)?;
        Ok(())
    })();
    if result.is_err() {
        let _ = fs::remove_file(&stage);
    }
    result
}

pub fn erase_backups(database: &Path) -> Result<(), StoreError> {
    let Some(parent) = database.parent() else {
        return Ok(());
    };
    let directory = parent.join("backups");
    if directory.exists() {
        paths::private_dir(&directory).map_err(io)?;
    }
    let quarantine = parent.join("recovery-original");
    if quarantine.exists() {
        paths::private_dir(&quarantine).map_err(io)?;
        for entry in fs::read_dir(&quarantine).map_err(io)? {
            let file = entry.map_err(io)?.path();
            paths::check_file(&file).map_err(io)?;
            fs::remove_file(file).map_err(io)?;
        }
    }
    if !directory.exists() {
        return Ok(());
    }
    for entry in fs::read_dir(&directory).map_err(io)? {
        let file = entry.map_err(io)?.path();
        paths::check_file(&file).map_err(io)?;
        fs::remove_file(file).map_err(io)?;
    }
    Ok(())
}

/// Administrative storage operations. The composition root must hold the daemon lock.
pub fn operate(database: &Path, action: &str, argument: &str) -> Result<String, StoreError> {
    cleanup_staging(database)?;
    if action == "restore" {
        restore(database, argument)?;
        return Ok(
            "Restored the verified library backup. Original recovery bytes were preserved.".into(),
        );
    }
    let mut conn = super::connection::open(database, true)?;
    migrations::apply(&mut conn, chrono::Utc::now().timestamp())?;
    verify_integrity(&conn)?;
    match action {
        "backup" => {
            snapshot(&conn, database, "manual")?;
            Ok(format!(
                "Verified library backup: {}",
                database
                    .parent()
                    .unwrap_or(Path::new("."))
                    .join("backups/manual.db")
                    .display()
            ))
        }
        "forget" => {
            let id = argument
                .parse::<crate::domain::ids::MediaId>()
                .map_err(|e| StoreError::Integrity(e.to_string()))?;
            if conn.query_row(
                "SELECT count(*) FROM media WHERE media_id=?1",
                [id.get()],
                |r| r.get::<_, i64>(0),
            )? == 0
            {
                return Err(StoreError::Integrity(
                    "That local title does not exist; nothing was erased".into(),
                ));
            }
            erase_backups(database)?;
            let tx = conn.transaction()?;
            let existed = super::maintenance::forget(&tx, id.get())?;
            tx.commit()?;
            conn.execute_batch(
                "PRAGMA wal_checkpoint(TRUNCATE); VACUUM; PRAGMA wal_checkpoint(TRUNCATE);",
            )?;
            if !existed {
                return Err(StoreError::Integrity(
                    "That local title does not exist".into(),
                ));
            }
            Ok("Erased the title's local history, raw evidence and app-managed backups. Pending Mac reminders are removed when the service resumes. The OS may retain delivered notifications; external backups and existing assistant chats are separate.".into())
        }
        "notifications" if matches!(argument, "private" | "titles") => {
            conn.execute(
                "UPDATE app_preferences SET private_notifications=?1 WHERE singleton_id=1",
                [argument == "private"],
            )?;
            Ok(format!("Notification banners now use {} mode.", argument))
        }
        "storage" => {
            let bytes: i64 = conn.query_row(
                "SELECT COALESCE(SUM(byte_length),0) FROM source_fetches",
                [],
                |r| r.get(0),
            )?;
            let database_bytes = fs::metadata(database).map_err(io)?.len();
            Ok(format!("Database: {database_bytes} bytes (256 MiB maximum). Raw evidence: {bytes} bytes (32 MiB budget, 30 days). Backups: up to four verified snapshots in {}. Dropped titles retain history until explicitly forgotten.",database.parent().unwrap_or(Path::new(".")).join("backups").display()))
        }
        _ => Err(StoreError::Integrity(
            "unknown library maintenance command".into(),
        )),
    }
}

/// Removes only UUID-named work files left by interrupted snapshot/restore operations.
/// Caller owns the daemon lock; no current operation can still use these files.
pub fn cleanup_staging(database: &Path) -> Result<(), StoreError> {
    let Some(parent) = database.parent() else {
        return Ok(());
    };
    for (directory, restore) in [
        (parent.to_path_buf(), true),
        (parent.join("backups"), false),
    ] {
        if !directory.exists() {
            continue;
        }
        paths::private_dir(&directory).map_err(io)?;
        for entry in fs::read_dir(&directory).map_err(io)? {
            let file = entry.map_err(io)?.path();
            let Some(name) = file.file_name().and_then(|name| name.to_str()) else {
                continue;
            };
            let stem = name
                .trim_end_matches("-journal")
                .trim_end_matches("-wal")
                .trim_end_matches("-shm")
                .trim_end_matches(".db");
            let id = if restore {
                stem.strip_prefix(".restore-")
            } else {
                stem.strip_prefix('.')
            };
            if id.is_some_and(|id| uuid::Uuid::parse_str(id).is_ok()) {
                paths::check_file(&file).map_err(io)?;
                fs::remove_file(file).map_err(io)?;
            }
        }
    }
    Ok(())
}
