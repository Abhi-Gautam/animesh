//! The Library: the only layer that combines the store with a source.
//!
//! Owns every transaction boundary. No network call happens inside one — the
//! fetch completes first, and only then does a single transaction commit the
//! evidence, the observation, the projection, and the notification intent
//! together.

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;

use crate::domain::ids::{AniListId, BoundedText, InstallationUuid, MediaId, UnixTimestamp};
use crate::domain::media::{MediaObservation, MAX_TITLE_LEN};
use crate::domain::notification::{JobOutcome, OsIdentifier, NATIVE_CAPACITY};
use crate::domain::read_models::{
    AuthorizationState, BootstrapState, DegradedReason, FollowOutcome, FollowResult, FollowSummary,
    Freshness, HealthSnapshot, NotificationCounts, RefreshAccepted, RefreshDisposition,
    UpcomingRelease, AIRED_VISIBILITY_SECS, DEFAULT_UPCOMING_LIMIT,
};
use crate::domain::release::FollowState;
use crate::domain::time::{JitterSource, WallClock};
use crate::error::{AppError, ErrorCode};
use crate::ipc::protocol::PROTOCOL_VERSION;
use crate::sources::anilist::client::{AniListClient, FetchOutcome, RawResponse};
use crate::sources::anilist::parser::ItemResult;
use crate::sources::anilist::{
    decode_batch, decode_detail, decode_search, queries, BatchDecode, DetailDecode, SearchDecode,
};
use crate::store::connection::{Store, StoreError};
use crate::store::{graph, read_models, releases};

use super::reducers::{
    self, reduce_follow, reduce_notification, reduce_release, ExistingFollow, FollowPlan,
    NotificationInputs, ReleaseInputs,
};

/// Everything the Library needs, assembled once at bootstrap.
pub struct Library {
    store: Store,
    source: AniListClient,
    clock: Arc<dyn WallClock>,
    jitter: Arc<dyn JitterSource>,
    installation: InstallationUuid,
    instance_id: Arc<str>,
    started_at: UnixTimestamp,
    schema_version: i64,
    /// Bumped whenever committed data changes what a surface would display.
    data_revision: AtomicU64,
    /// Bumped whenever the effective desired notification set changes.
    ///
    /// In memory rather than in the database: nothing needs it to survive a
    /// restart, because a fresh process reconciles against OS truth on its
    /// first pass anyway.
    plan_generation: AtomicU64,
}

impl std::fmt::Debug for Library {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Library")
            .field("instance_id", &self.instance_id)
            .field("data_revision", &self.data_revision)
            .finish_non_exhaustive()
    }
}

impl Library {
    pub fn new(
        store: Store,
        source: AniListClient,
        clock: Arc<dyn WallClock>,
        jitter: Arc<dyn JitterSource>,
        installation: InstallationUuid,
        schema_version: i64,
    ) -> Self {
        let started_at = clock.now();
        Self {
            store,
            source,
            clock,
            jitter,
            installation,
            instance_id: Arc::from(uuid::Uuid::new_v4().to_string().as_str()),
            started_at,
            schema_version,
            data_revision: AtomicU64::new(0),
            plan_generation: AtomicU64::new(0),
        }
    }

    pub fn instance_id(&self) -> Arc<str> {
        Arc::clone(&self.instance_id)
    }

    pub fn data_revision(&self) -> u64 {
        self.data_revision.load(Ordering::SeqCst)
    }

    pub fn plan_generation(&self) -> u64 {
        self.plan_generation.load(Ordering::SeqCst)
    }

    pub fn now(&self) -> UnixTimestamp {
        self.clock.now()
    }

    pub(crate) fn store(&self) -> &Store {
        &self.store
    }

    // -----------------------------------------------------------------------
    // Notification reconciliation
    // -----------------------------------------------------------------------

    /// Commits everything one reconciliation pass observed.
    ///
    /// One transaction for the whole pass: a crash partway through leaves the
    /// database describing the OS as it was before, and the next pass re-reads
    /// OS truth anyway. Recording half a pass would be strictly worse than
    /// recording none of it.
    ///
    /// This never bumps the plan generation. Registration is a consequence of
    /// the desired set, not a change to it; bumping here would make every pass
    /// invalidate itself and reconcile forever.
    pub async fn record_reconciliation(
        &self,
        outcomes: Vec<JobOutcome>,
        authorization: AuthorizationState,
        error_code: Option<String>,
    ) -> Result<(), AppError> {
        let now = self.now();
        let changed = self
            .store
            .write(move |tx| {
                let mut changed = false;
                for outcome in &outcomes {
                    changed = true;
                    match outcome {
                        JobOutcome::Registered { key, revision } => {
                            releases::mark_registered(tx, key, *revision, now)?;
                        }
                        JobOutcome::Delivered { key } => {
                            releases::mark_delivered(tx, key, now)?;
                        }
                        JobOutcome::Failed {
                            key,
                            error_code,
                            retry_after,
                        } => {
                            releases::mark_failed(tx, key, error_code, *retry_after, now)?;
                        }
                    }
                }
                let surface_changed =
                    releases::record_surface(tx, authorization, error_code.as_deref(), now)?;
                Ok(changed || surface_changed)
            })
            .await?;

        if changed {
            self.bump_data();
        }
        Ok(())
    }

