//! AniList transport.
//!
//! Returns a classified result rather than `Result`: section 12 requires every
//! response occurrence to become Bronze evidence, including the failures, so a
//! transport error is data here rather than an early return.

use std::time::{Duration, Instant};

use serde::Serialize;

use crate::domain::ids::UnixTimestamp;

/// Hard ceiling on a response body.
///
/// Enforced while reading rather than after: a server that streams gigabytes
/// must not be able to make us allocate them first.
pub const MAX_BODY_BYTES: usize = 2 * 1024 * 1024;

pub const REQUEST_TIMEOUT: Duration = Duration::from_secs(10);

pub const DEFAULT_BASE_URL: &str = "https://graphql.anilist.co";

const USER_AGENT: &str = concat!("animesh/", env!("CARGO_PKG_VERSION"));

/// Classification of one response occurrence.
///
/// The string forms are the `source_fetches.outcome` CHECK values; changing one
/// is a schema migration.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum FetchOutcome {
    Success,
    HttpError,
    GraphQlError,
    DecodeError,
    TransportError,
    Timeout,
    TooLarge,
    IntegrityError,
}

impl FetchOutcome {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Success => "success",
            Self::HttpError => "http_error",
            Self::GraphQlError => "graphql_error",
            Self::DecodeError => "decode_error",
            Self::TransportError => "transport_error",
            Self::Timeout => "timeout",
            Self::TooLarge => "too_large",
            Self::IntegrityError => "integrity_error",
        }
    }

    /// Whether this outcome may be retried on the normal failure cadence.
    pub const fn is_retryable(self) -> bool {
        matches!(
            self,
            Self::HttpError | Self::TransportError | Self::Timeout | Self::TooLarge
        )
    }

    /// Whether the schema permits a fetch row with no stored body.
    ///
    /// Mirrors the `V0001` CHECK so a violation is caught in Rust tests rather
    /// than at insert time.
    pub const fn permits_absent_body(self) -> bool {
        matches!(self, Self::TransportError | Self::Timeout | Self::TooLarge)
    }
}

/// One response occurrence, successful or not.
#[derive(Debug, Clone)]
pub struct RawResponse {
    pub outcome: FetchOutcome,
    pub http_status: Option<u16>,
    pub body: Option<String>,
    pub byte_length: Option<usize>,
    pub retry_after_secs: Option<u32>,
    pub rate_limit_remaining: Option<u32>,
    pub rate_limit_reset_at: Option<i64>,
    pub duration_ms: u64,
    pub error_code: Option<String>,
}

impl RawResponse {
    fn failure(outcome: FetchOutcome, duration: Duration, error_code: &str) -> Self {
        Self {
            outcome,
            http_status: None,
            body: None,
            byte_length: None,
            retry_after_secs: None,
            rate_limit_remaining: None,
            rate_limit_reset_at: None,
            duration_ms: duration.as_millis() as u64,
            error_code: Some(error_code.to_owned()),
        }
    }

    /// Reclassifies after envelope decoding, preserving the stored evidence.
    pub fn reclassified(mut self, outcome: FetchOutcome, error_code: Option<String>) -> Self {
        self.outcome = outcome;
        self.error_code = error_code;
        self
    }
}

/// Parses a `Retry-After` header in either form RFC 7231 permits.
///
/// AniList sends the delay-seconds form in practice, but the HTTP-date form is
/// legal and a proxy in front of it may rewrite one into the other. Getting
/// this wrong means either hammering a rate-limited API or backing off for
/// decades, so both forms are handled.
pub fn parse_retry_after(value: &str, now: UnixTimestamp) -> Option<u32> {
    let value = value.trim();

    if let Ok(seconds) = value.parse::<u32>() {
        return Some(seconds);
    }

    let deadline = chrono::DateTime::parse_from_rfc2822(value).ok()?;
    let delta = deadline.timestamp() - now.get();
    u32::try_from(delta.max(0)).ok()
}

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

#[cfg(test)]
mod tests {
    use super::*;

