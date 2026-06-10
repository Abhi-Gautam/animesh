use anyhow::{Context, Result};

use crate::ingest::budget::{failure_backoff, next_refresh_due_at};
use crate::library::Library;
use crate::sources::SourceRegistry;
use crate::store::SourceParseError;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RefreshReport {
    pub attempted: usize,
    pub succeeded: usize,
    pub failed: usize,
    pub skipped_missing_adapter: usize,
    pub failures: Vec<(String, String, String)>,
}

pub struct RefreshService<'a> {
    library: &'a Library,
    sources: &'a SourceRegistry,
}

impl<'a> RefreshService<'a> {
    pub fn new(library: &'a Library, sources: &'a SourceRegistry) -> Self {
        Self { library, sources }
    }

    pub async fn refresh_due(&self, budget: usize, now: i64) -> Result<RefreshReport> {
        let due = self
            .library
            .due_source_ref_refresh_states(budget as u32)
            .context("load due source_ref refresh states")?;
        let mut report = RefreshReport {
            attempted: 0,
            succeeded: 0,
            failed: 0,
            skipped_missing_adapter: 0,
            failures: Vec::new(),
        };

        for state in due.into_iter().take(budget) {
            let source = state.source.clone();
            let source_id = state.source_id.clone();
            let Some(adapter) = self.sources.adapter(&source) else {
                report.skipped_missing_adapter += 1;
                let reason = format!("missing source adapter {source:?}");
                let failure_count = state.failure_count + 1;
                self.library.record_source_ingest_failure(
                    &source,
                    &source_id,
                    &reason,
                    now + failure_backoff(failure_count),
                )?;
                report.failures.push((source, source_id, reason));
                continue;
            };

            report.attempted += 1;
            let payload = match adapter.ingest(&source_id, now).await {
                Ok(Some(payload)) => payload,
                Ok(None) => {
                    let reason = format!("{source} returned no detail payload for {source_id}");
                    self.record_failure(&source, &source_id, &reason, state.failure_count, now)?;
                    report.failed += 1;
                    report.failures.push((source, source_id, reason));
                    continue;
                }
                Err(err) => {
                    let reason = format!("{source} detail ingest: {err:#}");
                    self.record_failure(&source, &source_id, &reason, state.failure_count, now)?;
                    report.failed += 1;
                    report.failures.push((source, source_id, reason));
                    continue;
                }
            };

            let observation = match adapter.parser().parse_fetch(&payload) {
                Ok(Some(observation)) => observation,
                Ok(None) => {
                    self.library.store_raw_source_payload(&payload)?;
                    let reason = format!("{source} parser produced no observation for {source_id}");
                    self.record_failure(&source, &source_id, &reason, state.failure_count, now)?;
                    report.failed += 1;
                    report.failures.push((source, source_id, reason));
                    continue;
                }
                Err(err) => {
                    self.library.store_raw_source_payload(&payload)?;
                    let _ = self.library.record_source_parse_error(&SourceParseError {
                        raw_payload_id: payload.id.clone(),
                        source: payload.source.clone(),
                        endpoint: payload.endpoint.clone(),
                        error: format!("{err:#}"),
                        occurred_at: now,
                    });
                    let reason = format!("{source} detail parse: {err:#}");
                    self.record_failure(&source, &source_id, &reason, state.failure_count, now)?;
                    report.failed += 1;
                    report.failures.push((source, source_id, reason));
                    continue;
                }
            };

            let Some(canonical_id) = self
                .library
                .canonical_id_for_source_ref(&source, &source_id)?
            else {
                let reason = format!("no canonical source_ref for ({source}, {source_id})");
                self.record_failure(&source, &source_id, &reason, state.failure_count, now)?;
                report.failed += 1;
                report.failures.push((source, source_id, reason));
                continue;
            };

            let next_event_at = observation
                .release_events
                .iter()
                .filter_map(|event| event.scheduled_at)
                .filter(|scheduled_at| *scheduled_at > now)
                .min();
            let next_due_at = next_refresh_due_at(
                observation.kind,
                observation.status.as_deref(),
                next_event_at,
                now,
            );
            self.library.record_source_ingest_success(
                &canonical_id,
                &payload,
                &observation,
                next_due_at,
            )?;
            report.succeeded += 1;
        }

        Ok(report)
    }

    fn record_failure(
        &self,
        source: &str,
        source_id: &str,
        reason: &str,
        existing_failure_count: i64,
        now: i64,
    ) -> Result<()> {
        self.library.record_source_ingest_failure(
            source,
            source_id,
            reason,
            now + failure_backoff(existing_failure_count + 1),
        )
    }
}
