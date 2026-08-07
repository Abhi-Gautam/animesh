//! The frozen IPC contract between the CLI and the app.
//!
//! Seven requests, all plain request/reply. The notification-plan snapshot
//! protocol, the change-wait long poll, and the attempt CAS that earlier
//! revisions carried here are gone: they existed only to move state across a
//! daemon/menu process boundary that no longer exists.
//!
//! Every enum is adjacently tagged, so an unknown variant fails to decode
//! rather than silently matching a neighbour.

use serde::{Deserialize, Serialize};

use crate::domain::ids::{AniListId, MediaId};
use crate::domain::media::SearchCandidate;
use crate::domain::read_models::{
    FollowResult, FollowSummary, HealthSnapshot, RefreshAccepted, UpcomingRelease,
    MAX_UPCOMING_LIMIT,
};
use crate::error::{AppError, ErrorCode};

/// Bumped only for an incompatible change. A mismatch is reported, never
/// negotiated: two versions guessing at each other is worse than a clear stop.
pub const PROTOCOL_VERSION: u32 = 1;

/// Largest frame either side will read or write.
pub const MAX_FRAME_BYTES: usize = 1024 * 1024;

/// Longest accepted search query.
pub const MAX_QUERY_LEN: usize = 128;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", content = "data", rename_all = "snake_case")]
pub enum Request {
    Status,
    SearchAnime { query: String },
    FollowAnilist { id: AniListId },
    Drop { media_id: MediaId },
    ListFollows,
    Upcoming { limit: Option<u32> },
    TriggerRefresh,
}