    fn at(seconds: i64) -> UnixTimestamp {
        UnixTimestamp::new(seconds).expect("valid timestamp")
    }

    #[test]
    fn outcome_strings_match_the_schema_check() {
        let expected = [
            "success",
            "http_error",
            "graphql_error",
            "decode_error",
            "transport_error",
            "timeout",
            "too_large",
            "integrity_error",
        ];
        let actual = [
            FetchOutcome::Success.as_str(),
            FetchOutcome::HttpError.as_str(),
            FetchOutcome::GraphQlError.as_str(),
            FetchOutcome::DecodeError.as_str(),
            FetchOutcome::TransportError.as_str(),
            FetchOutcome::Timeout.as_str(),
            FetchOutcome::TooLarge.as_str(),
            FetchOutcome::IntegrityError.as_str(),
        ];
        assert_eq!(actual, expected);
    }

    #[test]
    fn only_bodyless_outcomes_permit_an_absent_body() {
        for outcome in [
            FetchOutcome::TransportError,
            FetchOutcome::Timeout,
            FetchOutcome::TooLarge,
        ] {
            assert!(outcome.permits_absent_body(), "{outcome:?}");
        }
        for outcome in [
            FetchOutcome::Success,
            FetchOutcome::HttpError,
            FetchOutcome::GraphQlError,
            FetchOutcome::DecodeError,
            FetchOutcome::IntegrityError,
        ] {
            assert!(!outcome.permits_absent_body(), "{outcome:?}");
        }
    }

    #[test]
    fn integrity_errors_are_never_retried() {
        // Retrying an identical request that produced a contradictory response
        // just spends rate limit to get the same contradiction.
        assert!(!FetchOutcome::IntegrityError.is_retryable());
        assert!(!FetchOutcome::GraphQlError.is_retryable());
    }

    #[test]
    fn retry_after_parses_the_seconds_form() {
        assert_eq!(parse_retry_after("120", at(1_000)), Some(120));
        assert_eq!(parse_retry_after("  60 ", at(1_000)), Some(60));
        assert_eq!(parse_retry_after("0", at(1_000)), Some(0));
    }

    #[test]
    fn retry_after_parses_the_http_date_form() {
        // 2015-10-21T07:28:00Z
        let deadline = 1_445_412_480;
        let now = at(deadline - 300);
        assert_eq!(
            parse_retry_after("Wed, 21 Oct 2015 07:28:00 GMT", now),
            Some(300)
        );
    }

    #[test]
    fn a_past_http_date_clamps_to_zero_rather_than_underflowing() {
        let deadline = 1_445_412_480;
        assert_eq!(
            parse_retry_after("Wed, 21 Oct 2015 07:28:00 GMT", at(deadline + 500)),
            Some(0)
        );
    }

    #[test]
    fn unparseable_retry_after_is_ignored() {
        // Falling back to the normal backoff beats inventing a delay.
        assert_eq!(parse_retry_after("soon", at(1_000)), None);
        assert_eq!(parse_retry_after("", at(1_000)), None);
        assert_eq!(parse_retry_after("-5", at(1_000)), None);
    }

    #[test]
    fn reclassification_preserves_the_stored_body() {
        let response = RawResponse {
            outcome: FetchOutcome::Success,
            http_status: Some(200),
            body: Some("{}".into()),
            byte_length: Some(2),
            retry_after_secs: None,
            rate_limit_remaining: Some(88),
            rate_limit_reset_at: Some(1_000),
            duration_ms: 12,
            error_code: None,
        };
        let reclassified =
            response.reclassified(FetchOutcome::GraphQlError, Some("validation".into()));

        assert_eq!(reclassified.outcome, FetchOutcome::GraphQlError);
        assert_eq!(reclassified.body.as_deref(), Some("{}"));
        assert_eq!(reclassified.rate_limit_remaining, Some(88));
        assert_eq!(reclassified.error_code.as_deref(), Some("validation"));
    }
}
