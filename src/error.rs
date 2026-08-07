//! Stable error codes and the exit categories they map to.
//!
//! The code is the contract: it crosses the IPC boundary, appears in logs, and
//! decides the CLI's exit status. Error *messages* are for humans and may
//! change freely; codes may not.

use serde::{Deserialize, Serialize};

use crate::domain::ids::IdError;

/// Machine-readable classification of any failure a surface can observe.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ErrorCode {
    InvalidArgument,
    NotFound,
    AlreadyRunning,
    /// The app is running but bootstrap failed permanently.
    BootstrapUnavailable,
    /// The app is not reachable over IPC.
    Unavailable,
    ProtocolMismatch,
    SourceUnavailable,
    SourceRateLimited,
    /// The source returned data that contradicts what was requested.
    SourceIntegrity,
    DatabaseBusy,
    DatabaseReadOnly,
    DatabaseCorrupt,
    SchemaTooNew,
    NotificationPermission,
    NotificationRegistration,
    Internal,
}

impl ErrorCode {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::InvalidArgument => "invalid_argument",
            Self::NotFound => "not_found",
            Self::AlreadyRunning => "already_running",
            Self::BootstrapUnavailable => "bootstrap_unavailable",
            Self::Unavailable => "unavailable",
            Self::ProtocolMismatch => "protocol_mismatch",
            Self::SourceUnavailable => "source_unavailable",
            Self::SourceRateLimited => "source_rate_limited",
            Self::SourceIntegrity => "source_integrity",
            Self::DatabaseBusy => "database_busy",
            Self::DatabaseReadOnly => "database_read_only",
            Self::DatabaseCorrupt => "database_corrupt",
            Self::SchemaTooNew => "schema_too_new",
            Self::NotificationPermission => "notification_permission",
            Self::NotificationRegistration => "notification_registration",
            Self::Internal => "internal",
        }
    }

    /// Whether retrying the same request could plausibly succeed later.
    pub const fn is_retryable(self) -> bool {
        matches!(
            self,
            Self::SourceUnavailable
                | Self::SourceRateLimited
                | Self::DatabaseBusy
                | Self::Unavailable
        )
    }

    pub const fn exit_category(self) -> ExitCategory {
        match self {
            Self::InvalidArgument | Self::NotFound => ExitCategory::User,

            Self::ProtocolMismatch
            | Self::BootstrapUnavailable
            | Self::DatabaseReadOnly
            | Self::DatabaseCorrupt
            | Self::SchemaTooNew
            | Self::NotificationPermission
            | Self::Internal => ExitCategory::Durable,

            Self::AlreadyRunning
            | Self::Unavailable
            | Self::SourceUnavailable
            | Self::SourceRateLimited
            | Self::SourceIntegrity
            | Self::DatabaseBusy
            | Self::NotificationRegistration => ExitCategory::Temporary,
        }
    }
}

/// The CLI's four exit statuses, frozen by section 15.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[repr(i32)]
pub enum ExitCategory {
    Success = 0,
    /// Usage error, bad input, or a thing that does not exist.
    User = 1,
    /// Configuration, protocol, or durable-state problem needing intervention.
    Durable = 2,
    /// The app or the source is temporarily unavailable; try again.
    Temporary = 3,
}

impl ExitCategory {
    pub const fn code(self) -> i32 {
        self as i32
    }
}

/// An error with a stable code and a human-readable message.
///
/// Deliberately does not carry a source chain: this type crosses the IPC
/// boundary, and section 15 keeps raw chains in the log rather than shipping
/// internal detail to a client.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, thiserror::Error)]
#[error("{code}: {message}", code = self.code.as_str())]
pub struct AppError {
    pub code: ErrorCode,
    pub message: String,
    /// Present when the caller can usefully wait before retrying.
    pub retry_after_secs: Option<u32>,
}

impl AppError {
    pub fn new(code: ErrorCode, message: impl Into<String>) -> Self {
        Self {
            code,
            message: message.into(),
            retry_after_secs: None,
        }
    }

    pub fn with_retry_after(mut self, seconds: u32) -> Self {
        self.retry_after_secs = Some(seconds);
        self
    }

    pub fn invalid_argument(message: impl Into<String>) -> Self {
        Self::new(ErrorCode::InvalidArgument, message)
    }

    pub fn not_found(message: impl Into<String>) -> Self {
        Self::new(ErrorCode::NotFound, message)
    }

    pub fn internal(message: impl Into<String>) -> Self {
        Self::new(ErrorCode::Internal, message)
    }

    pub const fn exit_category(&self) -> ExitCategory {
        self.code.exit_category()
    }
}