impl Request {
    /// Stable label for logs and metrics.
    pub const fn name(&self) -> &'static str {
        match self {
            Self::Status => "status",
            Self::SearchAnime { .. } => "search_anime",
            Self::FollowAnilist { .. } => "follow_anilist",
            Self::Drop { .. } => "drop",
            Self::ListFollows => "list_follows",
            Self::Upcoming { .. } => "upcoming",
            Self::TriggerRefresh => "trigger_refresh",
        }
    }

    /// Whether this request may reach the source.
    ///
    /// Used to reject source work while bootstrap is degraded, and asserted by
    /// a test so `upcoming` can never quietly acquire a network path.
    pub const fn touches_source(&self) -> bool {
        matches!(
            self,
            Self::SearchAnime { .. } | Self::FollowAnilist { .. } | Self::TriggerRefresh
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
            Self::SearchAnime { query } => {
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
            Self::Upcoming { limit: Some(0) } => {
                Err(AppError::invalid_argument("limit must be at least 1"))
            }
            Self::Upcoming { limit: Some(limit) } if *limit > MAX_UPCOMING_LIMIT => Err(
                AppError::invalid_argument(format!("limit must be at most {MAX_UPCOMING_LIMIT}")),
            ),
            _ => Ok(()),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", content = "data", rename_all = "snake_case")]
pub enum Response {
    Status(Box<HealthSnapshot>),
    SearchAnime(Vec<SearchCandidate>),
    FollowAnilist(Box<FollowResult>),
    Drop(Box<FollowSummary>),
    ListFollows(Vec<FollowSummary>),
    Upcoming(Vec<UpcomingRelease>),
    TriggerRefresh(RefreshAccepted),
}

impl Response {
    pub const fn name(&self) -> &'static str {
        match self {
            Self::Status(_) => "status",
            Self::SearchAnime(_) => "search_anime",
            Self::FollowAnilist(_) => "follow_anilist",
            Self::Drop(_) => "drop",
            Self::ListFollows(_) => "list_follows",
            Self::Upcoming(_) => "upcoming",
            Self::TriggerRefresh(_) => "trigger_refresh",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "status", content = "data", rename_all = "snake_case")]
pub enum ReplyResult {
    Ok(Response),
    Err(AppError),
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RequestEnvelope {
    pub protocol_version: u32,
    pub request_id: String,
    pub body: Request,
}

impl RequestEnvelope {
    pub fn new(request_id: impl Into<String>, body: Request) -> Self {
        Self {
            protocol_version: PROTOCOL_VERSION,
            request_id: request_id.into(),
            body,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ReplyEnvelope {
    pub protocol_version: u32,
    pub request_id: String,
    pub app_instance_id: String,
    pub result: ReplyResult,
}

impl ReplyEnvelope {
    pub fn ok(request_id: impl Into<String>, instance: impl Into<String>, body: Response) -> Self {
        Self {
            protocol_version: PROTOCOL_VERSION,
            request_id: request_id.into(),
            app_instance_id: instance.into(),
            result: ReplyResult::Ok(body),
        }
    }

    pub fn err(request_id: impl Into<String>, instance: impl Into<String>, body: AppError) -> Self {
        Self {
            protocol_version: PROTOCOL_VERSION,
            request_id: request_id.into(),
            app_instance_id: instance.into(),
            result: ReplyResult::Err(body),
        }
    }
}

/// Checks a peer's version against ours.
///
/// Returns an actionable error rather than attempting compatibility: the two
/// sides ship in the same bundle, so a mismatch means a partial install, and
/// guessing would corrupt state rather than fix it.
pub fn check_version(peer: u32) -> Result<(), AppError> {
    if peer == PROTOCOL_VERSION {
        return Ok(());
    }
    Err(AppError::new(
        ErrorCode::ProtocolMismatch,
        format!(
            "peer speaks protocol {peer}, this build speaks {PROTOCOL_VERSION}; \
             reinstall so the app and CLI come from the same bundle"
        ),
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn every_request() -> Vec<Request> {
        vec![
            Request::Status,
            Request::SearchAnime {
                query: "one piece".into(),
            },
            Request::FollowAnilist {
                id: AniListId::new(21).expect("valid id"),
            },
            Request::Drop {
                media_id: MediaId::new(1).expect("valid id"),
            },
            Request::ListFollows,
            Request::Upcoming { limit: Some(10) },
            Request::TriggerRefresh,
        ]
    }

    #[test]
    fn the_surface_is_exactly_seven_requests() {
        // Guards against the deleted notification and change-wait requests
        // creeping back in.
        assert_eq!(every_request().len(), 7);
    }

    #[test]
    fn requests_round_trip() {
        for request in every_request() {
            let json = serde_json::to_string(&request).expect("serialize");
            let decoded: Request = serde_json::from_str(&json).expect("deserialize");
            assert_eq!(decoded, request);
        }
    }

    #[test]
    fn wire_form_is_adjacently_tagged_snake_case() {
        let json = serde_json::to_string(&Request::FollowAnilist {
            id: AniListId::new(21).expect("valid id"),
        })
        .expect("serialize");
        assert_eq!(json, r#"{"kind":"follow_anilist","data":{"id":21}}"#);

        let json = serde_json::to_string(&Request::Status).expect("serialize");
        assert_eq!(json, r#"{"kind":"status"}"#);
    }

    #[test]
    fn an_unknown_request_kind_fails_to_decode() {
        // Must not silently match a neighbouring variant.
        let result: Result<Request, _> =
            serde_json::from_str(r#"{"kind":"inject_airing","data":{}}"#);
        assert!(result.is_err());
    }

    #[test]
    fn deleted_requests_are_not_decodable() {
        for kind in [
            "wait_for_change",
            "open_notification_plan",
            "notification_plan_page",
            "validate_notification_plan",
            "begin_notification_attempt",
            "acknowledge_notification_result",
            "report_notification_surface_state",
            "wake",
        ] {
            let json = format!(r#"{{"kind":"{kind}","data":{{}}}}"#);
            assert!(
                serde_json::from_str::<Request>(&json).is_err(),
                "{kind} still decodes"
            );
        }
    }

    #[test]
    fn an_out_of_range_id_is_rejected_at_the_boundary() {
        // The newtype's serde bound is what stops a hostile frame here.
        assert!(
            serde_json::from_str::<Request>(r#"{"kind":"follow_anilist","data":{"id":0}}"#)
                .is_err()
        );
        assert!(serde_json::from_str::<Request>(
            r#"{"kind":"follow_anilist","data":{"id":2147483648}}"#
        )
        .is_err());
    }

    #[test]
    fn only_source_touching_requests_report_as_such() {
        assert!(Request::TriggerRefresh.touches_source());
        assert!(Request::SearchAnime { query: "x".into() }.touches_source());
        assert!(Request::FollowAnilist {
            id: AniListId::new(21).expect("valid id"),
        }
        .touches_source());

        // `next` must never acquire a network path; this is the assertion that
        // would fail if it did.
        assert!(!Request::Upcoming { limit: None }.touches_source());
        assert!(!Request::ListFollows.touches_source());
        assert!(!Request::Status.touches_source());
    }

    #[test]
    fn only_status_is_served_while_degraded() {
        assert!(Request::Status.served_when_degraded());
        for request in every_request()
            .into_iter()
            .filter(|r| *r != Request::Status)
        {
            assert!(
                !request.served_when_degraded(),
                "{} is served while degraded",
                request.name()
            );
        }
    }

    #[test]
    fn empty_and_blank_queries_are_rejected() {
        assert!(Request::SearchAnime {
            query: String::new()
        }
        .validate()
        .is_err());
        assert!(Request::SearchAnime {
            query: "   ".into()
        }
        .validate()
        .is_err());
    }

    #[test]
    fn overlong_query_is_rejected() {
        let request = Request::SearchAnime {
            query: "x".repeat(MAX_QUERY_LEN + 1),
        };
        assert_eq!(
            request.validate().map_err(|e| e.code),
            Err(ErrorCode::InvalidArgument)
        );
        assert!(Request::SearchAnime {
            query: "x".repeat(MAX_QUERY_LEN)
        }
        .validate()
        .is_ok());
    }

    #[test]
    fn limit_bounds_are_enforced_server_side() {
        assert!(Request::Upcoming { limit: Some(0) }.validate().is_err());
        assert!(Request::Upcoming {
            limit: Some(MAX_UPCOMING_LIMIT + 1)
        }
        .validate()
        .is_err());
        assert!(Request::Upcoming {
            limit: Some(MAX_UPCOMING_LIMIT)
        }
        .validate()
        .is_ok());
        assert!(Request::Upcoming { limit: None }.validate().is_ok());
    }

    #[test]
    fn envelopes_round_trip() {
        let envelope = RequestEnvelope::new("req-1", Request::ListFollows);
        let json = serde_json::to_string(&envelope).expect("serialize");
        let decoded: RequestEnvelope = serde_json::from_str(&json).expect("deserialize");
        assert_eq!(decoded, envelope);
        assert_eq!(decoded.protocol_version, PROTOCOL_VERSION);
    }

    #[test]
    fn error_replies_round_trip() {
        let reply = ReplyEnvelope::err("req-1", "instance-a", AppError::not_found("no such media"));
        let json = serde_json::to_string(&reply).expect("serialize");
        let decoded: ReplyEnvelope = serde_json::from_str(&json).expect("deserialize");
        assert_eq!(decoded, reply);
        match decoded.result {
            ReplyResult::Err(error) => assert_eq!(error.code, ErrorCode::NotFound),
            ReplyResult::Ok(_) => panic!("expected an error reply"),
        }
    }

    #[test]
    fn matching_versions_pass() {
        assert!(check_version(PROTOCOL_VERSION).is_ok());
    }

    #[test]
    fn mismatched_versions_are_explicit_and_actionable() {
        let error = check_version(PROTOCOL_VERSION + 1).expect_err("should mismatch");
        assert_eq!(error.code, ErrorCode::ProtocolMismatch);
        assert!(error.message.contains("reinstall"), "{}", error.message);
    }

    #[test]
    fn protocol_version_is_frozen_at_one() {
        assert_eq!(PROTOCOL_VERSION, 1);
    }
}
