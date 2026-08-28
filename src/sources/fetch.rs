//! Shared transport classification.
//!
//! Every source returns a classified response rather than `Result`. Failures
//! are evidence too: a timeout, a 429, and a well-formed body all become rows
//! in `source_fetches`.

use std::time::Duration;

use crate::domain::ids::UnixTimestamp;

/// Hard ceiling on a response body.
///
/// Enforced while reading rather than after: a server that streams gigabytes
/// must not be able to make us allocate them first.
pub const MAX_BODY_BYTES: usize = 2 * 1024 * 1024;

pub const REQUEST_TIMEOUT: Duration = Duration::from_secs(10);

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
    pub(crate) fn failure(outcome: FetchOutcome, duration: Duration, error_code: &str) -> Self {
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
