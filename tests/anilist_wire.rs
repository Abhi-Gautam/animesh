//! S1 — the AniList contract proof.
//!
//! Every row of plan section 12's outcome matrix, exercised against a real HTTP
//! server. This runs before the schema freezes: if the parser cannot produce a
//! fact from a real payload, `V0001` must not have a column demanding it.
//!
//! Nothing here contacts AniList.

// clippy's allow-*-in-tests config only covers `#[test]` bodies, not the shared
// helpers beside them. In a test binary an `expect` is the assertion.
#![allow(clippy::expect_used)]

use std::time::Duration;

use animesh::domain::ids::{AniListId, UnixTimestamp};
use animesh::domain::media::MediaStatus;
use animesh::sources::anilist::client::{AniListClient, FetchOutcome, MAX_BODY_BYTES};
use animesh::sources::anilist::parser::{BatchIntegrityError, ItemResult};
use animesh::sources::anilist::{
    decode_batch, decode_detail, decode_search, queries, BatchDecode, DetailDecode, SearchDecode,
};

fn id(value: i64) -> AniListId {
    AniListId::new(value).expect("valid id")
}

fn now() -> UnixTimestamp {
    UnixTimestamp::new(1_700_000_000).expect("valid timestamp")
}

const ONE_PIECE: &str = r#"{"id":21,"title":{"romaji":"ONE PIECE","english":"One Piece","native":"ワンピース"},"status":"RELEASING","episodes":null,"format":"TV","seasonYear":1999,"nextAiringEpisode":{"episode":1169,"airingAt":1783865760}}"#;

const HUNTER: &str = r#"{"id":11061,"title":{"romaji":"Hunter x Hunter","english":"Hunter x Hunter (2011)","native":"ハンター×ハンター"},"status":"FINISHED","episodes":148,"format":"TV","seasonYear":2011,"nextAiringEpisode":null}"#;

async fn respond(status: usize, body: &str) -> (mockito::ServerGuard, AniListClient) {
    let mut server = mockito::Server::new_async().await;
    server
        .mock("POST", "/")
        .with_status(status)
        .with_body(body)
        .create_async()
        .await;
    let client = AniListClient::new(server.url()).expect("build client");
    (server, client)
}

// ---------------------------------------------------------------------------
// Transport classification
// ---------------------------------------------------------------------------

