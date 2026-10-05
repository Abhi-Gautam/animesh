//! Small, transport-independent serving contracts for the Command Center.

use chrono::Datelike;
use serde::{Deserialize, Serialize};

use super::ids::{MediaId, Source, SourceKey, UnixTimestamp};
use super::media::SearchCandidate;
use super::media::{MediaKind, MediaObservation};
use super::read_models::{Freshness, HealthSnapshot, UpcomingRelease};
use super::release::FollowState;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RevisionStamp {
    pub instance_id: String,
    pub revision: u64,
}

pub const PAGE_SIZE: u32 = 50;
pub const DISCOVERY_LIMIT: usize = 50;
pub const DISCOVERY_TTL: i64 = 6 * 60 * 60;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FeedKey {
    AnimeThisSeason,
    AnimeAiringThisWeek,
    TvOnNow,
}

impl FeedKey {
    pub const ALL: [Self; 3] = [
        Self::AnimeThisSeason,
        Self::AnimeAiringThisWeek,
        Self::TvOnNow,
    ];
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::AnimeThisSeason => "anime_this_season",
            Self::AnimeAiringThisWeek => "anime_airing_this_week",
            Self::TvOnNow => "tv_on_now",
        }
    }
    pub const fn source(self) -> Source {
        match self {
            Self::TvOnNow => Source::TvMaze,
            _ => Source::AniList,
        }
    }
    pub fn accepts(self, observation: &MediaObservation, now: UnixTimestamp) -> bool {
        if observation.source_key.source != self.source() {
            return false;
        }
        match self {
            Self::AnimeAiringThisWeek => {
                observation.status == super::media::MediaStatus::Releasing
                    && observation.next_airing.is_some_and(|next| {
                        next.airing_at.get() >= now.get().saturating_sub(86400)
                            && next.airing_at.get() <= now.get().saturating_add(7 * 86400)
                    })
            }
            _ => true,
        }
    }
}

pub fn anime_season(now: UnixTimestamp) -> (&'static str, i32) {
    let date = now.to_utc();
    let season = match date.month() {
        1..=3 => "WINTER",
        4..=6 => "SPRING",
        7..=9 => "SUMMER",
        _ => "FALL",
    };
    (season, date.year())
}

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
pub struct DiscoveryFeed {
    pub key: FeedKey,
    pub generated_at: Option<UnixTimestamp>,
    pub expires_at: Option<UnixTimestamp>,
    pub freshness: Freshness,
    pub last_error: Option<String>,
    pub last_attempt: Option<SnapshotCompleteness>,
    pub items: Vec<MediaDetail>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SnapshotCompleteness {
    Complete,
    Partial,
    Failed,
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
    Discovery {
        feed: FeedKey,
    },
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
    Discovery(DiscoveryFeed),
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
#[serde(tag = "kind", content = "feed", rename_all = "snake_case")]
pub enum RefreshTarget {
    Library,
    Discovery(FeedKey),
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
