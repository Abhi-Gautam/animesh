//! Reconciling the desired notification plan against what macOS actually holds.
//!
//! Platform-neutral by construction: everything Apple-specific sits behind
//! [`NotificationSurface`], so the state machine is testable without a running
//! notification centre and V4 only has to supply an adapter.
//!
//! The pass never trusts its own bookkeeping about registration. It reads the
//! OS pending and delivered lists first and treats them as authoritative, which
//! is what lets a crash at any point heal on the next pass instead of needing a
//! speculative in-flight record.

use std::future::Future;
use std::pin::Pin;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;

use crate::domain::ids::UnixTimestamp;
use crate::domain::notification::{JobOutcome, NativeRequest, NotificationKey, OsIdentifier};
use crate::domain::read_models::AuthorizationState;
use crate::error::AppError;
use crate::library::notification_plan::SelectedPlan;
use crate::library::reducers::failure_backoff_secs;
use crate::library::service::Library;

/// How many times one settle loop will rerun before giving up until the next
/// trigger.
///
/// A plan that keeps moving faster than a pass completes is a live library, not
/// an error; stopping and waiting for the next signal is better than spinning.
const MAX_PASSES: u32 = 4;

pub type SurfaceFuture<'a, T> = Pin<Box<dyn Future<Output = Result<T, SurfaceError>> + Send + 'a>>;

/// A request the OS reports as pending.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PendingRequest {
    pub os_identifier: String,
    /// The canonical request JSON the adapter stashed when it submitted.
    ///
    /// `None` when the OS is holding a request this build cannot account for —
    /// an older format, or one written by a previous version. Treated as a
    /// mismatch so it gets replaced, which is safe because replacement under
    /// the same identifier is idempotent.
    pub request_json: Option<String>,
}

#[derive(Debug, thiserror::Error)]
pub enum SurfaceError {
    /// Worth retrying on a backoff.
    #[error("{0}")]
    Transient(String),
    /// Will not succeed until the owner changes something.
    #[error("{0}")]
    Permanent(String),
}

impl SurfaceError {
    fn code(&self) -> &'static str {
        match self {
            Self::Transient(_) => "surface_transient",
            Self::Permanent(_) => "surface_permanent",
        }
    }
}

/// The OS notification centre, as the engine needs it.
///
/// Boxed futures rather than `async fn` because the adapter bridges Apple
/// completion handlers, and the reconciler must be usable through `dyn` from a
/// single-threaded AppKit composition root.
pub trait NotificationSurface: Send + Sync {
    /// Whether the OS will hold a request until a future instant and can be
    /// asked what it is holding.
    ///
    /// True on macOS: a `UNCalendarNotificationTrigger` fires without this
    /// process, and `pending` reports it back, so the schedule is durable
    /// outside animesh.
    ///
    /// False for a surface that fires on submit and keeps no pending list — the
    /// freedesktop spec is the case in hand. Three things follow, and the
    /// reconciler applies all of them: a request may only be submitted once its
    /// airtime has arrived, submitting *is* delivering because there is no
    /// later observation that could confirm it, and the daemon has to be alive
    /// at airtime or the job falls to the catch-up window and then expires.
    fn holds_schedule(&self) -> bool {
        true
    }

    fn authorization(&self) -> SurfaceFuture<'_, AuthorizationState>;
    fn pending(&self) -> SurfaceFuture<'_, Vec<PendingRequest>>;
    fn delivered(&self) -> SurfaceFuture<'_, Vec<String>>;
    fn add<'a>(&'a self, request: &'a NativeRequest) -> SurfaceFuture<'a, ()>;
    fn remove(&self, identifiers: Vec<String>) -> SurfaceFuture<'_, ()>;
}

/// What one pass did.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct PassReport {
    pub generation: u64,
    /// Submitted to the OS, either new or replacing a stale request.
    pub submitted: u32,
    /// Already present at the desired revision; no OS call made.
    pub confirmed: u32,
    pub delivered: u32,
    pub failed: u32,
    pub removed: u32,
    /// Skipped because their failure backoff had not expired.
    pub backing_off: u32,
    /// Wanted, airtime still ahead, on a surface that cannot hold a schedule.
    ///
    /// Not a problem and not deferred work: these are the jobs the daemon will
    /// fire itself when the moment arrives.
    pub waiting: u32,
    pub deferred: u32,
    pub authorization: AuthorizationState,
    /// Set when the pass could not proceed at all.
    pub blocked: Option<String>,
}