    // -----------------------------------------------------------------------
    // Search
    // -----------------------------------------------------------------------

    /// Transient search. Writes no evidence, because results never mutate the
    /// user graph.
    pub async fn search(
        &self,
        query: &str,
    ) -> Result<Vec<crate::domain::media::SearchCandidate>, AppError> {
        let now = self.now();
        self.check_source_available(now).await?;

        let response = self
            .source
            .post(
                &queries::search(),
                serde_json::json!({ "search": query, "perPage": queries::SEARCH_PER_PAGE }),
                now,
            )
            .await;

        self.record_source_rate_state(&response, now).await?;

        let body = self.usable_body(&response)?;
        match decode_search(&body) {
            SearchDecode::Candidates(candidates) => Ok(candidates),
            SearchDecode::GraphQl(message) => Err(AppError::new(
                ErrorCode::SourceUnavailable,
                format!("AniList rejected the search: {message}"),
            )),
            SearchDecode::Decode(message) => Err(AppError::new(
                ErrorCode::SourceUnavailable,
                format!("AniList sent an unreadable response: {message}"),
            )),
        }
    }

    // -----------------------------------------------------------------------
    // Follow
    // -----------------------------------------------------------------------

    /// Follows an AniList ID, validating it against the source when unseen.
    pub async fn follow(&self, anilist_id: AniListId) -> Result<FollowResult, AppError> {
        let now = self.now();

        let existing = self.existing_follow(anilist_id).await?;

        match reduce_follow(existing) {
            FollowPlan::FetchDetail => self.follow_with_fetch(anilist_id, now).await,
            FollowPlan::Reactivate => {
                self.follow_from_cache(anilist_id, FollowOutcome::Reactivated, now)
                    .await
            }
            FollowPlan::AlreadyActive => {
                self.follow_from_cache(anilist_id, FollowOutcome::AlreadyActive, now)
                    .await
            }
        }
    }

    async fn existing_follow(
        &self,
        anilist_id: AniListId,
    ) -> Result<Option<ExistingFollow>, AppError> {
        Ok(self
            .store
            .read(move |conn| {
                let Some(row) = graph::find_source_media(conn, anilist_id)? else {
                    return Ok(None);
                };
                let Some(state) = graph::follow_state(conn, row.media_id)? else {
                    return Ok(None);
                };
                Ok(Some(ExistingFollow {
                    state,
                    has_current_observation: row.current_observation_id.is_some(),
                }))
            })
            .await?)
    }

