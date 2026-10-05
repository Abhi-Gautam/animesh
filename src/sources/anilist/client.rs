//! AniList transport.
//!
//! Returns a classified result rather than `Result`. Every response occurrence
//! becomes evidence, including the failures, so a transport error is data here
//! rather than an early return.

use std::time::{Duration, Instant};

use serde::Serialize;

use crate::domain::ids::UnixTimestamp;
pub use crate::sources::fetch::{
    parse_retry_after, FetchOutcome, RawResponse, MAX_BODY_BYTES, REQUEST_TIMEOUT,
};

pub const DEFAULT_BASE_URL: &str = "https://graphql.anilist.co";

const USER_AGENT: &str = concat!("animesh/", env!("CARGO_PKG_VERSION"));

#[derive(Debug)]
pub struct AniListClient {
    client: reqwest::Client,
    base_url: String,
}

impl AniListClient {
    pub fn new(base_url: impl Into<String>) -> Result<Self, reqwest::Error> {
        Self::with_timeout(base_url, REQUEST_TIMEOUT)
    }

    /// Builds a client with a non-default timeout.
    ///
    /// Exists so the timeout path can be tested in milliseconds instead of the
    /// production ten seconds.
    pub fn with_timeout(
        base_url: impl Into<String>,
        timeout: Duration,
    ) -> Result<Self, reqwest::Error> {
        let client = reqwest::Client::builder()
            .timeout(timeout)
            .user_agent(USER_AGENT)
            .build()?;
        Ok(Self {
            client,
            base_url: base_url.into(),
        })
    }

    pub fn production() -> Result<Self, reqwest::Error> {
        Self::new(DEFAULT_BASE_URL)
    }

    /// Posts a GraphQL document and classifies whatever comes back.
    pub async fn post<V: Serialize>(
        &self,
        query: &str,
        variables: V,
        now: UnixTimestamp,
    ) -> RawResponse {
        let started = Instant::now();
        let payload = serde_json::json!({ "query": query, "variables": variables });

        let response = match self.client.post(&self.base_url).json(&payload).send().await {
            Ok(response) => response,
            Err(error) => {
                let outcome = if error.is_timeout() {
                    FetchOutcome::Timeout
                } else {
                    FetchOutcome::TransportError
                };
                let code = if error.is_connect() {
                    "connect"
                } else if error.is_timeout() {
                    "timeout"
                } else if error.is_request() {
                    "request"
                } else {
                    "transport"
                };
                return RawResponse::failure(outcome, started.elapsed(), code);
            }
        };

        let http_status = response.status().as_u16();
        let headers = response.headers().clone();
        let retry_after_secs = headers
            .get(reqwest::header::RETRY_AFTER)
            .and_then(|v| v.to_str().ok())
            .and_then(|v| parse_retry_after(v, now));
        let rate_limit_remaining =
            header_number(&headers, "x-ratelimit-remaining").and_then(|v| u32::try_from(v).ok());
        let rate_limit_reset_at = header_number(&headers, "x-ratelimit-reset");

        let mut response = response;
        let mut buffer: Vec<u8> = Vec::new();
        loop {
            match response.chunk().await {
                Ok(Some(chunk)) => {
                    if buffer.len() + chunk.len() > MAX_BODY_BYTES {
                        let mut result = RawResponse::failure(
                            FetchOutcome::TooLarge,
                            started.elapsed(),
                            "body_over_cap",
                        );
                        result.http_status = Some(http_status);
                        result.retry_after_secs = retry_after_secs;
                        result.rate_limit_remaining = rate_limit_remaining;
                        result.rate_limit_reset_at = rate_limit_reset_at;
                        return result;
                    }
                    buffer.extend_from_slice(&chunk);
                }
                Ok(None) => break,
                Err(error) => {
                    let outcome = if error.is_timeout() {
                        FetchOutcome::Timeout
                    } else {
                        FetchOutcome::TransportError
                    };
                    let mut result =
                        RawResponse::failure(outcome, started.elapsed(), "body_stream");
                    result.http_status = Some(http_status);
                    return result;
                }
            }
        }

        let byte_length = buffer.len();
        let body = match String::from_utf8(buffer) {
            Ok(body) => body,
            Err(_) => {
                let mut result =
                    RawResponse::failure(FetchOutcome::DecodeError, started.elapsed(), "not_utf8");
                result.http_status = Some(http_status);
                return result;
            }
        };

        let outcome = if (200..300).contains(&http_status) {
            FetchOutcome::Success
        } else {
            FetchOutcome::HttpError
        };

        RawResponse {
            outcome,
            http_status: Some(http_status),
            body: Some(body),
            byte_length: Some(byte_length),
            retry_after_secs,
            rate_limit_remaining,
            rate_limit_reset_at,
            duration_ms: started.elapsed().as_millis() as u64,
            error_code: (outcome == FetchOutcome::HttpError).then(|| format!("http_{http_status}")),
        }
    }
}

fn header_number(headers: &reqwest::header::HeaderMap, name: &str) -> Option<i64> {
    headers.get(name)?.to_str().ok()?.trim().parse().ok()
}