impl PassReport {
    /// Whether the pass changed nothing, which is the steady state.
    pub fn is_quiet(&self) -> bool {
        self.submitted == 0 && self.delivered == 0 && self.failed == 0 && self.removed == 0
    }
}

/// The serialized reconciliation actor.
///
/// Serialization is the whole point: at most one pass may have outstanding
/// Apple callbacks, because two passes racing would both read the same pending
/// list and both decide to add the same request.
pub struct Reconciler {
    library: Arc<Library>,
    surface: Arc<dyn NotificationSurface>,
    /// Bumped by every pass that changed registration or authorization.
    ///
    /// Separate from the plan generation so menu rendering can refresh on
    /// status movement without that movement looking like a new desired set.
    status_revision: AtomicU64,
}

impl std::fmt::Debug for Reconciler {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Reconciler")
            .field("status_revision", &self.status_revision)
            .finish_non_exhaustive()
    }
}

impl Reconciler {
    pub fn new(library: Arc<Library>, surface: Arc<dyn NotificationSurface>) -> Self {
        Self {
            library,
            surface,
            status_revision: AtomicU64::new(0),
        }
    }

    pub fn status_revision(&self) -> u64 {
        self.status_revision.load(Ordering::SeqCst)
    }

    /// Runs passes until the plan stops moving underneath them.
    ///
    /// Callers do not mark work pending; every trigger — launch, data change,
    /// wake, activation, menu open, the safety tick — calls this directly, and
    /// serialization comes from the single surface task rather than a flag.
    pub async fn settle(&self) -> Result<PassReport, AppError> {
        let mut report = PassReport::default();
        for _ in 0..MAX_PASSES {
            report = self.run_pass().await?;
            if self.library.plan_generation() == report.generation {
                return Ok(report);
            }
        }
        Ok(report)
    }

    /// One pass: observe, converge, record.
    pub async fn run_pass(&self) -> Result<PassReport, AppError> {
        let authorization = match self.surface.authorization().await {
            Ok(state) => state,
            Err(error) => return self.abort(AuthorizationState::Unknown, &error).await,
        };

        let plan = self.library.selected_plan().await?;
        let mut report = PassReport {
            generation: plan.generation,
            deferred: plan.deferred,
            authorization,
            ..PassReport::default()
        };

        // Jobs stay `desired` so they register the moment authorization
        // arrives. Attempting without it would only produce one failure per job
        // per pass forever, and the prompt is raised deliberately by the menu,
        // never as a side effect of reconciling.
        if !authorization.permits_registration() {
            let (reason, code) = match authorization {
                AuthorizationState::Denied => ("notifications are denied", Some("denied")),
                _ => ("notifications are not authorized yet", None),
            };
            report.blocked = Some(reason.to_owned());
            self.library
                .record_reconciliation(Vec::new(), authorization, code.map(str::to_owned))
                .await?;
            self.bump_status();
            return Ok(report);
        }

        let pending = match self.surface.pending().await {
            Ok(pending) => pending,
            Err(error) => return self.abort(authorization, &error).await,
        };
        let delivered = match self.surface.delivered().await {
            Ok(delivered) => delivered,
            Err(error) => return self.abort(authorization, &error).await,
        };

        let outcomes = self
            .converge(&plan, &pending, &delivered, &mut report)
            .await;

        // Removal comes last, after every add and replacement has been
        // attempted. Removing first would leave a window in which the OS holds
        // nothing for an episode that is about to air.
        report.removed = self.remove_undesired(&plan, &pending).await;

        self.library
            .record_reconciliation(outcomes, authorization, None)
            .await?;
        if !report.is_quiet() {
            self.bump_status();
        }
        Ok(report)
    }