impl From<IdError> for AppError {
    fn from(value: IdError) -> Self {
        // Every IdError is a validation failure by construction, so the mapping
        // is total rather than a default-to-internal fallback.
        Self::invalid_argument(value.to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const ALL_CODES: &[ErrorCode] = &[
        ErrorCode::InvalidArgument,
        ErrorCode::NotFound,
        ErrorCode::AlreadyRunning,
        ErrorCode::BootstrapUnavailable,
        ErrorCode::Unavailable,
        ErrorCode::ProtocolMismatch,
        ErrorCode::SourceUnavailable,
        ErrorCode::SourceRateLimited,
        ErrorCode::SourceIntegrity,
        ErrorCode::DatabaseBusy,
        ErrorCode::DatabaseReadOnly,
        ErrorCode::DatabaseCorrupt,
        ErrorCode::SchemaTooNew,
        ErrorCode::NotificationPermission,
        ErrorCode::NotificationRegistration,
        ErrorCode::Internal,
    ];

    #[test]
    fn codes_round_trip_through_json() {
        for code in ALL_CODES {
            let json = serde_json::to_string(code).expect("serialize");
            let decoded: ErrorCode = serde_json::from_str(&json).expect("deserialize");
            assert_eq!(decoded, *code);
        }
    }

    #[test]
    fn wire_encoding_matches_the_documented_string() {
        let json = serde_json::to_string(&ErrorCode::SourceRateLimited).expect("serialize");
        assert_eq!(json, "\"source_rate_limited\"");
        assert_eq!(ErrorCode::SourceRateLimited.as_str(), "source_rate_limited");
    }

    #[test]
    fn every_code_maps_to_a_nonzero_exit() {
        // A failure that exits 0 would make scripting Animesh actively unsafe.
        for code in ALL_CODES {
            assert_ne!(
                code.exit_category().code(),
                0,
                "{code:?} maps to a success exit"
            );
        }
    }

    #[test]
    fn exit_categories_match_the_frozen_numbers() {
        assert_eq!(ExitCategory::Success.code(), 0);
        assert_eq!(ExitCategory::User.code(), 1);
        assert_eq!(ExitCategory::Durable.code(), 2);
        assert_eq!(ExitCategory::Temporary.code(), 3);
    }

    #[test]
    fn user_errors_exit_one() {
        assert_eq!(
            ErrorCode::InvalidArgument.exit_category(),
            ExitCategory::User
        );
        assert_eq!(ErrorCode::NotFound.exit_category(), ExitCategory::User);
    }

    #[test]
    fn durable_problems_exit_two() {
        for code in [
            ErrorCode::ProtocolMismatch,
            ErrorCode::SchemaTooNew,
            ErrorCode::DatabaseCorrupt,
            ErrorCode::BootstrapUnavailable,
        ] {
            assert_eq!(code.exit_category(), ExitCategory::Durable, "{code:?}");
        }
    }

    #[test]
    fn transient_problems_exit_three() {
        for code in [
            ErrorCode::Unavailable,
            ErrorCode::SourceUnavailable,
            ErrorCode::SourceRateLimited,
            ErrorCode::DatabaseBusy,
        ] {
            assert_eq!(code.exit_category(), ExitCategory::Temporary, "{code:?}");
        }
    }

    #[test]
    fn retryable_codes_are_all_temporary() {
        // Telling a caller to retry something that exits as durable would send
        // them into a loop that cannot resolve.
        for code in ALL_CODES.iter().filter(|c| c.is_retryable()) {
            assert_eq!(
                code.exit_category(),
                ExitCategory::Temporary,
                "{code:?} is retryable but not temporary"
            );
        }
    }

    #[test]
    fn id_validation_failures_become_invalid_argument() {
        let err: AppError = IdError::OutOfRange {
            kind: "AniListId",
            min: 1,
            max: 2_147_483_647,
            value: 0,
        }
        .into();
        assert_eq!(err.code, ErrorCode::InvalidArgument);
        assert!(err.message.contains("AniListId"));
    }

    #[test]
    fn errors_round_trip_across_the_wire() {
        let err = AppError::new(ErrorCode::SourceRateLimited, "slow down").with_retry_after(60);
        let json = serde_json::to_string(&err).expect("serialize");
        let decoded: AppError = serde_json::from_str(&json).expect("deserialize");
        assert_eq!(decoded, err);
        assert_eq!(decoded.retry_after_secs, Some(60));
    }

    #[test]
    fn display_leads_with_the_stable_code() {
        let err = AppError::not_found("no such media");
        assert_eq!(err.to_string(), "not_found: no such media");
    }
}
