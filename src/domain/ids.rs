//! Validated identifier and timestamp newtypes.
//!
//! These ranges are frozen. Every value that crosses a trust
//! boundary — AniList payloads, IPC frames, CLI arguments, SQLite rows — is
//! parsed into one of these types, so downstream code never re-validates and
//! an out-of-range value cannot be represented at all.

use std::fmt;

use serde::{Deserialize, Serialize};
use thiserror::Error;

/// Upper bound AniList uses for media and episode identifiers (`i32::MAX`).
///
/// Stored as `i64` because that is SQLite's native integer width; the bound is
/// what the source guarantees, not what the storage can hold.
pub const MAX_SOURCE_INT: i64 = 2_147_483_647;

/// Failure to construct a validated identifier.
#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum IdError {
    #[error("{kind} must be between {min} and {max}, got {value}")]
    OutOfRange {
        kind: &'static str,
        min: i64,
        max: i64,
        value: i64,
    },

    #[error("timestamp {0} is not representable as a UTC instant")]
    UnrepresentableTimestamp(i64),

    #[error("unknown source {0:?}")]
    UnknownSource(String),

    #[error("{kind} must be 1..={max} characters, got {len}")]
    BadLength {
        kind: &'static str,
        max: usize,
        len: usize,
    },
}

/// Defines a newtype over `i64` that can only hold values within `$min..=$max`.
///
/// Serde goes through `try_from`/`into` so a hostile IPC frame or a corrupt row
/// fails to deserialize rather than producing an out-of-range value.
macro_rules! bounded_id {
    (
        $(#[$meta:meta])*
        $name:ident, $min:expr, $max:expr
    ) => {
        $(#[$meta])*
        #[derive(
            Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize,
        )]
        #[serde(try_from = "i64", into = "i64")]
        pub struct $name(i64);

        impl $name {
            pub const MIN: i64 = $min;
            pub const MAX: i64 = $max;

            pub fn new(value: i64) -> Result<Self, IdError> {
                if (Self::MIN..=Self::MAX).contains(&value) {
                    Ok(Self(value))
                } else {
                    Err(IdError::OutOfRange {
                        kind: stringify!($name),
                        min: Self::MIN,
                        max: Self::MAX,
                        value,
                    })
                }
            }

            pub const fn get(self) -> i64 {
                self.0
            }
        }

        impl TryFrom<i64> for $name {
            type Error = IdError;

            fn try_from(value: i64) -> Result<Self, Self::Error> {
                Self::new(value)
            }
        }

        impl From<$name> for i64 {
            fn from(value: $name) -> Self {
                value.0
            }
        }

        impl fmt::Display for $name {
            fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                fmt::Display::fmt(&self.0, f)
            }
        }

        impl std::str::FromStr for $name {
            type Err = IdError;

            fn from_str(s: &str) -> Result<Self, Self::Err> {
                let value: i64 = s.trim().parse().map_err(|_| IdError::OutOfRange {
                    kind: stringify!($name),
                    min: Self::MIN,
                    max: Self::MAX,
                    value: 0,
                })?;
                Self::new(value)
            }
        }
    };
}

bounded_id! {
    /// An AniList media identifier.
    AniListId, 1, MAX_SOURCE_INT
}

bounded_id! {
    /// A 1-based episode number as reported by a source.
    EpisodeNumber, 1, MAX_SOURCE_INT
}

bounded_id! {
    /// Canonical media row identifier. Assigned by SQLite, never by a source.
    MediaId, 1, i64::MAX
}

bounded_id! {
    /// Release event row identifier. Pairs with an immutable [`EventUuid`].
    ReleaseEventId, 1, i64::MAX
}

bounded_id! {
    /// `source_media` row identifier.
    SourceMediaId, 1, i64::MAX
}

bounded_id! {
    /// `source_fetches` row identifier.
    FetchId, 1, i64::MAX
}

bounded_id! {
    /// `source_observations` row identifier.
    ObservationId, 1, i64::MAX
}

/// A nonnegative Unix timestamp in UTC seconds that `chrono` can represent.
///
/// Both directions matter: a source may hand us any nonnegative integer, and
/// every current wall-clock instant must round-trip. Values that
/// are nonnegative but outside `chrono`'s range are rejected here rather than
/// panicking at the point of formatting.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(try_from = "i64", into = "i64")]
pub struct UnixTimestamp(i64);

impl UnixTimestamp {
    /// The Unix epoch, `1970-01-01T00:00:00Z`.
    pub const EPOCH: Self = Self(0);

    /// Constructs a timestamp, rejecting negative and unrepresentable values.
    pub fn new(seconds: i64) -> Result<Self, IdError> {
        if seconds < 0 {
            return Err(IdError::OutOfRange {
                kind: "UnixTimestamp",
                min: 0,
                max: i64::MAX,
                value: seconds,
            });
        }
        match chrono::DateTime::from_timestamp(seconds, 0) {
            Some(_) => Ok(Self(seconds)),
            None => Err(IdError::UnrepresentableTimestamp(seconds)),
        }
    }

