//! Revisioned local queries and invalidation; no surface-specific state.

use std::sync::Arc;
use std::time::Duration;

use crate::domain::command_center::*;
use crate::error::{AppError, ErrorCode};
use crate::store::command_center as serving;

use super::service::Library;

impl Library {
    pub async fn unified_search(
        &self,
        query: &str,
        kind: Option<crate::domain::media::MediaKind>,
    ) -> Result<SearchResults, AppError> {
        use crate::domain::ids::Source;
        use crate::domain::media::MediaKind;
        let (anime, tv) = tokio::join!(
            async {
                if kind != Some(MediaKind::Tv) {
                    self.search(query).await
                } else {
                    Ok(Vec::new())
                }
            },
            async {
                if kind != Some(MediaKind::Anime) {
                    self.search_tv(query).await
                } else {
                    Ok(Vec::new())
                }
            },
        );
        let mut candidates = Vec::new();
        let mut issues = Vec::new();
        for (source, result) in [(Source::AniList, anime), (Source::TvMaze, tv)] {
            match result {
                Ok(items) => candidates.extend(items),
                Err(error) => issues.push(SourceIssue {
                    source,
                    message: error.message,
                }),
            }
        }
        let items = self.resolve_search(candidates).await?;
        Ok(SearchResults { items, issues })
    }

    /// Annotate an existing search result locally when follow state changes.
    pub async fn resolve_search(
        &self,
        candidates: Vec<crate::domain::media::SearchCandidate>,
    ) -> Result<Vec<SearchHit>, AppError> {
        use crate::domain::ids::SourceKey;
        self.store()
            .read(move |conn| {
                let tx = conn.unchecked_transaction()?;
                let hits = candidates
                    .into_iter()
                    .map(|candidate| {
                        let row = crate::store::graph::find_source_media(
                            &tx,
                            SourceKey {
                                source: candidate.source,
                                id: candidate.source_id,
                            },
                        )?;
                        let state = row
                            .as_ref()
                            .map(|row| crate::store::graph::follow_state(&tx, row.media_id))
                            .transpose()?
                            .flatten();
                        Ok(SearchHit {
                            candidate,
                            media_id: row.map(|row| row.media_id),
                            followed: state == Some(crate::domain::release::FollowState::Active),
                        })
                    })
                    .collect::<Result<Vec<_>, crate::store::connection::StoreError>>()?;
                tx.commit()?;
                Ok(hits)
            })
            .await
            .map_err(Into::into)
    }

    pub async fn view(&self, query: ViewQuery) -> Result<ViewSnapshot, AppError> {
        for _ in 0..8 {
            let stamp = self.revision_stamp();
            let (cursor, scope) = match &query {
                ViewQuery::Library {
                    kind,
                    state,
                    sort,
                    cursor,
                } => (
                    cursor.as_ref(),
                    format!("library:{kind:?}:{state:?}:{sort:?}"),
                ),
                ViewQuery::Schedule { kind, cursor } => {
                    (cursor.as_ref(), format!("schedule:{kind:?}"))
                }
                _ => (None, String::new()),
            };
            if let Some(cursor) = cursor {
                if cursor.instance_id != stamp.instance_id
                    || cursor.revision != stamp.revision
                    || cursor.scope != scope
                    || cursor.offset > u32::MAX - PAGE_SIZE
                {
                    return Err(AppError::invalid_argument(
                        "This page changed. Return to the first page.",
                    ));
                }
            }
            let context = serving::SnapshotContext {
                stamp: stamp.clone(),
                now: self.now(),
                started_at: self.started_at,
                schema_version: self.schema_version,
                protocol_version: crate::ipc::protocol::PROTOCOL_VERSION,
            };
            let query = query.clone();
            let snapshot = self
                .store()
                .read(move |conn| serving::snapshot(conn, query, context))
                .await?;
            if stamp.revision == self.data_revision() {
                return snapshot.ok_or_else(|| {
                    AppError::not_found(
                        "This title has not been ingested. Follow it from search first.",
                    )
                });
            }
        }
        Err(AppError::new(
            ErrorCode::DatabaseBusy,
            "Animesh is updating this view. Retry shortly.",
        ))
    }

    pub async fn start_operation(
        self: &Arc<Self>,
        target: RefreshTarget,
    ) -> Result<OperationStatus, AppError> {
        let operation = OperationStatus {
            id: uuid::Uuid::new_v4().to_string(),
            target,
            state: OperationState::Queued,
            updated_at: self.now(),
            message: None,
        };
        let (operation, started) = self
            .store()
            .write(move |tx| serving::begin_operation(tx, operation))
            .await?;
        if started {
            self.bump_data();
            let library = Arc::clone(self);
            let task = operation.clone();
            tokio::spawn(async move {
                if let Err(error) = library.run_operation(task).await {
                    tracing::warn!(
                        code = error.code.as_str(),
                        "refresh operation could not be recorded"
                    );
                }
            });
        }
        Ok(operation)
    }

    async fn run_operation(&self, mut operation: OperationStatus) -> Result<(), AppError> {
        operation.state = OperationState::Running;
        self.record_operation(operation.clone()).await?;
        let result = match operation.target {
            RefreshTarget::Library => {
                let pass = self.refresh_due(crate::engine::REFRESH_BATCH).await;
                pass.and_then(|pass| {
                    if pass.throttled {
                        Err(AppError::new(
                            ErrorCode::SourceRateLimited,
                            "A source asked Animesh to wait. Refresh resumes automatically.",
                        ))
                    } else if pass.failed > 0 {
                        Err(AppError::new(
                            ErrorCode::SourceUnavailable,
                            "Some titles could not refresh. Existing schedules remain available.",
                        ))
                    } else {
                        Ok(())
                    }
                })
            }
        };
        operation.state = match &result {
            Ok(_) => OperationState::Completed,
            Err(e) if e.code == ErrorCode::SourceRateLimited => OperationState::Throttled,
            Err(_) => OperationState::Failed,
        };
        operation.message = result.err().map(|e| e.message);
        operation.updated_at = self.now();
        self.record_operation(operation).await
    }

    async fn record_operation(&self, operation: OperationStatus) -> Result<(), AppError> {
        self.store()
            .write(move |tx| serving::set_operation(tx, &operation))
            .await?;
        self.bump_data();
        Ok(())
    }

    pub fn revision_stamp(&self) -> RevisionStamp {
        RevisionStamp {
            instance_id: self.instance_id().to_string(),
            revision: self.data_revision(),
        }
    }

    /// Subscribe first, then check: a commit between the client's query and
    /// this wait is already visible, and one during the check is retained.
    /// A daemon restart resets the revision's meaning and returns immediately.
    pub async fn wait_for_revision(&self, instance_id: &str, after: u64) -> RevisionStamp {
        let mut changes = self.subscribe_changes();
        if instance_id == self.instance_id().as_ref() && *changes.borrow_and_update() <= after {
            let _ = tokio::time::timeout(Duration::from_secs(20), changes.changed()).await;
        }
        self.revision_stamp()
    }
}
