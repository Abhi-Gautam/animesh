//! Transport-neutral application commands and query results.

use serde::{Deserialize, Serialize};

use crate::domain::ids::{AniListId, MediaId, TvMazeId};
use crate::domain::media::SearchCandidate;
use crate::domain::read_models::{
    FollowResult, FollowSummary, HealthSnapshot, RefreshAccepted, UpcomingRelease,
    MAX_UPCOMING_LIMIT,
};
use crate::error::AppError;

/// Longest accepted search query.
pub const MAX_QUERY_LEN: usize = 128;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", content = "data", rename_all = "snake_case")]
pub enum Request {
    Search {
        query: String,
        kind: Option<crate::domain::media::MediaKind>,
    },
    ResolveSearch {
        candidates: Vec<SearchCandidate>,
    },
    Status,
    View {
        query: crate::domain::command_center::ViewQuery,
    },
    Refresh {
        target: crate::domain::command_center::RefreshTarget,
    },
    SearchAnime {
        query: String,
    },
    FollowAnilist {
        id: AniListId,
    },
    SearchTv {
        query: Option<String>,
    },
    FollowTv {
        id: TvMazeId,
    },
    Drop {
        media_id: MediaId,
    },
    ListFollows,
    Upcoming {
        limit: Option<u32>,
        /// Just-aired rows inside the visibility window. Default is the future.
        #[serde(default)]
        dropped: bool,
    },
    TriggerRefresh,
    WaitForRevision {
        instance_id: String,
        after: u64,
    },
}

impl Request {
    /// Stable label for logs and metrics.
    pub const fn name(&self) -> &'static str {
        match self {
            Self::Search { .. } => "search",
            Self::ResolveSearch { .. } => "resolve_search",
            Self::Status => "status",
            Self::View { .. } => "view",
            Self::Refresh { .. } => "refresh",
            Self::SearchAnime { .. } => "search_anime",
            Self::FollowAnilist { .. } => "follow_anilist",
            Self::SearchTv { .. } => "search_tv",
            Self::FollowTv { .. } => "follow_tv",
            Self::Drop { .. } => "drop",
            Self::ListFollows => "list_follows",
            Self::Upcoming { .. } => "upcoming",
            Self::TriggerRefresh => "trigger_refresh",
            Self::WaitForRevision { .. } => "wait_for_revision",
        }
    }

    /// Whether this request may reach the source.
    ///
    /// Used to reject source work while bootstrap is degraded, and asserted by
    /// a test so `upcoming` can never quietly acquire a network path.
    pub const fn touches_source(&self) -> bool {
        matches!(
            self,
            Self::Search { .. }
                | Self::Refresh { .. }
                | Self::SearchAnime { .. }
                | Self::FollowAnilist { .. }
                | Self::SearchTv { .. }
                | Self::FollowTv { .. }
                | Self::TriggerRefresh
        )
    }

    /// Whether this request is answerable while bootstrap has failed.
    pub const fn served_when_degraded(&self) -> bool {
        matches!(self, Self::Status)
    }

    /// Rejects input the handler should never see.
    ///
    /// Validation lives on the protocol type so both the client and the server
    /// enforce identical rules; a client-only check is not a check.
    pub fn validate(&self) -> Result<(), AppError> {
        match self {
            Self::ResolveSearch { candidates } if candidates.len() > 30 => Err(
                AppError::invalid_argument("at most 30 search results may be resolved"),
            ),
            Self::Search { query, .. } => validate_query(query),
            Self::SearchAnime { query } => validate_query(query),
            Self::SearchTv { query: Some(query) } => validate_query(query),
            Self::SearchTv { query: None } => Ok(()),
            Self::Upcoming { limit: Some(0), .. } => {
                Err(AppError::invalid_argument("limit must be at least 1"))
            }
            Self::Upcoming {
                limit: Some(limit), ..
            } if *limit > MAX_UPCOMING_LIMIT => Err(AppError::invalid_argument(format!(
                "limit must be at most {MAX_UPCOMING_LIMIT}"
            ))),
            Self::WaitForRevision { instance_id, .. }
                if instance_id.is_empty() || instance_id.len() > 128 =>
            {
                Err(AppError::invalid_argument(
                    "invalid daemon instance identity",
                ))
            }
            _ => Ok(()),
        }
    }
}

fn validate_query(query: &str) -> Result<(), AppError> {
    let trimmed = query.trim();
    if trimmed.is_empty() {
        return Err(AppError::invalid_argument("search query is empty"));
    }
    if trimmed.chars().count() > MAX_QUERY_LEN {
        return Err(AppError::invalid_argument(format!(
            "search query is longer than {MAX_QUERY_LEN} characters"
        )));
    }
    Ok(())
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", content = "data", rename_all = "snake_case")]
pub enum Response {
    Search(crate::domain::command_center::SearchResults),
    ResolveSearch(Vec<crate::domain::command_center::SearchHit>),
    Status(Box<HealthSnapshot>),
    View(Box<crate::domain::command_center::ViewSnapshot>),
    Refresh(crate::domain::command_center::OperationStatus),
    SearchAnime(Vec<SearchCandidate>),
    FollowAnilist(Box<FollowResult>),
    SearchTv(Vec<SearchCandidate>),
    FollowTv(Box<FollowResult>),
    Drop(Box<FollowSummary>),
    ListFollows(Vec<FollowSummary>),
    Upcoming(Vec<UpcomingRelease>),
    TriggerRefresh(RefreshAccepted),
    WaitForRevision(crate::domain::command_center::RevisionStamp),
}

impl Response {
    pub const fn name(&self) -> &'static str {
        match self {
            Self::Search(_) => "search",
            Self::ResolveSearch(_) => "resolve_search",
            Self::Status(_) => "status",
            Self::View(_) => "view",
            Self::Refresh(_) => "refresh",
            Self::SearchAnime(_) => "search_anime",
            Self::FollowAnilist(_) => "follow_anilist",
            Self::SearchTv(_) => "search_tv",
            Self::FollowTv(_) => "follow_tv",
            Self::Drop(_) => "drop",
            Self::ListFollows(_) => "list_follows",
            Self::Upcoming(_) => "upcoming",
            Self::TriggerRefresh(_) => "trigger_refresh",
            Self::WaitForRevision(_) => "wait_for_revision",
        }
    }
}