#[tokio::test]
async fn success_carries_body_status_and_duration() {
    let body = format!(r#"{{"data":{{"Media":{ONE_PIECE}}}}}"#);
    let (_server, client) = respond(200, &body).await;

    let response = client
        .post(&queries::detail(), serde_json::json!({}), now())
        .await;

    assert_eq!(response.outcome, FetchOutcome::Success);
    assert_eq!(response.http_status, Some(200));
    assert_eq!(response.body.as_deref(), Some(body.as_str()));
    assert_eq!(response.byte_length, Some(body.len()));
    assert_eq!(response.error_code, None);
}

#[tokio::test]
async fn rate_limit_headers_are_captured() {
    let mut server = mockito::Server::new_async().await;
    server
        .mock("POST", "/")
        .with_status(200)
        .with_header("x-ratelimit-remaining", "42")
        .with_header("x-ratelimit-reset", "1700000600")
        .with_body(r#"{"data":{"Media":null}}"#)
        .create_async()
        .await;
    let client = AniListClient::new(server.url()).expect("build client");

    let response = client.post("query{}", serde_json::json!({}), now()).await;

    assert_eq!(response.rate_limit_remaining, Some(42));
    assert_eq!(response.rate_limit_reset_at, Some(1_700_000_600));
}

#[tokio::test]
async fn rate_limited_response_reports_retry_after_seconds() {
    let mut server = mockito::Server::new_async().await;
    server
        .mock("POST", "/")
        .with_status(429)
        .with_header("retry-after", "60")
        .with_body("rate limited")
        .create_async()
        .await;
    let client = AniListClient::new(server.url()).expect("build client");

    let response = client.post("query{}", serde_json::json!({}), now()).await;

    assert_eq!(response.outcome, FetchOutcome::HttpError);
    assert_eq!(response.http_status, Some(429));
    assert_eq!(response.retry_after_secs, Some(60));
    // The body is still evidence even though it is not a GraphQL envelope.
    assert_eq!(response.body.as_deref(), Some("rate limited"));
}

#[tokio::test]
async fn rate_limited_response_reports_retry_after_http_date() {
    // A proxy is entitled to rewrite the seconds form into a date; misreading
    // it would either hammer the API or back off for years.
    let mut server = mockito::Server::new_async().await;
    server
        .mock("POST", "/")
        .with_status(429)
        .with_header("retry-after", "Tue, 14 Nov 2023 22:13:40 GMT")
        .with_body("rate limited")
        .create_async()
        .await;
    let client = AniListClient::new(server.url()).expect("build client");

    // now() is 1_700_000_000 = 2023-11-14T22:13:20Z, twenty seconds earlier.
    let response = client.post("query{}", serde_json::json!({}), now()).await;

    assert_eq!(response.retry_after_secs, Some(20));
}

#[tokio::test]
async fn server_error_is_retryable_and_keeps_its_body() {
    let (_server, client) = respond(503, "upstream unavailable").await;

    let response = client.post("query{}", serde_json::json!({}), now()).await;

    assert_eq!(response.outcome, FetchOutcome::HttpError);
    assert!(response.outcome.is_retryable());
    assert_eq!(response.body.as_deref(), Some("upstream unavailable"));
    assert_eq!(response.error_code.as_deref(), Some("http_503"));
}

#[tokio::test]
async fn oversized_body_is_rejected_without_being_kept() {
    let body = "x".repeat(MAX_BODY_BYTES + 1024);
    let (_server, client) = respond(200, &body).await;

    let response = client.post("query{}", serde_json::json!({}), now()).await;

    assert_eq!(response.outcome, FetchOutcome::TooLarge);
    assert!(response.outcome.permits_absent_body());
    assert_eq!(response.body, None);
    assert_eq!(response.http_status, Some(200));
}

#[tokio::test]
async fn a_body_at_exactly_the_cap_is_accepted() {
    // Off-by-one here would silently drop the largest legitimate responses.
    let body = "x".repeat(MAX_BODY_BYTES);
    let (_server, client) = respond(200, &body).await;

    let response = client.post("query{}", serde_json::json!({}), now()).await;

    assert_eq!(response.outcome, FetchOutcome::Success);
    assert_eq!(response.byte_length, Some(MAX_BODY_BYTES));
}

#[tokio::test]
async fn timeout_is_classified_separately_from_transport_failure() {
    // A listener that accepts and never answers.
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind");
    let addr = listener.local_addr().expect("addr");
    tokio::spawn(async move {
        let mut held = Vec::new();
        while let Ok((stream, _)) = listener.accept().await {
            held.push(stream);
        }
    });

    let client = AniListClient::with_timeout(format!("http://{addr}"), Duration::from_millis(150))
        .expect("build client");

    let response = client.post("query{}", serde_json::json!({}), now()).await;

    assert_eq!(response.outcome, FetchOutcome::Timeout);
    assert_eq!(response.body, None);
    assert!(response.outcome.permits_absent_body());
}

#[tokio::test]
async fn connection_refused_is_a_transport_error() {
    // Bind then drop, so the port is closed and nothing is listening.
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind");
    let addr = listener.local_addr().expect("addr");
    drop(listener);

    let client = AniListClient::with_timeout(format!("http://{addr}"), Duration::from_secs(2))
        .expect("build client");

    let response = client.post("query{}", serde_json::json!({}), now()).await;

    assert_eq!(response.outcome, FetchOutcome::TransportError);
    assert_eq!(response.body, None);
    assert_eq!(response.error_code.as_deref(), Some("connect"));
}

// ---------------------------------------------------------------------------
// Detail decoding
// ---------------------------------------------------------------------------

#[tokio::test]
async fn detail_decodes_a_complete_item() {
    let body = format!(r#"{{"data":{{"Media":{ONE_PIECE}}}}}"#);
    let (_server, client) = respond(200, &body).await;
    let response = client
        .post(&queries::detail(), serde_json::json!({"id": 21}), now())
        .await;

    let decoded = decode_detail(id(21), &response.body.expect("body"));

    match decoded {
        DetailDecode::Observed(observation) => {
            assert_eq!(observation.source_key.id, id(21));
            assert_eq!(observation.display_title.as_str(), "One Piece");
            assert_eq!(observation.status, MediaStatus::Releasing);
            let next = observation.next_airing.expect("next airing");
            assert_eq!(next.episode.get(), 1169);
        }
        other => panic!("expected an observation, got {other:?}"),
    }
}

#[tokio::test]
async fn detail_404_with_null_media_is_not_found_not_a_failure() {
    // AniList's real not-found shape. Classifying it as an HTTP failure would
    // put a nonexistent ID into permanent retry.
    let body = r#"{"errors":[{"message":"Not Found.","status":404}],"data":{"Media":null}}"#;
    let (_server, client) = respond(404, body).await;
    let response = client
        .post(
            &queries::detail(),
            serde_json::json!({"id": 999999999}),
            now(),
        )
        .await;

    assert_eq!(response.outcome, FetchOutcome::HttpError);

    let decoded = decode_detail(id(999_999_999), &response.body.expect("body"));
    assert_eq!(decoded, DetailDecode::NotFound);
    assert_eq!(decoded.outcome(), FetchOutcome::Success);
    assert!(!decoded.outcome().is_retryable());
}

#[tokio::test]
async fn detail_with_a_mismatched_id_is_an_integrity_error() {
    let body = format!(r#"{{"data":{{"Media":{HUNTER}}}}}"#);
    let decoded = decode_detail(id(21), &body);

    assert_eq!(
        decoded,
        DetailDecode::Integrity {
            requested: 21,
            returned: 11061,
        }
    );
    assert_eq!(decoded.outcome(), FetchOutcome::IntegrityError);
    assert!(!decoded.outcome().is_retryable());
}

#[tokio::test]
async fn detail_with_errors_and_no_data_is_a_graphql_error() {
    let body = r#"{"data":null,"errors":[{"message":"Validation error"}]}"#;
    let decoded = decode_detail(id(21), body);

    assert_eq!(decoded, DetailDecode::GraphQl("Validation error".into()));
    assert_eq!(decoded.outcome(), FetchOutcome::GraphQlError);
}

#[tokio::test]
async fn malformed_body_is_a_decode_error() {
    let (_server, client) = respond(200, "this is not json").await;
    let response = client.post("query{}", serde_json::json!({}), now()).await;

    // Transport succeeded; the failure is at decode.
    assert_eq!(response.outcome, FetchOutcome::Success);
    let decoded = decode_detail(id(21), &response.body.expect("body"));
    assert!(matches!(decoded, DetailDecode::Decode(_)));
    assert_eq!(decoded.outcome(), FetchOutcome::DecodeError);
}

#[tokio::test]
async fn an_envelope_with_neither_data_nor_errors_is_a_decode_error() {
    let decoded = decode_detail(id(21), r#"{"data":null}"#);
    assert!(matches!(decoded, DetailDecode::Decode(_)));
}

// ---------------------------------------------------------------------------
// Batch decoding
// ---------------------------------------------------------------------------

#[tokio::test]
async fn batch_decodes_every_requested_id() {
    let body = format!(r#"{{"data":{{"Page":{{"media":[{ONE_PIECE},{HUNTER}]}}}}}}"#);
    let requested = [id(21), id(11061)];
    let (_server, client) = respond(200, &body).await;
    let response = client
        .post(
            &queries::batch(),
            serde_json::json!({"ids": [21, 11061], "perPage": 2}),
            now(),
        )
        .await;

    match decode_batch(&requested, &response.body.expect("body")) {
        BatchDecode::Items(items) => {
            assert_eq!(items.len(), 2);
            assert!(matches!(items[&id(21)], ItemResult::Observed(_)));
            assert!(matches!(items[&id(11061)], ItemResult::Observed(_)));
        }
        other => panic!("expected items, got {other:?}"),
    }
}

#[tokio::test]
async fn batch_omission_is_missing_and_never_looks_like_a_null_schedule() {
    // The single most consequential distinction in the matrix: an omitted item
    // must preserve its projection, while an explicit null withdraws it.
    let body = format!(r#"{{"data":{{"Page":{{"media":[{ONE_PIECE}]}}}}}}"#);
    let requested = [id(21), id(11061)];

    match decode_batch(&requested, &body) {
        BatchDecode::Items(items) => {
            assert!(matches!(items[&id(21)], ItemResult::Observed(_)));
            assert_eq!(items[&id(11061)], ItemResult::Missing);
        }
        other => panic!("expected items, got {other:?}"),
    }
}

#[tokio::test]
async fn batch_with_data_and_errors_accepts_the_valid_items() {
    let body = format!(
        r#"{{"data":{{"Page":{{"media":[{ONE_PIECE},null]}}}},"errors":[{{"message":"Internal error"}}]}}"#
    );
    let requested = [id(21), id(11061)];

    match decode_batch(&requested, &body) {
        BatchDecode::Items(items) => {
            assert!(matches!(items[&id(21)], ItemResult::Observed(_)));
            assert_eq!(items[&id(11061)], ItemResult::Missing);
        }
        other => panic!("expected partial items, got {other:?}"),
    }
}

#[tokio::test]
async fn batch_with_a_duplicate_id_is_an_integrity_error() {
    let body = format!(r#"{{"data":{{"Page":{{"media":[{ONE_PIECE},{ONE_PIECE}]}}}}}}"#);
    let decoded = decode_batch(&[id(21)], &body);

    assert_eq!(
        decoded,
        BatchDecode::Integrity(BatchIntegrityError::DuplicateId(21))
    );
    assert_eq!(decoded.outcome(), FetchOutcome::IntegrityError);
}

#[tokio::test]
async fn batch_with_an_unrequested_id_is_an_integrity_error() {
    let body = format!(r#"{{"data":{{"Page":{{"media":[{HUNTER}]}}}}}}"#);
    let decoded = decode_batch(&[id(21)], &body);

    assert_eq!(
        decoded,
        BatchDecode::Integrity(BatchIntegrityError::UnrequestedId(11061))
    );
}

// ---------------------------------------------------------------------------
// Search decoding
// ---------------------------------------------------------------------------

#[tokio::test]
async fn search_decodes_candidates() {
    let body = format!(r#"{{"data":{{"Page":{{"media":[{ONE_PIECE},{HUNTER}]}}}}}}"#);
    let (_server, client) = respond(200, &body).await;
    let response = client
        .post(
            &queries::search(),
            serde_json::json!({"search": "one piece", "perPage": queries::SEARCH_PER_PAGE}),
            now(),
        )
        .await;

    match decode_search(&response.body.expect("body")) {
        SearchDecode::Candidates(candidates) => {
            assert_eq!(candidates.len(), 2);
            assert_eq!(candidates[0].anilist_id, id(21));
            assert_eq!(candidates[0].display_title.as_str(), "One Piece");
            assert_eq!(candidates[1].episode_count.map(|e| e.get()), Some(148));
        }
        other => panic!("expected candidates, got {other:?}"),
    }
}

#[tokio::test]
async fn search_with_errors_and_no_data_is_a_graphql_error() {
    let decoded = decode_search(r#"{"data":null,"errors":[{"message":"Validation error"}]}"#);
    assert_eq!(decoded, SearchDecode::GraphQl("Validation error".into()));
}

#[tokio::test]
async fn search_requests_at_most_ten_results() {
    assert_eq!(queries::SEARCH_PER_PAGE, 10);
}

// ---------------------------------------------------------------------------
// Status coverage
// ---------------------------------------------------------------------------

#[tokio::test]
async fn every_anilist_status_normalizes_over_the_wire() {
    for (raw, expected) in [
        ("RELEASING", MediaStatus::Releasing),
        ("NOT_YET_RELEASED", MediaStatus::NotYetReleased),
        ("FINISHED", MediaStatus::Finished),
        ("CANCELLED", MediaStatus::Cancelled),
        ("HIATUS", MediaStatus::Hiatus),
        ("ASCENDED", MediaStatus::Unknown),
    ] {
        let body = format!(
            r#"{{"data":{{"Media":{{"id":21,"title":{{"romaji":"x","english":null,"native":null}},"status":"{raw}","episodes":null,"format":"TV","seasonYear":1999,"nextAiringEpisode":null}}}}}}"#
        );
        match decode_detail(id(21), &body) {
            DetailDecode::Observed(observation) => {
                assert_eq!(observation.status, expected, "status {raw}");
                if expected == MediaStatus::Unknown {
                    assert_eq!(
                        observation.status_raw.as_ref().map(|t| t.as_str()),
                        Some(raw),
                        "unknown status must retain its raw value"
                    );
                }
            }
            other => panic!("status {raw} failed to decode: {other:?}"),
        }
    }
}

#[tokio::test]
async fn a_null_status_does_not_stop_ingestion() {
    let body = r#"{"data":{"Media":{"id":21,"title":{"romaji":"x","english":null,"native":null},"status":null,"episodes":null,"format":null,"seasonYear":null,"nextAiringEpisode":{"episode":5,"airingAt":1783865760}}}}"#;

    match decode_detail(id(21), body) {
        DetailDecode::Observed(observation) => {
            assert_eq!(observation.status, MediaStatus::Unknown);
            assert_eq!(observation.status_raw, None);
            // The schedule is the point; it must survive an unknown status.
            assert!(observation.next_airing.is_some());
        }
        other => panic!("expected an observation, got {other:?}"),
    }
}
