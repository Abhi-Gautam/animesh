//! Canonical media facts and the source observations they are projected from.

use serde::{Deserialize, Serialize};

use super::ids::{AniListId, BoundedText, EpisodeNumber, IdError, SourceKey, UnixTimestamp};

/// Version of the parser that produced an observation.
///
/// Bumped whenever the mapping from a source payload to [`MediaObservation`]
/// changes in a way that would produce different facts from the same bytes.
/// Stored per observation so a reprocessing pass can tell old rows from new.
pub const PARSER_VERSION: i64 = 1;

/// Maximum stored length of a title column, matching the `V0001` schema.
pub const MAX_TITLE_LEN: usize = 512;

/// Maximum stored length of a raw source enum value.
pub const MAX_RAW_LEN: usize = 64;

/// Normalized release status.
///
/// `Unknown` is a real, expected value rather than an error: a source may add a
/// status at any time, and section 12 requires ingestion to keep working when
/// it does. The unrecognized original is preserved alongside in `status_raw`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MediaStatus {
    Releasing,
    NotYetReleased,
    Finished,
    Cancelled,
    Hiatus,
    Unknown,
}

impl MediaStatus {
    /// Stable string used in the database and on the wire.
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Releasing => "releasing",
            Self::NotYetReleased => "not_yet_released",
            Self::Finished => "finished",
            Self::Cancelled => "cancelled",
            Self::Hiatus => "hiatus",
            Self::Unknown => "unknown",
        }
    }

    /// Decodes the stored form, rejecting anything unrecognized.
    ///
    /// Unlike [`MediaStatus::normalize`], this is strict: a value that is not a
    /// known encoding means the database is corrupt or from a newer schema, not
    /// that a source invented a status.
    pub fn parse(value: &str) -> Result<Self, IdError> {
        match value {
            "releasing" => Ok(Self::Releasing),
            "not_yet_released" => Ok(Self::NotYetReleased),
            "finished" => Ok(Self::Finished),
            "cancelled" => Ok(Self::Cancelled),
            "hiatus" => Ok(Self::Hiatus),
            "unknown" => Ok(Self::Unknown),
            other => Err(IdError::UnknownSource(other.to_owned())),
        }
    }

    /// Maps a raw AniList `MediaStatus` to the normalized form.
    ///
    /// Anything unrecognized — including `None` — becomes [`MediaStatus::Unknown`].
    pub fn normalize(raw: Option<&str>) -> Self {
        match raw {
            Some("RELEASING") => Self::Releasing,
            Some("NOT_YET_RELEASED") => Self::NotYetReleased,
            Some("FINISHED") => Self::Finished,
            Some("CANCELLED") => Self::Cancelled,
            Some("HIATUS") => Self::Hiatus,
            _ => Self::Unknown,
        }
    }

    /// Whether the title may still produce new episodes.
    ///
    /// Section 12 treats unknown and null as active-but-uncertain, so they
    /// refresh on the active cadence rather than the 7-day terminal cadence.
    pub const fn may_still_air(self) -> bool {
        match self {
            Self::Releasing | Self::NotYetReleased | Self::Hiatus | Self::Unknown => true,
            Self::Finished | Self::Cancelled => false,
        }
    }
}

/// The next scheduled episode, as a single indivisible fact.
///
/// The episode number and its airtime are always present together or absent
/// together. The `V0001` schema enforces this with a `CHECK`; representing them
/// as one struct means Rust code cannot construct the half-populated state that
/// the `CHECK` exists to reject.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct NextAiring {
    pub episode: EpisodeNumber,
    pub airing_at: UnixTimestamp,
}

/// Title variants as reported by a source.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct TitleSet {
    pub english: Option<BoundedText>,
    pub romaji: Option<BoundedText>,
    pub native: Option<BoundedText>,
}

impl TitleSet {
    /// Picks the display title: English, then romaji, then native.
    ///
    /// Carried forward from the prototype, which matched what the owner
    /// actually wants to read in a menu bar.
    pub fn display_title(&self) -> Option<&BoundedText> {
        self.english
            .as_ref()
            .or(self.romaji.as_ref())
            .or(self.native.as_ref())
    }
}

/// One parsed observation of a media item from a source at a point in time.
///
/// This is Silver: normalized source facts, not yet reconciled into the user
/// graph. Every successfully parsed item produces one of these, including when
/// the facts are identical to the previous observation — the occurrence itself
/// is evidence that the source still asserted them.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MediaObservation {
    pub source_key: SourceKey,
    pub display_title: BoundedText,
    pub titles: TitleSet,
    pub status: MediaStatus,
    /// Original source status when it did not normalize, for diagnostics.
    pub status_raw: Option<BoundedText>,
    /// Original source format string; not normalized in Plan 001.
    pub format_raw: Option<BoundedText>,
    pub episode_count: Option<EpisodeNumber>,
    pub season_year: Option<i32>,
    pub next_airing: Option<NextAiring>,
    pub parser_version: i64,
}

/// A transient search result.
///
/// Search results are not evidence: they never mutate the user graph, so they
/// are not persisted and carry no observation identity.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SearchCandidate {
    pub anilist_id: AniListId,
    pub display_title: BoundedText,
    pub titles: TitleSet,
    pub status: MediaStatus,
    pub format: Option<BoundedText>,
    pub episode_count: Option<EpisodeNumber>,
    pub season_year: Option<i32>,
}

/// Smallest and largest season years the schema accepts.
pub const MIN_SEASON_YEAR: i32 = 1900;
/// Largest season year the schema accepts.
pub const MAX_SEASON_YEAR: i32 = 9999;