    pub const fn get(self) -> i64 {
        self.0
    }

    /// Infallible: [`UnixTimestamp::new`] already proved representability.
    pub fn to_utc(self) -> chrono::DateTime<chrono::Utc> {
        chrono::DateTime::from_timestamp(self.0, 0).unwrap_or_default()
    }

    /// Returns `self + seconds`, saturating at the representable maximum.
    pub fn saturating_add_secs(self, seconds: i64) -> Self {
        Self::new(self.0.saturating_add(seconds)).unwrap_or(Self(self.0))
    }

    /// Returns `other - self` in seconds; negative when `other` is in the past.
    pub const fn seconds_until(self, other: Self) -> i64 {
        other.0 - self.0
    }
}

impl TryFrom<i64> for UnixTimestamp {
    type Error = IdError;

    fn try_from(value: i64) -> Result<Self, Self::Error> {
        Self::new(value)
    }
}

impl From<UnixTimestamp> for i64 {
    fn from(value: UnixTimestamp) -> Self {
        value.0
    }
}

impl fmt::Display for UnixTimestamp {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        fmt::Display::fmt(&self.0, f)
    }
}

/// An external data source.
///
/// AniList is the only source. The enum exists because the stored encoding must
/// be explicit and fallibly decoded, not because a second source is planned.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Source {
    AniList,
}

impl Source {
    /// Stable string used in the database and on the wire.
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::AniList => "anilist",
        }
    }

    /// Decodes the stored form, rejecting anything unrecognized.
    pub fn parse(value: &str) -> Result<Self, IdError> {
        match value {
            "anilist" => Ok(Self::AniList),
            other => Err(IdError::UnknownSource(other.to_owned())),
        }
    }
}

impl fmt::Display for Source {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

/// A source paired with its identifier within that source.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub struct SourceKey {
    pub source: Source,
    pub id: AniListId,
}

impl SourceKey {
    pub const fn anilist(id: AniListId) -> Self {
        Self {
            source: Source::AniList,
            id,
        }
    }
}

impl fmt::Display for SourceKey {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}:{}", self.source, self.id)
    }
}

/// An immutable UUID assigned to a release event when it is first observed.
///
/// The OS notification identifier is derived from this, so it must never change
/// for the life of the event — that is what makes re-registration idempotent.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct EventUuid(uuid::Uuid);

impl EventUuid {
    /// Generates a fresh event UUID.
    pub fn generate() -> Self {
        Self(uuid::Uuid::new_v4())
    }

    pub const fn from_uuid(value: uuid::Uuid) -> Self {
        Self(value)
    }

    pub const fn as_uuid(&self) -> &uuid::Uuid {
        &self.0
    }
}

impl fmt::Display for EventUuid {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        fmt::Display::fmt(&self.0, f)
    }
}

/// A UUID identifying this installation, minted once at database creation.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct InstallationUuid(uuid::Uuid);

impl InstallationUuid {
    /// Generates a fresh installation UUID.
    pub fn generate() -> Self {
        Self(uuid::Uuid::new_v4())
    }

    pub const fn from_uuid(value: uuid::Uuid) -> Self {
        Self(value)
    }

    pub const fn as_uuid(&self) -> &uuid::Uuid {
        &self.0
    }
}

impl fmt::Display for InstallationUuid {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        fmt::Display::fmt(&self.0, f)
    }
}

/// A bounded, non-empty string used where the schema caps a text column.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct BoundedText(String);

impl BoundedText {
    /// Constructs a bounded string, rejecting empty and over-long input.
    pub fn new(kind: &'static str, max: usize, value: impl Into<String>) -> Result<Self, IdError> {
        let value = value.into();
        let len = value.chars().count();
        if len == 0 || len > max {
            return Err(IdError::BadLength { kind, max, len });
        }
        Ok(Self(value))
    }

    /// Truncates to `max` characters on a character boundary.
    ///
    /// Used for source-provided strings where rejecting the whole payload over a
    /// long title would lose real data. Returns `None` only for empty input.
    pub fn truncating(max: usize, value: &str) -> Option<Self> {
        let truncated: String = value.chars().take(max).collect();
        if truncated.is_empty() {
            None
        } else {
            Some(Self(truncated))
        }
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }

    pub fn into_inner(self) -> String {
        self.0
    }
}

impl fmt::Display for BoundedText {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn anilist_id_accepts_boundaries() {
        assert_eq!(AniListId::new(1).map(AniListId::get), Ok(1));
        assert_eq!(
            AniListId::new(MAX_SOURCE_INT).map(AniListId::get),
            Ok(MAX_SOURCE_INT)
        );
    }

