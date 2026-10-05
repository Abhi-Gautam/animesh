//! The frozen IPC contract between the CLI and the app.
//!
//! Commands and local queries use plain request/reply. Long-lived surfaces wait
//! on data revisions and re-query snapshots; notification reconciliation remains
//! inside the daemon.
//!
//! Every enum is adjacently tagged, so an unknown variant fails to decode
//! rather than silently matching a neighbour.

use serde::{Deserialize, Serialize};

use crate::error::{AppError, ErrorCode};

pub use crate::application::commands::{Request, Response, MAX_QUERY_LEN};

/// Bumped only for an incompatible change. A mismatch is reported, never
/// negotiated: two versions guessing at each other is worse than a clear stop.
pub const PROTOCOL_VERSION: u32 = 2;

/// Largest frame either side will read or write.
pub const MAX_FRAME_BYTES: usize = 1024 * 1024;

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
    use crate::domain::ids::{AniListId, MediaId, TvMazeId};
    use crate::domain::read_models::MAX_UPCOMING_LIMIT;

    fn every_request() -> Vec<Request> {
        vec![
            Request::Status,
            Request::Search {
                query: "one piece".into(),
                kind: None,
            },
            Request::ResolveSearch {
                candidates: Vec::new(),
            },
            Request::View {
                query: crate::domain::command_center::ViewQuery::Home,
            },
            Request::Refresh {
                target: crate::domain::command_center::RefreshTarget::Library,
            },
            Request::SearchAnime {
                query: "one piece".into(),
            },
            Request::FollowAnilist {
                id: AniListId::new(21).expect("valid id"),
            },
            Request::SearchTv { query: None },
            Request::FollowTv {
                id: TvMazeId::new(82).expect("valid id"),
            },
            Request::Drop {
                media_id: MediaId::new(1).expect("valid id"),
            },
            Request::ListFollows,
            Request::Upcoming {
                limit: Some(10),
                dropped: false,
            },
            Request::TriggerRefresh,
            Request::WaitForRevision {
                instance_id: "instance".into(),
                after: 42,
            },
        ]
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
        assert!(
            serde_json::from_str::<Request>(r#"{"kind":"follow_tv","data":{"id":0}}"#).is_err()
        );
    }

    #[test]
    fn only_source_touching_requests_report_as_such() {
        assert!(Request::TriggerRefresh.touches_source());
        assert!(Request::SearchAnime { query: "x".into() }.touches_source());
        assert!(Request::FollowAnilist {
            id: AniListId::new(21).expect("valid id"),
        }
        .touches_source());
        assert!(Request::SearchTv { query: None }.touches_source());
        assert!(Request::FollowTv {
            id: TvMazeId::new(82).expect("valid id"),
        }
        .touches_source());

        // `next` must never acquire a network path; this is the assertion that
        // would fail if it did.
        assert!(!Request::Upcoming {
            limit: None,
            dropped: false,
        }
        .touches_source());
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
        assert!(Request::Upcoming {
            limit: Some(0),
            dropped: false,
        }
        .validate()
        .is_err());
        assert!(Request::Upcoming {
            limit: Some(MAX_UPCOMING_LIMIT + 1),
            dropped: false,
        }
        .validate()
        .is_err());
        assert!(Request::Upcoming {
            limit: Some(MAX_UPCOMING_LIMIT),
            dropped: false,
        }
        .validate()
        .is_ok());
        assert!(Request::Upcoming {
            limit: None,
            dropped: false,
        }
        .validate()
        .is_ok());
    }

    #[test]
    fn upcoming_defaults_dropped_to_false() {
        let decoded: Request =
            serde_json::from_str(r#"{"kind":"upcoming","data":{"limit":10}}"#).expect("decode");
        assert_eq!(
            decoded,
            Request::Upcoming {
                limit: Some(10),
                dropped: false,
            }
        );
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
    fn protocol_version_is_frozen_at_two() {
        assert_eq!(PROTOCOL_VERSION, 2);
    }
}