    /// The full path: one detail request outside any transaction, then one
    /// transaction that commits everything or nothing.
    async fn follow_with_fetch(
        &self,
        anilist_id: AniListId,
        now: UnixTimestamp,
    ) -> Result<FollowResult, AppError> {
        self.check_source_available(now).await?;

        let response = self
            .source
            .post(
                &queries::detail(),
                serde_json::json!({ "id": anilist_id.get() }),
                now,
            )
            .await;
        self.record_source_rate_state(&response, now).await?;

        let body = self.usable_body(&response)?;
        let observation = match decode_detail(anilist_id, &body) {
            DetailDecode::Observed(observation) => *observation,
            DetailDecode::NotFound => {
                return Err(AppError::not_found(format!(
                    "AniList has no anime with id {anilist_id}"
                )))
            }
            DetailDecode::Integrity {
                requested,
                returned,
            } => {
                return Err(AppError::new(
                    ErrorCode::SourceIntegrity,
                    format!("asked AniList for {requested} and it answered about {returned}"),
                ))
            }
            DetailDecode::InvalidItem(error) => {
                return Err(AppError::new(
                    ErrorCode::SourceIntegrity,
                    format!("AniList sent an unusable item: {error}"),
                ))
            }
            DetailDecode::GraphQl(message) | DetailDecode::Decode(message) => {
                return Err(AppError::new(
                    ErrorCode::SourceUnavailable,
                    format!("AniList: {message}"),
                ))
            }
        };

        let record = OwnedFetch::from_response(&response, "detail", &format!("id={anilist_id}"))
            .stamped(now, response.duration_ms);
        let installation = self.installation;
        let jitter = Arc::clone(&self.jitter);

        let media_id = self
            .store
            .write(move |tx| {
                let row = match graph::find_source_media(tx, anilist_id)? {
                    Some(row) => row,
                    None => graph::create_media(tx, anilist_id, &observation.display_title, now)?,
                };

                let fetch_id = graph::insert_fetch(tx, &record.as_record())?;
                let observation_id = graph::insert_observation(
                    tx,
                    row.source_media_id,
                    fetch_id,
                    &observation,
                    now,
                )?;
                graph::set_current_observation(tx, row.source_media_id, observation_id, now)?;
                graph::set_display_title(tx, row.media_id, &observation.display_title, now)?;
                graph::set_follow(tx, row.media_id, FollowState::Active, now)?;

                let refresh_after = reducers::next_refresh_after(
                    observation.status,
                    observation.next_airing.map(|n| n.airing_at),
                    0,
                    now,
                    row.source_media_id.get(),
                    jitter.as_ref(),
                );
                graph::record_refresh_success(tx, row.source_media_id, now, refresh_after)?;

                project(
                    tx,
                    &row,
                    observation_id,
                    &observation,
                    installation,
                    FollowState::Active,
                    now,
                )?;

                Ok(row.media_id)
            })
            .await?;

        self.bump_data();
        self.bump_plan();
        self.follow_result(media_id, anilist_id, FollowOutcome::NewlyFollowed)
            .await
    }

    /// Reactivation and already-active both commit without touching the source.
    async fn follow_from_cache(
        &self,
        anilist_id: AniListId,
        outcome: FollowOutcome,
        now: UnixTimestamp,
    ) -> Result<FollowResult, AppError> {
        let installation = self.installation;
        let media_id = self
            .store
            .write(move |tx| {
                let row = graph::find_source_media(tx, anilist_id)?.ok_or_else(|| {
                    StoreError::Integrity("follow vanished between read and write".into())
                })?;
                graph::set_follow(tx, row.media_id, FollowState::Active, now)?;
                renotify_for(tx, row.media_id, installation, FollowState::Active, now)?;
                Ok(row.media_id)
            })
            .await?;

        if outcome == FollowOutcome::Reactivated {
            self.bump_data();
            self.bump_plan();
        }
        self.follow_result(media_id, anilist_id, outcome).await
    }

    async fn follow_result(
        &self,
        media_id: MediaId,
        anilist_id: AniListId,
        outcome: FollowOutcome,
    ) -> Result<FollowResult, AppError> {
        let now = self.now();
        let summaries = self.list_follows().await?;
        let summary = summaries
            .into_iter()
            .find(|s| s.media_id == media_id)
            .ok_or_else(|| AppError::internal("the follow just committed is not readable"))?;

        let _ = now;
        Ok(FollowResult {
            media_id,
            anilist_id,
            display_title: summary.display_title,
            outcome,
            upcoming: summary.upcoming,
            last_success_at: summary.last_success_at,
        })
    }

    /// Drops a follow, cancelling its notification intent but keeping evidence.
    pub async fn drop_follow(&self, media_id: MediaId) -> Result<FollowSummary, AppError> {
        let now = self.now();
        let installation = self.installation;

        let existed = self
            .store
            .write(move |tx| {
                if graph::follow_state(tx, media_id)?.is_none() {
                    return Ok(false);
                }
                graph::set_follow(tx, media_id, FollowState::Dropped, now)?;
                renotify_for(tx, media_id, installation, FollowState::Dropped, now)?;
                Ok(true)
            })
            .await?;

        if !existed {
            return Err(AppError::not_found(format!("no followed media {media_id}")));
        }

        self.bump_data();
        self.bump_plan();

        self.list_all_summaries()
            .await?
            .into_iter()
            .find(|s| s.media_id == media_id)
            .ok_or_else(|| AppError::internal("the drop just committed is not readable"))
    }

    pub async fn list_follows(&self) -> Result<Vec<FollowSummary>, AppError> {
        Ok(self
            .list_all_summaries()
            .await?
            .into_iter()
            .filter(|s| s.state.is_active())
            .collect())
    }

    async fn list_all_summaries(&self) -> Result<Vec<FollowSummary>, AppError> {
        let now = self.now();
        Ok(self.store.read(move |conn| summaries(conn, now)).await?)
    }

    // -----------------------------------------------------------------------
    // Serving
    // -----------------------------------------------------------------------