    #[test]
    fn anilist_id_rejects_outside_boundaries() {
        assert!(AniListId::new(0).is_err());
        assert!(AniListId::new(-1).is_err());
        assert!(AniListId::new(MAX_SOURCE_INT + 1).is_err());
    }

    #[test]
    fn episode_number_rejects_zero() {
        // AniList episodes are 1-based; a zero would silently mean "unknown".
        assert!(EpisodeNumber::new(0).is_err());
        assert!(EpisodeNumber::new(1).is_ok());
    }

    #[test]
    fn media_id_rejects_nonpositive() {
        assert!(MediaId::new(0).is_err());
        assert!(MediaId::new(-5).is_err());
        assert!(MediaId::new(1).is_ok());
    }

    #[test]
    fn ids_reject_out_of_range_during_deserialization() {
        // The bound has to hold across serde, or a hostile IPC frame bypasses it.
        assert!(serde_json::from_str::<AniListId>("0").is_err());
        assert!(serde_json::from_str::<AniListId>("2147483648").is_err());
        assert_eq!(
            serde_json::from_str::<AniListId>("21")
                .ok()
                .map(AniListId::get),
            Some(21)
        );
    }

    #[test]
    fn ids_round_trip_through_json() {
        let id = AniListId::new(21).expect("valid id");
        let json = serde_json::to_string(&id).expect("serialize");
        assert_eq!(json, "21");
        assert_eq!(serde_json::from_str::<AniListId>(&json).ok(), Some(id));
    }

    #[test]
    fn id_parses_from_str_for_cli_arguments() {
        assert_eq!("21".parse::<AniListId>().map(AniListId::get), Ok(21));
        assert!("0".parse::<AniListId>().is_err());
        assert!("not-a-number".parse::<AniListId>().is_err());
    }

    #[test]
    fn timestamp_rejects_negative() {
        assert!(UnixTimestamp::new(-1).is_err());
        assert!(UnixTimestamp::new(0).is_ok());
    }

    #[test]
    fn timestamp_rejects_unrepresentable_values() {
        // Nonnegative but far outside chrono's range: must fail here, not at
        // the point where the menu tries to format it.
        assert!(matches!(
            UnixTimestamp::new(i64::MAX),
            Err(IdError::UnrepresentableTimestamp(_))
        ));
    }

    #[test]
    fn timestamp_converts_to_utc() {
        let ts = UnixTimestamp::new(1_783_865_760).expect("valid timestamp");
        assert_eq!(ts.to_utc().timestamp(), 1_783_865_760);
    }

    #[test]
    fn timestamp_arithmetic_is_saturating() {
        let ts = UnixTimestamp::new(1_000).expect("valid timestamp");
        assert_eq!(ts.saturating_add_secs(500).get(), 1_500);
        // Overflow must not panic and must not wrap into the past.
        assert!(ts.saturating_add_secs(i64::MAX).get() >= 1_000);
    }

    #[test]
    fn timestamp_difference_is_signed() {
        let earlier = UnixTimestamp::new(100).expect("valid timestamp");
        let later = UnixTimestamp::new(400).expect("valid timestamp");
        assert_eq!(earlier.seconds_until(later), 300);
        assert_eq!(later.seconds_until(earlier), -300);
    }

    #[test]
    fn source_encoding_is_stable() {
        assert_eq!(Source::AniList.as_str(), "anilist");
        assert_eq!(Source::parse("anilist"), Ok(Source::AniList));
    }

    #[test]
    fn source_decoding_rejects_unknown() {
        assert!(matches!(
            Source::parse("myanimelist"),
            Err(IdError::UnknownSource(_))
        ));
        // Case matters: the stored form is exactly lowercase.
        assert!(Source::parse("AniList").is_err());
    }

    #[test]
    fn bounded_text_rejects_empty_and_overlong() {
        assert!(BoundedText::new("title", 8, "").is_err());
        assert!(BoundedText::new("title", 8, "123456789").is_err());
        assert!(BoundedText::new("title", 8, "12345678").is_ok());
    }

    #[test]
    fn bounded_text_counts_characters_not_bytes() {
        // The schema caps `length(display_title)`, which SQLite measures in
        // characters for TEXT. Counting bytes would reject valid Japanese titles.
        let title = "ワンピース";
        assert_eq!(title.chars().count(), 5);
        assert!(BoundedText::new("title", 5, title).is_ok());
    }

    #[test]
    fn bounded_text_truncates_on_character_boundary() {
        let truncated = BoundedText::truncating(3, "ワンピース").expect("non-empty");
        assert_eq!(truncated.as_str(), "ワンピ");
        assert!(BoundedText::truncating(3, "").is_none());
    }

    #[test]
    fn event_uuids_are_unique() {
        assert_ne!(EventUuid::generate(), EventUuid::generate());
    }
}
