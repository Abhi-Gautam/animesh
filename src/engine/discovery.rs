//! Low-priority discovery deadlines. Idle work waits on revisions or a timer.

use crate::library::service::Library;
use crate::store::command_center;
use std::{sync::Arc, time::Duration};
use tokio::sync::watch;

pub async fn run(library: Arc<Library>, mut shutdown: watch::Receiver<bool>) {
    let mut changes = library.subscribe_changes();
    loop {
        changes.borrow_and_update();
        if *shutdown.borrow() {
            return;
        }
        let next = library.store().read(command_center::next_discovery).await;
        let delay = match next {
            Ok(Some((feed, deadline))) if deadline.get() <= library.now().get() => {
                match library.refresh_discovery(feed).await {
                    Ok(()) => continue,
                    Err(_) => Duration::from_secs(60),
                }
            }
            Ok(Some((_, deadline))) => {
                Duration::from_secs(library.now().seconds_until(deadline).unsigned_abs())
                    .min(super::MAX_SLEEP)
            }
            _ => super::MAX_SLEEP,
        };
        tokio::select! {
            _ = tokio::time::sleep(delay) => {},
            _ = changes.changed() => {},
            _ = shutdown.changed() => {},
        }
    }
}