    /// Local-only. No network, no writes.
    pub async fn upcoming(&self, limit: Option<u32>) -> Result<Vec<UpcomingRelease>, AppError> {
        let now = self.now();
        let limit = limit.unwrap_or(DEFAULT_UPCOMING_LIMIT);
        Ok(self
            .store
            .read(move |conn| read_models::upcoming(conn, now, limit, AIRED_VISIBILITY_SECS))
            .await?)
    }

    pub async fn health(&self) -> Result<HealthSnapshot, AppError> {
        let now = self.now();
        let instance_id = self.instance_id.to_string();
        let started_at = self.started_at;
        let schema_version = self.schema_version;

        Ok(self
            .store
            .read(move |conn| {
                let earliest = read_models::upcoming(conn, now, 1, 0)?.into_iter().next();
                let surface = releases::surface_state(conn)?;
                let counts = releases::counts(conn, NATIVE_CAPACITY)?;
                let blocked_until = graph::source_blocked_until(conn)?;
                Ok(HealthSnapshot {
                    process_version: crate::PROCESS_VERSION.to_owned(),
                    schema_version,
                    protocol_version: PROTOCOL_VERSION,
                    app_instance_id: instance_id,
                    started_at,
                    bootstrap: BootstrapState::Ready,
                    database_ready: true,
                    active_follows: graph::active_follow_count(conn)?,
                    earliest_upcoming: earliest,
                    last_success_at: read_models::last_success(conn)?,
                    refresh: read_models::refresh_counts(conn, now)?,
                    source_blocked_until: blocked_until,
                    notifications: counts,
                    last_reconciled_at: surface.last_reconciled_at,
                    authorization: surface.authorization,
                    authorization_observed_at: surface.authorization_observed_at,
                    degraded: degraded_reasons(surface.authorization, &counts, blocked_until),
                })
            })
            .await?)
    }

    // -----------------------------------------------------------------------
    // Refresh
    // -----------------------------------------------------------------------

    /// Runs one refresh pass over whatever is due.
    ///
    /// Returns immediately with nothing done when the source is throttled or
    /// nothing is due — an idle library must make no request and no write.
    pub async fn refresh_due(&self, budget: u32) -> Result<RefreshPass, AppError> {
        let now = self.now();

        if self.check_source_available(now).await.is_err() {
            return Ok(RefreshPass::throttled());
        }

        let due = self
            .store
            .read(move |conn| graph::due_for_refresh(conn, now, budget))
            .await?;

        if due.is_empty() {
            return Ok(RefreshPass::default());
        }

        // Claim before fetching. A response whose generation no longer matches
        // may still become evidence but must not replace projection.
        let claims = self.claim_all(&due, now).await?;
        let ids: Vec<AniListId> = due.iter().map(|row| row.anilist_id).collect();

        let response = self
            .source
            .post(
                &queries::batch(),
                serde_json::json!({
                    "ids": ids.iter().map(|i| i.get()).collect::<Vec<_>>(),
                    "perPage": ids.len(),
                }),
                now,
            )
            .await;
        self.record_source_rate_state(&response, now).await?;

        let fingerprint = format!("id_in={}", ids.len());
        let evidence = OwnedFetch::from_response(&response, "batch", &fingerprint)
            .stamped(now, response.duration_ms);

        let Some(body) = response.body.clone() else {
            return self
                .fail_all(&due, &claims, now, response.outcome.as_str())
                .await;
        };

        match decode_batch(&ids, &body) {
            BatchDecode::Items(items) => {
                self.apply_items(&due, &claims, items, evidence, now).await
            }
            // Integrity, decode, and GraphQL failures all preserve projection.
            BatchDecode::Integrity(error) => {
                self.fail_all(&due, &claims, now, &format!("integrity:{error}"))
                    .await
            }
            BatchDecode::Decode(_) => self.fail_all(&due, &claims, now, "decode").await,
            BatchDecode::GraphQl(_) => self.fail_all(&due, &claims, now, "graphql").await,
        }
    }

    async fn claim_all(
        &self,
        due: &[graph::DueRow],
        now: UnixTimestamp,
    ) -> Result<Vec<i64>, AppError> {
        let ids: Vec<crate::domain::ids::SourceMediaId> =
            due.iter().map(|row| row.source_media_id).collect();
        Ok(self
            .store
            .write(move |tx| {
                ids.iter()
                    .map(|id| graph::claim_generation(tx, *id, now))
                    .collect::<Result<Vec<_>, _>>()
            })
            .await?)
    }

