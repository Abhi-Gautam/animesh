//! Small, transport-independent serving contracts for the Command Center.

use serde::{Deserialize, Serialize};

use super::ids::{MediaId, Source, SourceKey, UnixTimestamp};
use super::media::{MediaKind, MediaObservation, SearchCandidate};
use super::read_models::{Freshness, HealthSnapshot, UpcomingRelease};
use super::release::FollowState;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RevisionStamp {
    pub instance_id: String,
    pub revision: u64,
}

pub const PAGE_SIZE: u32 = 50;
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MediaDetail {
    pub media_id: MediaId,
    pub facts: MediaObservation,
    pub follow_state: Option<FollowState>,
    pub last_success_at: Option<UnixTimestamp>,
    pub freshness: Freshness,
    pub schedule_revision: Option<i64>,
    pub notification_state: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PageCursor {
    pub instance_id: String,
    pub revision: u64,
    pub offset: u32,
    pub scope: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum FollowSort {
    #[default]
    Alphabetical,
    NextRelease,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "screen", rename_all = "snake_case")]
pub enum ViewQuery {
    Home,
    Library {
        kind: Option<MediaKind>,
        state: FollowState,
        sort: FollowSort,
        cursor: Option<PageCursor>,
    },
    Schedule {
        kind: Option<MediaKind>,
        cursor: Option<PageCursor>,
    },
    Detail {
        key: SourceKey,
    },
    Health,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Page<T> {
    pub items: Vec<T>,
    pub next: Option<PageCursor>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "screen", content = "data", rename_all = "snake_case")]
pub enum ViewData {
    Home {
        dropped: Vec<UpcomingRelease>,
        upcoming: Vec<UpcomingRelease>,
        anime: u32,
        tv: u32,
    },
    Library(Page<MediaDetail>),
    Schedule(Page<UpcomingRelease>),
    Detail(Box<MediaDetail>),
    Health {
        sources: Vec<SourceHealth>,
        operations: Vec<OperationStatus>,
    },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ViewSnapshot {
    pub stamp: RevisionStamp,
    pub generated_at: UnixTimestamp,
    pub health: HealthSnapshot,
    pub view: ViewData,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SourceHealth {
    pub source: Source,
    pub blocked_until: Option<UnixTimestamp>,
    pub last_success_at: Option<UnixTimestamp>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum RefreshTarget {
    Library,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum OperationState {
    Queued,
    Running,
    Completed,
    Throttled,
    Failed,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct OperationStatus {
    pub id: String,
    pub target: RefreshTarget,
    pub state: OperationState,
    pub updated_at: UnixTimestamp,
    pub message: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SearchHit {
    pub candidate: SearchCandidate,
    pub media_id: Option<MediaId>,
    pub followed: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SourceIssue {
    pub source: Source,
    pub message: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SearchResults {
    pub items: Vec<SearchHit>,
    pub issues: Vec<SourceIssue>,
}
