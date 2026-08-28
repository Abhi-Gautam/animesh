//! TVmaze transport. REST GET, no API key.

use std::fmt::Write;
use std::time::Instant;

use crate::domain::ids::UnixTimestamp;
use crate::sources::fetch::{
    parse_retry_after, FetchOutcome, RawResponse, MAX_BODY_BYTES, REQUEST_TIMEOUT,
};

pub const DEFAULT_BASE_URL: &str = "https://api.tvmaze.com";

const USER_AGENT: &str = concat!(
    "animesh/",
    env!("CARGO_PKG_VERSION"),
    " (https://github.com/Abhi-Gautam/animesh)"
);

#[derive(Debug)]
pub struct TvMazeClient {
    client: reqwest::Client,
    base_url: String,
}

impl TvMazeClient {
    pub fn new(base_url: impl Into<String>) -> Result<Self, reqwest::Error> {
        let client = reqwest::Client::builder()
            .timeout(REQUEST_TIMEOUT)
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

    pub async fn get(&self, path_and_query: &str, now: UnixTimestamp) -> RawResponse {
        let started = Instant::now();
        let url = format!("{}{path_and_query}", self.base_url.trim_end_matches('/'));
        let response = match self.client.get(&url).send().await {
            Ok(response) => response,
            Err(error) => {
                let outcome = if error.is_timeout() {
                    FetchOutcome::Timeout
                } else {
                    FetchOutcome::TransportError
                };
                return RawResponse::failure(
                    outcome,
                    started.elapsed(),
                    if error.is_timeout() {
                        "timeout"
                    } else {
                        "transport"
                    },
                );
            }
        };

        let http_status = response.status().as_u16();
        let retry_after_secs = response
            .headers()
            .get(reqwest::header::RETRY_AFTER)
            .and_then(|v| v.to_str().ok())
            .and_then(|v| parse_retry_after(v, now));

        if http_status == 404 {
            let mut result =
                RawResponse::failure(FetchOutcome::Success, started.elapsed(), "not_found");
            result.http_status = Some(404);
            result.body = Some(String::new());
            result.byte_length = Some(0);
            result.retry_after_secs = retry_after_secs;
            return result;
        }

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
                    return RawResponse::failure(outcome, started.elapsed(), "body");
                }
            }
        }

        let body = String::from_utf8_lossy(&buffer).into_owned();
        let outcome = if (200..300).contains(&http_status) {
            FetchOutcome::Success
        } else {
            FetchOutcome::HttpError
        };
        RawResponse {
            outcome,
            http_status: Some(http_status),
            byte_length: Some(body.len()),
            body: Some(body),
            retry_after_secs,
            rate_limit_remaining: None,
            rate_limit_reset_at: None,
            duration_ms: started.elapsed().as_millis() as u64,
            error_code: None,
        }
    }
}

/// Encodes a TVmaze `q=` value as `application/x-www-form-urlencoded`.
///
/// Spaces become `+`. Anything else that is not unreserved is percent-encoded,
/// so a query cannot inject extra parameters or path segments.
pub(crate) fn encode_query(query: &str) -> String {
    let mut out = String::with_capacity(query.len());
    for &b in query.as_bytes() {
        match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'.' | b'_' | b'~' => {
                out.push(b as char);
            }
            b' ' => out.push('+'),
            _ => {
                let _ = write!(out, "%{b:02X}");
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn query_encoding_does_not_let_ampersands_escape_the_parameter() {
        assert_eq!(encode_query("game of thrones"), "game+of+thrones");
        assert_eq!(encode_query("foo&embed=cast"), "foo%26embed%3Dcast");
        assert_eq!(encode_query("søster"), "s%C3%B8ster");
    }
}