    /// Applies each valid item in its own short transaction.
    ///
    /// Per item rather than per batch so one malformed or stale entry cannot
    /// abort the unrelated shows beside it.
    async fn apply_items(
        &self,
        due: &[graph::DueRow],
        claims: &[i64],
        items: std::collections::BTreeMap<AniListId, ItemResult>,
        evidence: OwnedFetch,
        now: UnixTimestamp,
    ) -> Result<RefreshPass, AppError> {
        let evidence = Arc::new(evidence);
        let mut pass = RefreshPass::default();

        for (row, generation) in due.iter().zip(claims.iter().copied()) {
            let Some(result) = items.get(&row.anilist_id) else {
                continue;
            };

            match result {
                ItemResult::Observed(observation) => {
                    let observation = (**observation).clone();
                    let row = *row;
                    let evidence = Arc::clone(&evidence);
                    let installation = self.installation;
                    let jitter = Arc::clone(&self.jitter);

                    let applied = self
                        .store
                        .write(move |tx| {
                            // Both rechecks happen inside the transaction: a
                            // drop or a newer claim that raced the fetch must
                            // win over this response.
                            if !graph::generation_is_current(tx, row.source_media_id, generation)? {
                                graph::insert_fetch(tx, &evidence.as_record())?;
                                return Ok(false);
                            }
                            let follow = graph::follow_state(tx, row.media_id)?;
                            let follow = follow.unwrap_or(FollowState::Dropped);

                            let fetch_id = graph::insert_fetch(tx, &evidence.as_record())?;
                            let observation_id = graph::insert_observation(
                                tx,
                                row.source_media_id,
                                fetch_id,
                                &observation,
                                now,
                            )?;
                            graph::set_current_observation(
                                tx,
                                row.source_media_id,
                                observation_id,
                                now,
                            )?;
                            graph::set_display_title(
                                tx,
                                row.media_id,
                                &observation.display_title,
                                now,
                            )?;

                            let refresh_after = reducers::next_refresh_after(
                                observation.status,
                                observation.next_airing.map(|n| n.airing_at),
                                0,
                                now,
                                row.source_media_id.get(),
                                jitter.as_ref(),
                            );
                            graph::record_refresh_success(
                                tx,
                                row.source_media_id,
                                now,
                                refresh_after,
                            )?;

                            let source_row = graph::SourceMediaRow {
                                source_media_id: row.source_media_id,
                                anilist_id: row.anilist_id,
                                media_id: row.media_id,
                                current_observation_id: Some(observation_id),
                            };
                            project(
                                tx,
                                &source_row,
                                observation_id,
                                &observation,
                                installation,
                                follow,
                                now,
                            )?;
                            Ok(true)
                        })
                        .await?;

                    if applied {
                        pass.applied += 1;
                    } else {
                        pass.stale += 1;
                    }
                }

                // An omitted or invalid item preserves everything and only
                // records the failure, so the schedule survives a bad response.
                ItemResult::Missing => {
                    self.record_item_failure(row, generation, now, "missing")
                        .await?;
                    pass.failed += 1;
                }
                ItemResult::Invalid(error) => {
                    self.record_item_failure(row, generation, now, &format!("item:{error}"))
                        .await?;
                    pass.failed += 1;
                }
            }
        }

        if pass.applied > 0 {
            self.bump_data();
            self.bump_plan();
        }
        Ok(pass)
    }

    async fn record_item_failure(
        &self,
        row: &graph::DueRow,
        generation: i64,
        now: UnixTimestamp,
        code: &str,
    ) -> Result<(), AppError> {
        let row = *row;
        let code = code.to_owned();
        self.store
            .write(move |tx| {
                if !graph::generation_is_current(tx, row.source_media_id, generation)? {
                    return Ok(());
                }
                let failures = graph::refresh_state(tx, row.source_media_id)?
                    .map_or(0, |s| s.consecutive_failures);
                let backoff = reducers::failure_backoff_secs(failures + 1);
                graph::record_refresh_failure(
                    tx,
                    row.source_media_id,
                    now,
                    &code,
                    now.saturating_add_secs(backoff),
                )
            })
            .await?;
        Ok(())
    }

    async fn fail_all(
        &self,
        due: &[graph::DueRow],
        claims: &[i64],
        now: UnixTimestamp,
        code: &str,
    ) -> Result<RefreshPass, AppError> {
        let mut pass = RefreshPass::default();
        for (row, generation) in due.iter().zip(claims.iter().copied()) {
            self.record_item_failure(row, generation, now, code).await?;
            pass.failed += 1;
        }
        Ok(pass)
    }

    /// The earliest moment any active follow becomes due, for the scheduler.
    pub async fn earliest_due(&self) -> Result<Option<UnixTimestamp>, AppError> {
        Ok(self.store.read(graph::earliest_due).await?)
    }

