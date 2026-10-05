//! TVmaze transport classification.

#![allow(clippy::expect_used)]

use animesh::domain::ids::UnixTimestamp;
use animesh::sources::fetch::{FetchOutcome, MAX_BODY_BYTES};
use animesh::sources::tvmaze::TvMazeClient;

fn at(seconds: i64) -> UnixTimestamp {
    UnixTimestamp::new(seconds).expect("valid timestamp")
}

#[tokio::test]
async fn a_200_is_success_with_the_body() {
    let mut server = mockito::Server::new_async().await;
    server
        .mock("GET", "/shows/82?embed=nextepisode")
        .with_status(200)
        .with_body(r#"{"id":82,"name":"Game of Thrones"}"#)
        .create_async()
        .await;

    let client = TvMazeClient::new(server.url()).expect("client");
    let response = client
        .get("/shows/82?embed=nextepisode", at(1_700_000_000))
        .await;
    assert_eq!(response.outcome, FetchOutcome::Success);
    assert_eq!(response.http_status, Some(200));
    assert!(response
        .body
        .as_deref()
        .unwrap_or("")
        .contains("Game of Thrones"));
}

#[tokio::test]
async fn a_404_is_an_empty_success_so_the_parser_can_say_not_found() {
    let mut server = mockito::Server::new_async().await;
    server
        .mock("GET", "/shows/999?embed=nextepisode")
        .with_status(404)
        .create_async()
        .await;

    let client = TvMazeClient::new(server.url()).expect("client");
    let response = client
        .get("/shows/999?embed=nextepisode", at(1_700_000_000))
        .await;
    assert_eq!(response.outcome, FetchOutcome::Success);
    assert_eq!(response.http_status, Some(404));
    assert_eq!(response.body.as_deref(), Some(""));
    assert_eq!(response.error_code.as_deref(), Some("not_found"));
}

#[tokio::test]
async fn an_oversized_body_is_too_large_without_storing_the_bytes() {
    let mut server = mockito::Server::new_async().await;
    let body = "x".repeat(MAX_BODY_BYTES + 1024);
    server
        .mock("GET", "/shows/1?embed=nextepisode")
        .with_status(200)
        .with_body(body)
        .create_async()
        .await;

    let client = TvMazeClient::new(server.url()).expect("client");
    let response = client
        .get("/shows/1?embed=nextepisode", at(1_700_000_000))
        .await;
    assert_eq!(response.outcome, FetchOutcome::TooLarge);
    assert!(response.body.is_none());
}

#[tokio::test]
async fn retry_after_is_captured_on_429() {
    let mut server = mockito::Server::new_async().await;
    server
        .mock("GET", "/schedule?country=US")
        .with_status(429)
        .with_header("retry-after", "12")
        .with_body("slow down")
        .create_async()
        .await;

    let client = TvMazeClient::new(server.url()).expect("client");
    let response = client.get("/schedule?country=US", at(1_700_000_000)).await;
    assert_eq!(response.outcome, FetchOutcome::HttpError);
    assert_eq!(response.http_status, Some(429));
    assert_eq!(response.retry_after_secs, Some(12));
}
