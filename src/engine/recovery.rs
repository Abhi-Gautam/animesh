//! Offline recovery composition: own the daemon lock before touching its storage.
use crate::paths::AppPaths;
use crate::store::connection::StoreError;
use std::time::{Duration, Instant};

pub fn offline(paths: &AppPaths, action: &str, argument: &str) -> Result<String, StoreError> {
    paths.ensure_dirs()?;
    let deadline = Instant::now() + Duration::from_secs(10);
    let _lock = loop {
        match crate::ipc::endpoint::SingletonLock::acquire(&paths.lock()) {
            Ok(lock) => break lock,
            Err(crate::ipc::endpoint::IpcError::AlreadyRunning) if Instant::now() < deadline => {
                std::thread::sleep(Duration::from_millis(50))
            }
            Err(_) => {
                return Err(StoreError::Io(std::io::Error::other(
                    "The library is still in use. Stop the daemon before maintenance.",
                )))
            }
        }
    };
    crate::store::recovery::operate(&paths.database(), action, argument)
}
