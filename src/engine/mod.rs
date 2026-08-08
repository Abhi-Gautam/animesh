//! The supervised runtime: the refresh scheduler and its wake signals.
//!
//! The loop blocks on a deadline or a signal, never on a tick. That is a hard
//! requirement: with nothing due there must be no wake, no request, no write.

pub mod bootstrap;
pub mod notifier;
pub mod reconciler;

use std::sync::Arc;
use std::time::Duration;

use tokio::sync::watch;

use crate::library::service::{Library, RefreshPass};

/// How many titles one pass claims. Bounded so a large library refreshes in
/// several batches rather than one request AniList would reject.
pub const REFRESH_BATCH: u32 = 25;

/// AniList serves at most 50 media per page, and `perPage` is bound to the
/// claimed id count — so the claim itself is what has to stay under the ceiling.
const _: () = assert!(REFRESH_BATCH <= 50);

/// Longest the loop will sleep in one step.
///
/// A deadline further out than this is re-evaluated on waking, so a suspended
/// laptop that misses its timer heals from durable state rather than sleeping
/// past an event by however long it was closed.
pub const MAX_SLEEP: Duration = Duration::from_secs(30 * 60);

/// Fallback delay when the due query itself fails.
const ERROR_BACKOFF: Duration = Duration::from_secs(60);

/// Coalescing wake signal.
///
/// A counter rather than a flag: several triggers between wakes collapse into
/// one pass, which is what keeps a burst of follows from queueing a burst of
/// refreshes.
#[derive(Debug, Clone)]
pub struct WakeHandle(watch::Sender<u64>);

impl WakeHandle {
    pub fn trigger(&self) {
        self.0.send_modify(|value| *value = value.wrapping_add(1));
    }
}

/// Creates the wake signal and its receiver.
pub fn wake_channel() -> (WakeHandle, watch::Receiver<u64>) {
    let (tx, rx) = watch::channel(0);
    (WakeHandle(tx), rx)
}

/// Runs refresh passes until `shutdown` reports true.
pub async fn run_scheduler(
    library: Arc<Library>,
    mut wake: watch::Receiver<u64>,
    mut shutdown: watch::Receiver<bool>,
) {
    loop {
        if *shutdown.borrow() {
            break;
        }

        let sleep_for = match library.earliest_due().await {
            Ok(Some(deadline)) => {
                let seconds = library.now().seconds_until(deadline);
                if seconds <= 0 {
                    // Due now. Run and re-evaluate rather than assuming one
                    // pass drained everything.
                    let pass = library.refresh_due(REFRESH_BATCH).await;
                    if should_pause(&pass) {
                        MAX_SLEEP.min(ERROR_BACKOFF)
                    } else {
                        continue;
                    }
                } else {
                    MAX_SLEEP.min(Duration::from_secs(seconds.unsigned_abs()))
                }
            }
            // Nothing scheduled at all: block on a signal, with no timer.
            Ok(None) => {
                tokio::select! {
                    _ = wake.changed() => {}
                    _ = shutdown.changed() => {}
                }
                continue;
            }
            Err(_) => ERROR_BACKOFF,
        };

        tokio::select! {
            _ = tokio::time::sleep(sleep_for) => {}
            _ = wake.changed() => {}
            _ = shutdown.changed() => {}
        }
    }
}

/// Whether a completed pass means the loop should wait rather than immediately
/// re-evaluate.
///
/// A throttled or wholly failed pass leaves rows due, so continuing would spin
/// against a source that has already said no.
fn should_pause(pass: &Result<RefreshPass, crate::error::AppError>) -> bool {
    match pass {
        Ok(pass) => pass.throttled || (pass.applied == 0 && pass.failed > 0),
        Err(_) => true,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn wake_signals_coalesce() {
        // Several triggers between wakes must collapse into one pass.
        let (handle, rx) = wake_channel();
        handle.trigger();
        handle.trigger();
        handle.trigger();
        assert_eq!(*rx.borrow(), 3, "the counter should advance per trigger");
    }

    #[test]
    fn a_throttled_pass_pauses_the_loop() {
        // Continuing would spin against a source that already said no.
        let throttled = Ok(RefreshPass {
            throttled: true,
            ..RefreshPass::default()
        });
        assert!(should_pause(&throttled));
    }

    #[test]
    fn a_wholly_failed_pass_pauses_the_loop() {
        let failed = Ok(RefreshPass {
            failed: 3,
            ..RefreshPass::default()
        });
        assert!(should_pause(&failed));
    }

    #[test]
    fn a_productive_pass_lets_the_loop_continue() {
        let applied = Ok(RefreshPass {
            applied: 2,
            ..RefreshPass::default()
        });
        assert!(!should_pause(&applied));
    }

    #[test]
    fn a_query_failure_pauses_rather_than_spinning() {
        let error = Err(crate::error::AppError::internal("database gone"));
        assert!(should_pause(&error));
    }

    #[test]
    fn the_sleep_ceiling_bounds_a_distant_deadline() {
        // A wake in seven days must still re-check sooner, because a suspended
        // laptop's timer cannot be trusted to fire on schedule.
        let week = Duration::from_secs(7 * 24 * 3600);
        assert_eq!(MAX_SLEEP.min(week), MAX_SLEEP);
    }
}
