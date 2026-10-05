//! Discovery ingestion reuses the canonical graph and release reducers.
//! Membership never changes user intent; partial/failing pages keep the last
//! complete pointer. Followed schedules always take priority.

use crate::domain::command_center::{anime_season, FeedKey, DISCOVERY_TTL};
use crate::domain::release::FollowState;
use crate::error::{AppError, ErrorCode};
use crate::sources::{anilist::queries, discovery::CatalogPage};
use crate::store::{command_center as serving, graph};

use super::service::{project, Library, OwnedFetch};

impl Library {
    pub async fn refresh_discovery(&self, feed: FeedKey) -> Result<(), AppError> {
        let _guard = self.discovery_lock.try_lock().map_err(|_| {
            AppError::new(
                ErrorCode::SourceRateLimited,
                "Another discovery refresh is running. Retry shortly.",
            )
            .with_retry_after(60)
        })?;
        let now = self.now();
        let fetched = self.fetch_discovery(feed).await;
        let pages = match fetched {
            Ok(pages) => pages,
            Err(error) => {
                let message = error.message.clone();
                let retry = i64::from(error.retry_after_secs.unwrap_or(300));
                self.store()
                    .write(move |tx| serving::discovery_failure(tx, feed, now, &message, retry))
                    .await?;
                self.bump_data();
                return Err(error);
            }
        };
        let complete = pages.iter().all(|(_, page)| page.complete);
        let installation = self.installation;
        self.store().write(move |tx| {
            let first_fetch = pages.first().map(|(fetch, _)| fetch.get());
            tx.execute("INSERT INTO discovery_snapshots (feed_key, fetch_id, generated_at, expires_at, completeness, error_code) VALUES (?1, ?2, ?3, ?4, ?5, ?6)", rusqlite::params![feed.as_str(), first_fetch, now.get(), now.get() + DISCOVERY_TTL, if complete { "complete" } else { "partial" }, if complete { None } else { Some("Some source items could not be read. The previous complete snapshot is retained.") }])?;
            let snapshot = tx.last_insert_rowid();
            let mut seen = std::collections::BTreeSet::new();
            let mut rank = 0;
            for (fetch_id, page) in pages {
                for observation in page.items {
                    if rank >= crate::domain::command_center::DISCOVERY_LIMIT || !seen.insert(observation.source_key) { continue; }
                    let row = match graph::find_source_media(tx, observation.source_key)? {
                        Some(row) => row,
                        None => graph::create_media(tx, observation.source_key, &observation.display_title, now)?,
                    };
                    let observation_id = graph::insert_observation(tx, row.source_media_id, fetch_id, &observation, now)?;
                    // Today's TV grid is not an authoritative next-episode
                    // detail response. Never overwrite a followed schedule.
                    if complete && graph::follow_state(tx, row.media_id)? != Some(FollowState::Active) {
                        graph::set_current_observation(tx, row.source_media_id, observation_id, now)?;
                        graph::set_display_title(tx, row.media_id, &observation.display_title, now)?;
                        graph::record_refresh_success(tx, row.source_media_id, now, now.saturating_add_secs(DISCOVERY_TTL))?;
                        project(tx, &row, observation_id, &observation, installation, FollowState::Dropped, now)?;
                    }
                    tx.execute("INSERT INTO discovery_members (snapshot_id, rank, source_media_id, observation_id) VALUES (?1, ?2, ?3, ?4)", rusqlite::params![snapshot, rank as i64, row.source_media_id.get(), observation_id.get()])?;
                    rank += 1;
                }
            }
            if complete {
                tx.execute("UPDATE discovery_feeds SET current_snapshot_id = ?2, refresh_after = ?3, retry_after = NULL, last_error = NULL WHERE feed_key = ?1", rusqlite::params![feed.as_str(), snapshot, now.get() + DISCOVERY_TTL])?;
            } else {
                tx.execute("UPDATE discovery_feeds SET retry_after = ?2, last_error = 'Partial source response. Previous complete snapshot retained.' WHERE feed_key = ?1", rusqlite::params![feed.as_str(), now.get() + 300])?;
            }
            // Keep the current complete snapshot and the latest three attempts.
            // Canonical observations and their referenced evidence stay intact.
            serving::prune_discovery(tx, feed)?;
            Ok(())
        }).await?;
        self.bump_data();
        if complete {
            Ok(())
        } else {
            Err(AppError::new(
                ErrorCode::SourceUnavailable,
                "Partial discovery response. Your previous complete collection is still available.",
            ))
        }
    }