/// Clamps a source-provided season year to the stored range.
///
/// Returns `None` for values outside it rather than failing the whole item: a
/// nonsense year is not a reason to drop a schedule.
pub fn normalize_season_year(raw: Option<i64>) -> Option<i32> {
    let year = raw?;
    let year = i32::try_from(year).ok()?;
    (MIN_SEASON_YEAR..=MAX_SEASON_YEAR)
        .contains(&year)
        .then_some(year)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn text(value: &str) -> BoundedText {
        BoundedText::new("test", MAX_TITLE_LEN, value).expect("valid text")
    }

    #[test]
    fn status_encodings_are_stable() {
        for status in [
            MediaStatus::Releasing,
            MediaStatus::NotYetReleased,
            MediaStatus::Finished,
            MediaStatus::Cancelled,
            MediaStatus::Hiatus,
            MediaStatus::Unknown,
        ] {
            assert_eq!(MediaStatus::parse(status.as_str()), Ok(status));
        }
    }

    #[test]
    fn status_parse_is_strict_about_stored_values() {
        // A value the database should never contain means corruption or a newer
        // schema, both of which must surface rather than silently normalize.
        assert!(MediaStatus::parse("RELEASING").is_err());
        assert!(MediaStatus::parse("").is_err());
    }

    #[test]
    fn status_normalizes_every_known_anilist_value() {
        assert_eq!(
            MediaStatus::normalize(Some("RELEASING")),
            MediaStatus::Releasing
        );
        assert_eq!(
            MediaStatus::normalize(Some("NOT_YET_RELEASED")),
            MediaStatus::NotYetReleased
        );
        assert_eq!(
            MediaStatus::normalize(Some("FINISHED")),
            MediaStatus::Finished
        );
        assert_eq!(
            MediaStatus::normalize(Some("CANCELLED")),
            MediaStatus::Cancelled
        );
        assert_eq!(MediaStatus::normalize(Some("HIATUS")), MediaStatus::Hiatus);
    }

    #[test]
    fn unknown_and_missing_status_normalize_without_failing() {
        // Section 12: an invented source status must not crash ingestion.
        assert_eq!(
            MediaStatus::normalize(Some("ASCENDED")),
            MediaStatus::Unknown
        );
        assert_eq!(MediaStatus::normalize(None), MediaStatus::Unknown);
    }

    #[test]
    fn unknown_status_is_treated_as_possibly_still_airing() {
        // Treating unknown as terminal would silently stop refreshing a show.
        assert!(MediaStatus::Unknown.may_still_air());
        assert!(MediaStatus::Releasing.may_still_air());
        assert!(MediaStatus::NotYetReleased.may_still_air());
        assert!(MediaStatus::Hiatus.may_still_air());
        assert!(!MediaStatus::Finished.may_still_air());
        assert!(!MediaStatus::Cancelled.may_still_air());
    }

    #[test]
    fn display_title_prefers_english_then_romaji_then_native() {
        let all = TitleSet {
            english: Some(text("English")),
            romaji: Some(text("Romaji")),
            native: Some(text("ネイティブ")),
        };
        assert_eq!(
            all.display_title().map(BoundedText::as_str),
            Some("English")
        );

        let no_english = TitleSet {
            english: None,
            ..all.clone()
        };
        assert_eq!(
            no_english.display_title().map(BoundedText::as_str),
            Some("Romaji")
        );

        let native_only = TitleSet {
            english: None,
            romaji: None,
            native: Some(text("ネイティブ")),
        };
        assert_eq!(
            native_only.display_title().map(BoundedText::as_str),
            Some("ネイティブ")
        );

        assert_eq!(TitleSet::default().display_title(), None);
    }

    #[test]
    fn season_year_normalizes_within_schema_range() {
        assert_eq!(normalize_season_year(Some(1999)), Some(1999));
        assert_eq!(normalize_season_year(Some(1900)), Some(1900));
        assert_eq!(normalize_season_year(Some(9999)), Some(9999));
    }

    #[test]
    fn season_year_outside_range_becomes_none_not_an_error() {
        // A bad year must not cost us the airtime that came with it.
        assert_eq!(normalize_season_year(Some(1899)), None);
        assert_eq!(normalize_season_year(Some(10_000)), None);
        assert_eq!(normalize_season_year(Some(i64::MAX)), None);
        assert_eq!(normalize_season_year(None), None);
    }

    #[test]
    fn next_airing_cannot_be_half_populated() {
        // This is the type-level counterpart of the V0001 CHECK constraint:
        // there is no way to build a NextAiring with only one of the two fields.
        let airing = NextAiring {
            episode: EpisodeNumber::new(12).expect("valid episode"),
            airing_at: UnixTimestamp::new(1_783_865_760).expect("valid timestamp"),
        };
        assert_eq!(airing.episode.get(), 12);
    }

    #[test]
    fn observation_round_trips_through_json() {
        let observation = MediaObservation {
            source_key: SourceKey::anilist(AniListId::new(21).expect("valid id")),
            display_title: text("One Piece"),
            titles: TitleSet {
                english: Some(text("One Piece")),
                romaji: Some(text("ONE PIECE")),
                native: Some(text("ワンピース")),
            },
            status: MediaStatus::Releasing,
            status_raw: None,
            format_raw: Some(text("TV")),
            episode_count: None,
            season_year: Some(1999),
            next_airing: Some(NextAiring {
                episode: EpisodeNumber::new(1169).expect("valid episode"),
                airing_at: UnixTimestamp::new(1_783_865_760).expect("valid timestamp"),
            }),
            parser_version: PARSER_VERSION,
        };
        let json = serde_json::to_string(&observation).expect("serialize");
        let decoded: MediaObservation = serde_json::from_str(&json).expect("deserialize");
        assert_eq!(decoded, observation);
    }
}