    /// Runs a refresh pass now and reports what it did.
    ///
    /// Synchronous rather than a signal to the scheduler: the caller is a person
    /// who typed `animesh refresh` and is waiting, and telling them "started"
    /// while doing nothing is worse than making them wait for a bounded pass.
    pub async fn trigger_refresh(&self) -> Result<RefreshAccepted, AppError> {
        let pass = self.refresh_due(crate::engine::REFRESH_BATCH).await?;
        Ok(RefreshAccepted {
            disposition: if pass.throttled {
                RefreshDisposition::AlreadyRunning
            } else {
                RefreshDisposition::Started
            },
            accepted_at: self.now(),
            data_revision: self.data_revision(),
        })
    }

    // -----------------------------------------------------------------------
    // Source plumbing
    // -----------------------------------------------------------------------

    /// Refuses to call the source while the persisted throttle is in force.
    ///
    /// The check reads the database rather than memory, so a 429 survives a
    /// restart instead of being forgotten with the process.
    async fn check_source_available(&self, now: UnixTimestamp) -> Result<(), AppError> {
        let blocked = self.store.read(graph::source_blocked_until).await?;

        match blocked {
            Some(deadline) if deadline.get() > now.get() => {
                let wait = u32::try_from(now.seconds_until(deadline)).unwrap_or(u32::MAX);
                Err(AppError::new(
                    ErrorCode::SourceRateLimited,
                    "AniList is rate limiting requests",
                )
                .with_retry_after(wait))
            }
            _ => Ok(()),
        }
    }

    /// Persists whatever the response said about the rate limit, including on
    /// failure, before the result is interpreted.
    async fn record_source_rate_state(
        &self,
        response: &RawResponse,
        now: UnixTimestamp,
    ) -> Result<(), AppError> {
        let rate_limited = response.http_status == Some(429);
        let blocked_until = rate_limited.then(|| {
            let wait = i64::from(response.retry_after_secs.unwrap_or(60));
            now.saturating_add_secs(wait)
        });

        if blocked_until.is_none()
            && response.rate_limit_remaining.is_none()
            && response.rate_limit_reset_at.is_none()
        {
            // Nothing to record, so nothing is written. Idle paths must not
            // produce recurring writes.
            return Ok(());
        }

        let remaining = response.rate_limit_remaining;
        let reset_at = response.rate_limit_reset_at;
        self.store
            .write(move |tx| {
                graph::set_source_throttle(tx, blocked_until, remaining, reset_at, now)
            })
            .await?;
        Ok(())
    }

    fn usable_body(&self, response: &RawResponse) -> Result<String, AppError> {
        if response.http_status == Some(429) {
            return Err(AppError::new(
                ErrorCode::SourceRateLimited,
                "AniList is rate limiting requests",
            )
            .with_retry_after(response.retry_after_secs.unwrap_or(60)));
        }

        match (&response.body, response.outcome) {
            (Some(body), _) => Ok(body.clone()),
            (None, FetchOutcome::Timeout) => Err(AppError::new(
                ErrorCode::SourceUnavailable,
                "AniList did not respond in time",
            )),
            (None, FetchOutcome::TooLarge) => Err(AppError::new(
                ErrorCode::SourceIntegrity,
                "AniList sent an oversized response",
            )),
            (None, _) => Err(AppError::new(
                ErrorCode::SourceUnavailable,
                "could not reach AniList",
            )),
        }
    }

    fn bump_data(&self) {
        self.data_revision.fetch_add(1, Ordering::SeqCst);
    }

    fn bump_plan(&self) {
        self.plan_generation.fetch_add(1, Ordering::SeqCst);
    }
}

/// What one refresh pass did.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct RefreshPass {
    pub applied: u32,
    /// Responses that arrived after a newer claim superseded them.
    pub stale: u32,
    pub failed: u32,
    /// True when the pass did nothing because the source is throttled.
    pub throttled: bool,
}

impl RefreshPass {
    fn throttled() -> Self {
        Self {
            throttled: true,
            ..Self::default()
        }
    }

    /// Whether the pass touched the source at all.
    pub const fn did_work(&self) -> bool {
        self.applied > 0 || self.stale > 0 || self.failed > 0
    }
}

/// What the owner needs to act on, most actionable first.
///
/// Only `Denied` is reported: an authorization that was never requested is the
/// normal state before the first follow, not a fault.
fn degraded_reasons(
    authorization: AuthorizationState,
    counts: &NotificationCounts,
    blocked_until: Option<UnixTimestamp>,
) -> Vec<DegradedReason> {
    let mut reasons = Vec::new();
    if authorization == AuthorizationState::Denied {
        reasons.push(DegradedReason::NotificationsDenied);
    }
    if counts.deferred_capacity > 0 {
        reasons.push(DegradedReason::NotificationCapacityExceeded);
    }
    if blocked_until.is_some() {
        reasons.push(DegradedReason::SourceRateLimited);
    }
    reasons
}

