//! The loop that fires notifications where the OS will not hold a schedule.
//!
//! On macOS the notification centre owns the timing: a request is handed over
//! once and fires without this process. Where [`NotificationSurface::holds_schedule`]
//! is false there is nothing to hand the schedule to, so the daemon keeps it —
//! it wakes at the next airtime, reconciles, and sleeps again.
//!
//! [`NotificationSurface::holds_schedule`]: super::reconciler::NotificationSurface::holds_schedule

use std::sync::Arc;
use std::time::Duration;

use tokio::sync::watch;

use crate::library::service::Library;

use super::reconciler::Reconciler;

/// How often a pass runs with nothing asking for one.
///
/// A backstop for a schedule that changed without waking us, not the mechanism.
pub const SAFETY_TICK: Duration = Duration::from_secs(15 * 60);

/// Smallest gap between passes.
///
/// A deadline already in the past that a pass did not clear — missing
/// authorization, or a job past its grace window waiting to be expired by the
/// next projection — would otherwise re-evaluate continuously.
pub const MIN_SLEEP: Duration = Duration::from_secs(60);

/// Reconciles until `shutdown` reports true.
pub async fn run(
    library: Arc<Library>,
    reconciler: Arc<Reconciler>,
    mut shutdown: watch::Receiver<bool>,
) {
    loop {
        if *shutdown.borrow() {
            break;
        }

        match reconciler.settle().await {
            Ok(report) if !report.is_quiet() => tracing::info!(
                submitted = report.submitted,
                delivered = report.delivered,
                waiting = report.waiting,
                failed = report.failed,
                "reconciled"
            ),
            Ok(_) => {}
            Err(error) => tracing::warn!(error = %error.message, "reconciliation failed"),
        }

        let sleep_for = next_sleep(&library).await;
        tokio::select! {
            _ = tokio::time::sleep(sleep_for) => {}
            _ = shutdown.changed() => {}
        }
    }
}

/// How long to wait before the next pass.
///
/// The next airtime, floored so a stuck job cannot spin and capped by the
/// safety tick so a schedule that moved without waking us is still picked up.
async fn next_sleep(library: &Library) -> Duration {
    let Ok(Some(next)) = library.earliest_notification().await else {
        return SAFETY_TICK;
    };

    let seconds = library.now().seconds_until(next);
    if seconds <= 0 {
        return MIN_SLEEP;
    }
    SAFETY_TICK.min(Duration::from_secs(seconds.unsigned_abs()))
}