    async fn fetch_discovery(
        &self,
        feed: FeedKey,
    ) -> Result<Vec<(crate::domain::ids::FetchId, CatalogPage)>, AppError> {
        let now = self.now();
        let priority = self
            .store()
            .read(move |conn| Ok(!graph::due_for_refresh(conn, now, 1)?.is_empty()))
            .await?;
        if priority {
            return Err(AppError::new(ErrorCode::SourceRateLimited, "Followed-title schedules are refreshing first. Discovery will retry automatically.").with_retry_after(60));
        }
        let reserve = self
            .store()
            .read(move |conn| serving::discovery_budget(conn, feed.source(), now))
            .await?;
        if let Some(deadline) = reserve {
            return Err(AppError::new(
                ErrorCode::SourceRateLimited,
                "The remaining source budget is reserved for followed schedules.",
            )
            .with_retry_after(
                u32::try_from(now.seconds_until(deadline))
                    .unwrap_or(60)
                    .max(60),
            ));
        }
        let responses = match feed {
            FeedKey::TvOnNow => {
                self.check_tv_available(now).await?;
                let broadcast = self.tvmaze.get("/schedule?country=US", now).await;
                self.record_tv_rate_state(&broadcast, now).await?;
                // Preserve the first response before a throttle prevents the second.
                if broadcast.http_status == Some(429) {
                    let evidence = OwnedFetch::from_response(
                        &broadcast,
                        "batch",
                        &format!("discovery:{}:broadcast", feed.as_str()),
                    )
                    .for_source(feed.source())
                    .stamped(now, broadcast.duration_ms);
                    self.store()
                        .write(move |tx| graph::insert_fetch(tx, &evidence.as_record()))
                        .await?;
                    return self.usable_tv_body(&broadcast).map(|_| Vec::new());
                }
                let web = self.tvmaze.get("/schedule/web?country=US", now).await;
                self.record_tv_rate_state(&web, now).await?;
                vec![broadcast, web]
            }
            _ => {
                self.check_source_available(now).await?;
                let (season, year) = anime_season(now);
                let response = self.source.post(&queries::discovery(feed == FeedKey::AnimeThisSeason), serde_json::json!({ "season": season, "year": year, "perPage": crate::domain::command_center::DISCOVERY_LIMIT }), now).await;
                self.record_source_rate_state(&response, now).await?;
                vec![response]
            }
        };
        let mut pages = Vec::new();
        for (index, response) in responses.into_iter().enumerate() {
            let evidence = OwnedFetch::from_response(
                &response,
                "batch",
                &format!("discovery:{}:{index}", feed.as_str()),
            )
            .for_source(feed.source())
            .stamped(now, response.duration_ms);
            let fetch_id = self
                .store()
                .write(move |tx| graph::insert_fetch(tx, &evidence.as_record()))
                .await?;
            let body = if feed == FeedKey::TvOnNow {
                self.usable_tv_body(&response)?
            } else {
                self.usable_body(&response)?
            };
            let page = if feed == FeedKey::TvOnNow {
                crate::sources::discovery::tvmaze(&body)
            } else {
                crate::sources::discovery::anilist(&body, feed, now)
            }
            .map_err(|message| AppError::new(ErrorCode::SourceUnavailable, message))?;
            pages.push((fetch_id, page));
        }
        Ok(pages)
    }
}