/// Runs the release and notification reducers and writes their decisions.
fn project(
    tx: &rusqlite::Transaction<'_>,
    row: &graph::SourceMediaRow,
    observation_id: crate::domain::ids::ObservationId,
    observation: &MediaObservation,
    installation: InstallationUuid,
    follow: FollowState,
    now: UnixTimestamp,
) -> Result<(), StoreError> {
    let scheduled = releases::scheduled_event(tx, row.source_media_id)?;
    let same_key = match observation.next_airing {
        Some(next) => {
            let key = crate::domain::release::source_event_key(next.episode)
                .map_err(|e| StoreError::Integrity(e.to_string()))?;
            releases::event_by_key(tx, row.source_media_id, key.as_str())?
        }
        None => None,
    };
    let latest = releases::latest_sequence(tx, row.source_media_id)?;

    let transition = reduce_release(ReleaseInputs {
        scheduled: scheduled.as_ref(),
        same_key: same_key.as_ref(),
        latest_sequence: latest,
        observed: observation.next_airing,
        now,
    });

    let event_id = releases::apply_transition(
        tx,
        row.source_media_id,
        row.media_id,
        observation_id,
        transition,
        now,
    )?;

    let Some(event_id) = event_id else {
        return Ok(());
    };
    let Some(event) = releases::event_by_id(tx, event_id)? else {
        return Ok(());
    };

    let existing = releases::job_for_event(tx, event.id)?;
    let inputs = NotificationInputs {
        follow,
        event: &event,
        existing: existing.as_ref(),
        os_identifier: OsIdentifier::new(&installation, &event.uuid),
        title: observation.display_title.as_str(),
        anilist_id: row.anilist_id,
        now,
    };
    let decision =
        reduce_notification(&inputs).map_err(|e| StoreError::Integrity(e.to_string()))?;
    releases::apply_notification(tx, &event, &decision, now)?;
    Ok(())
}

/// Re-runs the notification reducer over a media's scheduled events.
///
/// Called when the follow state changes without new source data. Dropping and
/// re-following must be symmetric: the drop cancels the jobs, so the re-follow
/// has to revive them. Without this, re-following a title whose schedule has
/// not moved leaves its job `cancelled`, because the only other caller of the
/// reducer is the observation path and a recent follow does not refetch.
///
/// Goes through the reducer rather than writing a state directly, so there
/// stays exactly one place that decides what a follow change means for a
/// registered request.
fn renotify_for(
    tx: &rusqlite::Transaction<'_>,
    media_id: MediaId,
    installation: InstallationUuid,
    follow: FollowState,
    now: UnixTimestamp,
) -> Result<(), StoreError> {
    let mut stmt = tx.prepare(
        "SELECT re.release_event_id, sm.source_id, m.display_title
         FROM release_events re
         JOIN source_media sm ON sm.source_media_id = re.source_media_id
         JOIN media m ON m.media_id = re.media_id
         WHERE re.media_id = ?1 AND re.state = 'scheduled'",
    )?;
    let rows = stmt
        .query_map([media_id.get()], |row| {
            Ok((
                row.get::<_, i64>(0)?,
                row.get::<_, i64>(1)?,
                row.get::<_, String>(2)?,
            ))
        })?
        .collect::<Result<Vec<_>, _>>()?;
    drop(stmt);

    for (raw_event, raw_anilist, title) in rows {
        let event_id = crate::domain::ids::ReleaseEventId::new(raw_event)
            .map_err(|e| StoreError::Integrity(e.to_string()))?;
        let anilist_id =
            AniListId::new(raw_anilist).map_err(|e| StoreError::Integrity(e.to_string()))?;

        let Some(event) = releases::event_by_id(tx, event_id)? else {
            continue;
        };
        let Some(existing) = releases::job_for_event(tx, event.id)? else {
            continue;
        };

        let inputs = NotificationInputs {
            follow,
            event: &event,
            existing: Some(&existing),
            os_identifier: OsIdentifier::new(&installation, &event.uuid),
            title: &title,
            anilist_id,
            now,
        };
        let decision =
            reduce_notification(&inputs).map_err(|e| StoreError::Integrity(e.to_string()))?;
        releases::apply_notification(tx, &event, &decision, now)?;
    }
    Ok(())
}