    async fn converge(
        &self,
        plan: &SelectedPlan,
        pending: &[PendingRequest],
        delivered: &[String],
        report: &mut PassReport,
    ) -> Vec<JobOutcome> {
        let now = plan.computed_at;
        let holds_schedule = self.surface.holds_schedule();
        let mut outcomes = Vec::new();

        for item in &plan.items {
            let identifier = item.request.os_identifier.as_str();

            if delivered.iter().any(|id| id == identifier) {
                report.delivered += 1;
                outcomes.push(JobOutcome::Delivered {
                    key: item.key.clone(),
                });
                continue;
            }

            if self.is_current_in_os(item, pending) {
                report.confirmed += 1;
                outcomes.push(JobOutcome::Registered {
                    key: item.key.clone(),
                    revision: item.desired_revision,
                });
                continue;
            }

            if !item.is_attemptable(now) {
                report.backing_off += 1;
                continue;
            }

            // Submitting to a surface that fires immediately would put tonight's
            // banner on screen this afternoon. Wait for the moment instead; the
            // notifier loop wakes for it.
            if !holds_schedule && item.request.fire_at.get() > now.get() {
                report.waiting += 1;
                continue;
            }

            match self.surface.add(&item.request).await {
                Ok(()) => {
                    report.submitted += 1;
                    outcomes.push(if holds_schedule {
                        JobOutcome::Registered {
                            key: item.key.clone(),
                            revision: item.desired_revision,
                        }
                    } else {
                        // The banner is on screen and no pending list will ever
                        // report it back, so the submission is the observation.
                        // `Delivered` is terminal, which is exactly right: it is
                        // what stops a second banner for the same episode.
                        JobOutcome::Delivered {
                            key: item.key.clone(),
                        }
                    });
                }
                Err(error) => {
                    report.failed += 1;
                    outcomes.push(self.failure(&item.key, &error, now));
                }
            }
        }
        outcomes
    }

    /// Whether the OS already holds exactly what this item wants.
    ///
    /// Compares the canonical JSON, not just the identifier: a rescheduled
    /// episode keeps its identifier, so an identifier match alone would report
    /// a stale trigger as correctly registered.
    fn is_current_in_os(
        &self,
        item: &crate::domain::notification::NotificationPlanItem,
        pending: &[PendingRequest],
    ) -> bool {
        let Ok(desired) = item.request.canonical_json() else {
            return false;
        };
        pending.iter().any(|request| {
            request.os_identifier == item.request.os_identifier.as_str()
                && request.request_json.as_deref() == Some(desired.as_str())
        })
    }

    async fn remove_undesired(&self, plan: &SelectedPlan, pending: &[PendingRequest]) -> u32 {
        let desired: Vec<&str> = plan
            .items
            .iter()
            .map(|item| item.request.os_identifier.as_str())
            .collect();

        let stale: Vec<String> = pending
            .iter()
            .filter(|request| OsIdentifier::is_owned(&request.os_identifier))
            .filter(|request| !desired.contains(&request.os_identifier.as_str()))
            .map(|request| request.os_identifier.clone())
            .collect();

        if stale.is_empty() {
            return 0;
        }
        let count = u32::try_from(stale.len()).unwrap_or(u32::MAX);
        match self.surface.remove(stale).await {
            Ok(()) => count,
            // A failed removal leaves a request the OS will fire for an episode
            // Animesh no longer tracks. Noisy, not wrong, and the next pass
            // retries it — so it does not fail the whole pass.
            Err(error) => {
                tracing::warn!(error = %error, "could not remove stale notification requests");
                0
            }
        }
    }

    fn failure(
        &self,
        key: &NotificationKey,
        error: &SurfaceError,
        now: UnixTimestamp,
    ) -> JobOutcome {
        // Attempt count lives in the database, so the backoff step is derived
        // from it there. This is the first step; repeated failures walk it up
        // because `mark_failed` increments before the next pass reads it.
        let delay = failure_backoff_secs(1);
        JobOutcome::Failed {
            key: key.clone(),
            error_code: error.code().to_owned(),
            retry_after: now.saturating_add_secs(delay),
        }
    }

    /// Records that the surface itself is unreachable and stops.
    ///
    /// Nothing was observed, so nothing about individual jobs may be written.
    async fn abort(
        &self,
        authorization: AuthorizationState,
        error: &SurfaceError,
    ) -> Result<PassReport, AppError> {
        self.library
            .record_reconciliation(Vec::new(), authorization, Some(error.code().to_owned()))
            .await?;
        self.bump_status();
        Ok(PassReport {
            generation: self.library.plan_generation(),
            authorization,
            blocked: Some(error.to_string()),
            ..PassReport::default()
        })
    }

    fn bump_status(&self) {
        self.status_revision.fetch_add(1, Ordering::SeqCst);
    }
}