fn summaries(
    conn: &rusqlite::Connection,
    now: UnixTimestamp,
) -> Result<Vec<FollowSummary>, StoreError> {
    let upcoming = read_models::upcoming(
        conn,
        now,
        crate::domain::read_models::MAX_UPCOMING_LIMIT,
        AIRED_VISIBILITY_SECS,
    )?;

    let mut stmt = conn.prepare(
        "SELECT f.media_id, sm.source_id, m.display_title, f.state,
                rs.last_success_at, rs.refresh_after, rs.retry_after
         FROM follows f
         JOIN media m ON m.media_id = f.media_id
         JOIN source_media sm ON sm.media_id = f.media_id
         LEFT JOIN source_refresh_state rs ON rs.source_media_id = sm.source_media_id
         ORDER BY m.display_title COLLATE NOCASE ASC, f.media_id ASC",
    )?;

    let rows = stmt
        .query_map([], |row| {
            Ok((
                row.get::<_, i64>(0)?,
                row.get::<_, i64>(1)?,
                row.get::<_, String>(2)?,
                row.get::<_, String>(3)?,
                row.get::<_, Option<i64>>(4)?,
                row.get::<_, Option<i64>>(5)?,
                row.get::<_, Option<i64>>(6)?,
            ))
        })?
        .collect::<Result<Vec<_>, _>>()?;

    rows.into_iter()
        .map(
            |(media_id, anilist_id, title, state, success, refresh_after, retry_after)| {
                let bad = |e: String| StoreError::Integrity(e);
                let media_id = MediaId::new(media_id).map_err(|e| bad(e.to_string()))?;
                let freshness = if retry_after.is_some_and(|d| d > now.get()) {
                    Freshness::BackingOff
                } else if refresh_after.is_none_or(|a| a <= now.get()) {
                    Freshness::Stale
                } else {
                    Freshness::Fresh
                };

                Ok(FollowSummary {
                    media_id,
                    anilist_id: AniListId::new(anilist_id).map_err(|e| bad(e.to_string()))?,
                    display_title: BoundedText::truncating(MAX_TITLE_LEN, &title)
                        .ok_or_else(|| bad("empty title".into()))?,
                    state: FollowState::parse(&state).map_err(|e| bad(e.to_string()))?,
                    upcoming: upcoming.iter().find(|u| u.media_id == media_id).cloned(),
                    last_success_at: success
                        .map(|s| UnixTimestamp::new(s).map_err(|e| bad(e.to_string())))
                        .transpose()?,
                    freshness,
                })
            },
        )
        .collect()
}

/// Owned copy of a response's evidence fields.
///
/// The borrowed [`graph::FetchRecord`] cannot cross into the `'static` closure
/// the writer thread requires, so the strings are owned here and re-borrowed
/// inside the transaction.
struct OwnedFetch {
    attempt_uuid: String,
    request_kind: String,
    fingerprint: String,
    requested_at: UnixTimestamp,
    completed_at: UnixTimestamp,
    outcome: String,
    http_status: Option<u16>,
    retry_after: Option<u32>,
    rate_limit_remaining: Option<u32>,
    rate_limit_reset_at: Option<i64>,
    body: Option<String>,
    error_code: Option<String>,
}

impl OwnedFetch {
    fn from_response(response: &RawResponse, kind: &str, fingerprint: &str) -> Self {
        let completed = UnixTimestamp::EPOCH;
        Self {
            attempt_uuid: uuid::Uuid::new_v4().to_string(),
            request_kind: kind.to_owned(),
            fingerprint: fingerprint.to_owned(),
            requested_at: completed,
            completed_at: completed,
            outcome: response.outcome.as_str().to_owned(),
            http_status: response.http_status,
            retry_after: response.retry_after_secs,
            rate_limit_remaining: response.rate_limit_remaining,
            rate_limit_reset_at: response.rate_limit_reset_at,
            body: response.body.clone(),
            error_code: response.error_code.clone(),
        }
    }

    fn stamped(mut self, now: UnixTimestamp, duration_ms: u64) -> Self {
        let started = now.saturating_add_secs(-(duration_ms as i64 / 1000));
        self.requested_at = started;
        self.completed_at = now;
        self
    }

    fn as_record(&self) -> graph::FetchRecord<'_> {
        graph::FetchRecord {
            attempt_uuid: &self.attempt_uuid,
            request_kind: &self.request_kind,
            request_fingerprint: &self.fingerprint,
            requested_at: self.requested_at,
            completed_at: self.completed_at,
            outcome: &self.outcome,
            http_status: self.http_status,
            retry_after: self.retry_after,
            rate_limit_remaining: self.rate_limit_remaining,
            rate_limit_reset_at: self.rate_limit_reset_at,
            body_json: self.body.as_deref(),
            error_code: self.error_code.as_deref(),
        }
    }
}
