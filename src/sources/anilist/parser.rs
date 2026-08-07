//! Maps AniList wire shapes onto domain observations.
//!
//! Pure: no I/O, no storage, no clock. Every function here is total over the
//! `dto` types, so a hostile or broken payload produces a classification rather
//! than a panic.

use std::collections::BTreeMap;

use thiserror::Error;

use super::dto;
use crate::domain::ids::{AniListId, BoundedText, EpisodeNumber, SourceKey, UnixTimestamp};
use crate::domain::media::{
    normalize_season_year, MediaObservation, MediaStatus, NextAiring, SearchCandidate, TitleSet,
    MAX_RAW_LEN, MAX_TITLE_LEN, PARSER_VERSION,
};

/// Why a single item could not become an observation.
#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum ItemError {
    #[error("item has no id")]
    MissingId,

    #[error("item id {0} is outside the valid range")]
    InvalidId(i64),

    #[error("item was null")]
    NullItem,

    #[error("item has no usable display title")]
    UnusableTitle,

    #[error("nextAiringEpisode is half-populated: episode={episode:?} airingAt={airing_at:?}")]
    PartialNextAiring {
        episode: Option<i64>,
        airing_at: Option<i64>,
    },

    #[error("nextAiringEpisode has an unusable value: episode={episode:?} airingAt={airing_at:?}")]
    UnusableNextAiring {
        episode: Option<i64>,
        airing_at: Option<i64>,
    },
}

/// Why an entire batch response must be discarded.
///
/// These are integrity failures, not item failures: the response does not
/// answer the question that was asked, so applying any part of it risks writing
/// one media's facts onto another.
#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum BatchIntegrityError {
    #[error("response contained id {0} twice")]
    DuplicateId(i64),

    #[error("response contained id {0}, which was not requested")]
    UnrequestedId(i64),
}

/// What happened to one requested ID.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ItemResult {
    Observed(Box<MediaObservation>),
    /// Requested but absent from the response.
    ///
    /// Explicitly distinct from an observation carrying no schedule: section 12
    /// requires omission to preserve the existing projection, while an explicit
    /// null withdraws it.
    Missing,
    Invalid(ItemError),
}

/// What happened to a single-item detail request.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DetailResult {
    Observed(Box<MediaObservation>),
    /// AniList answered with `Media: null` — the ID does not exist.
    NotFound,
    Invalid(ItemError),
    /// The response described a different media than was requested.
    IdMismatch {
        requested: i64,
        returned: i64,
    },
}

fn bounded(value: Option<&str>, max: usize) -> Option<BoundedText> {
    value
        .map(str::trim)
        .filter(|v| !v.is_empty())
        .and_then(|v| BoundedText::truncating(max, v))
}

fn title_set(title: Option<&dto::MediaTitle>) -> TitleSet {
    let Some(title) = title else {
        return TitleSet::default();
    };
    TitleSet {
        english: bounded(title.english.as_deref(), MAX_TITLE_LEN),
        romaji: bounded(title.romaji.as_deref(), MAX_TITLE_LEN),
        native: bounded(title.native.as_deref(), MAX_TITLE_LEN),
    }
}

/// Resolves the display title, falling back to the AniList ID.
///
/// A title-less item still carries a schedule worth notifying about, so losing
/// the whole observation would cost real function. `AniList #21` at least tells
/// the owner what to look up; a generic placeholder would not.
fn display_title(titles: &TitleSet, id: AniListId) -> Result<BoundedText, ItemError> {
    if let Some(title) = titles.display_title() {
        return Ok(title.clone());
    }
    BoundedText::truncating(MAX_TITLE_LEN, &format!("AniList #{id}"))
        .ok_or(ItemError::UnusableTitle)
}

fn next_airing(raw: Option<dto::NextAiringEpisode>) -> Result<Option<NextAiring>, ItemError> {
    let Some(raw) = raw else {
        return Ok(None);
    };
    match (raw.episode, raw.airing_at) {
        (None, None) => Ok(None),
        (Some(episode), Some(airing_at)) => {
            let episode =
                EpisodeNumber::new(episode).map_err(|_| ItemError::UnusableNextAiring {
                    episode: Some(episode),
                    airing_at: Some(airing_at),
                })?;
            let airing_at =
                UnixTimestamp::new(airing_at).map_err(|_| ItemError::UnusableNextAiring {
                    episode: Some(episode.get()),
                    airing_at: Some(airing_at),
                })?;
            Ok(Some(NextAiring { episode, airing_at }))
        }
        // Half-populated is an item failure, not "no schedule". Treating it as
        // absent would look identical to an explicit null and withdraw a real
        // scheduled event on the strength of a malformed payload.
        (episode, airing_at) => Err(ItemError::PartialNextAiring { episode, airing_at }),
    }
}

fn parse_id(raw: Option<i64>) -> Result<AniListId, ItemError> {
    let raw = raw.ok_or(ItemError::MissingId)?;
    AniListId::new(raw).map_err(|_| ItemError::InvalidId(raw))
}

/// Converts one wire item into an observation.
pub fn parse_media(media: &dto::Media) -> Result<MediaObservation, ItemError> {
    let id = parse_id(media.id)?;
    let titles = title_set(media.title.as_ref());
    let display_title = display_title(&titles, id)?;
    let status = MediaStatus::normalize(media.status.as_deref());

    Ok(MediaObservation {
        source_key: SourceKey::anilist(id),
        display_title,
        titles,
        status,
        // Only retained when it failed to normalize; storing a raw value we
        // already understood would be noise in every row.
        status_raw: match status {
            MediaStatus::Unknown => bounded(media.status.as_deref(), MAX_RAW_LEN),
            _ => None,
        },
        format_raw: bounded(media.format.as_deref(), MAX_RAW_LEN),
        episode_count: media.episodes.and_then(|e| EpisodeNumber::new(e).ok()),
        season_year: normalize_season_year(media.season_year),
        next_airing: next_airing(media.next_airing_episode)?,
        parser_version: PARSER_VERSION,
    })
}

/// Converts one wire item into a transient search candidate.
pub fn parse_candidate(media: &dto::Media) -> Result<SearchCandidate, ItemError> {
    let anilist_id = parse_id(media.id)?;
    let titles = title_set(media.title.as_ref());
    let display_title = display_title(&titles, anilist_id)?;

    Ok(SearchCandidate {
        anilist_id,
        display_title,
        titles,
        status: MediaStatus::normalize(media.status.as_deref()),
        format: bounded(media.format.as_deref(), MAX_RAW_LEN),
        episode_count: media.episodes.and_then(|e| EpisodeNumber::new(e).ok()),
        season_year: normalize_season_year(media.season_year),
    })
}

/// Parses a detail response for a specific requested ID.
pub fn parse_detail(requested: AniListId, data: Option<&dto::DetailData>) -> DetailResult {
    let Some(media) = data.and_then(|d| d.media.as_ref()) else {
        return DetailResult::NotFound;
    };

    match parse_media(media) {
        Ok(observation) => {
            let returned = observation.source_key.id;
            if returned == requested {
                DetailResult::Observed(Box::new(observation))
            } else {
                // Applying this would write one show's schedule onto another.
                DetailResult::IdMismatch {
                    requested: requested.get(),
                    returned: returned.get(),
                }
            }
        }
        Err(error) => DetailResult::Invalid(error),
    }
}

/// Parses search results, discarding items that cannot be identified.
///
/// Search is transient, so a bad item is dropped rather than reported: there is
/// no durable state for it to corrupt and nothing for the owner to act on.
pub fn parse_search(data: Option<&dto::PageData>) -> Vec<SearchCandidate> {
    data.and_then(|d| d.page.as_ref())
        .map(|page| {
            page.media
                .iter()
                .flatten()
                .filter_map(|m| parse_candidate(m).ok())
                .collect()
        })
        .unwrap_or_default()
}

/// Maps a batch response onto the IDs that were requested.
///
/// Every requested ID appears in the result exactly once. Returns an integrity
/// error, rather than a partial map, when the response contains a duplicate or
/// an ID nobody asked for.
pub fn parse_batch(
    requested: &[AniListId],
    data: Option<&dto::PageData>,
) -> Result<BTreeMap<AniListId, ItemResult>, BatchIntegrityError> {
    let mut results: BTreeMap<AniListId, ItemResult> = requested
        .iter()
        .map(|id| (*id, ItemResult::Missing))
        .collect();

    let items = data
        .and_then(|d| d.page.as_ref())
        .map(|page| page.media.as_slice())
        .unwrap_or_default();

    for item in items {
        let Some(item) = item else {
            // A null element carries no ID, so it cannot be attributed to a
            // requested item; the ID it would have answered stays Missing.
            continue;
        };

        let parsed = parse_media(item);
        let id = match &parsed {
            Ok(observation) => observation.source_key.id,
            Err(_) => match parse_id(item.id) {
                Ok(id) => id,
                Err(_) => continue,
            },
        };

        match results.get_mut(&id) {
            None => return Err(BatchIntegrityError::UnrequestedId(id.get())),
            Some(slot) => {
                if !matches!(slot, ItemResult::Missing) {
                    return Err(BatchIntegrityError::DuplicateId(id.get()));
                }
                *slot = match parsed {
                    Ok(observation) => ItemResult::Observed(Box::new(observation)),
                    Err(error) => ItemResult::Invalid(error),
                };
            }
        }
    }

    Ok(results)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn id(value: i64) -> AniListId {
        AniListId::new(value).expect("valid id")
    }

    fn media(id: Option<i64>) -> dto::Media {
        dto::Media {
            id,
            title: Some(dto::MediaTitle {
                romaji: Some("ONE PIECE".into()),
                english: Some("One Piece".into()),
                native: Some("ワンピース".into()),
            }),
            status: Some("RELEASING".into()),
            format: Some("TV".into()),
            episodes: None,
            season_year: Some(1999),
            next_airing_episode: Some(dto::NextAiringEpisode {
                episode: Some(1169),
                airing_at: Some(1_783_865_760),
            }),
        }
    }

    fn page(items: Vec<Option<dto::Media>>) -> dto::PageData {
        dto::PageData {
            page: Some(dto::MediaPage { media: items }),
        }
    }

    #[test]
    fn parses_a_complete_item() {
        let observation = parse_media(&media(Some(21))).expect("parse");
        assert_eq!(observation.source_key.id, id(21));
        assert_eq!(observation.display_title.as_str(), "One Piece");
        assert_eq!(observation.status, MediaStatus::Releasing);
        assert_eq!(observation.status_raw, None);
        assert_eq!(observation.season_year, Some(1999));
        let next = observation.next_airing.expect("next airing");
        assert_eq!(next.episode.get(), 1169);
        assert_eq!(next.airing_at.get(), 1_783_865_760);
        assert_eq!(observation.parser_version, PARSER_VERSION);
    }

    #[test]
    fn missing_id_is_an_item_error() {
        assert_eq!(parse_media(&media(None)), Err(ItemError::MissingId));
    }

    #[test]
    fn out_of_range_id_is_an_item_error() {
        assert_eq!(parse_media(&media(Some(0))), Err(ItemError::InvalidId(0)));
        assert_eq!(
            parse_media(&media(Some(2_147_483_648))),
            Err(ItemError::InvalidId(2_147_483_648))
        );
    }

    #[test]
    fn unknown_status_is_retained_raw() {
        let mut m = media(Some(21));
        m.status = Some("ASCENDED".into());
        let observation = parse_media(&m).expect("parse");
        assert_eq!(observation.status, MediaStatus::Unknown);
        assert_eq!(
            observation.status_raw.as_ref().map(BoundedText::as_str),
            Some("ASCENDED")
        );
    }

    #[test]
    fn known_status_stores_no_raw_value() {
        let observation = parse_media(&media(Some(21))).expect("parse");
        assert_eq!(observation.status_raw, None);
    }

    #[test]
    fn null_status_normalizes_without_a_raw_value() {
        let mut m = media(Some(21));
        m.status = None;
        let observation = parse_media(&m).expect("parse");
        assert_eq!(observation.status, MediaStatus::Unknown);
        assert_eq!(observation.status_raw, None);
    }

    #[test]
    fn absent_next_airing_is_none() {
        let mut m = media(Some(21));
        m.next_airing_episode = None;
        assert_eq!(parse_media(&m).expect("parse").next_airing, None);
    }

    #[test]
    fn fully_null_next_airing_is_none() {
        let mut m = media(Some(21));
        m.next_airing_episode = Some(dto::NextAiringEpisode {
            episode: None,
            airing_at: None,
        });
        assert_eq!(parse_media(&m).expect("parse").next_airing, None);
    }

    #[test]
    fn half_populated_next_airing_fails_the_item() {
        // Must not look like "no schedule": that would withdraw a real event.
        let mut m = media(Some(21));
        m.next_airing_episode = Some(dto::NextAiringEpisode {
            episode: Some(5),
            airing_at: None,
        });
        assert!(matches!(
            parse_media(&m),
            Err(ItemError::PartialNextAiring { .. })
        ));

        m.next_airing_episode = Some(dto::NextAiringEpisode {
            episode: None,
            airing_at: Some(1_783_865_760),
        });
        assert!(matches!(
            parse_media(&m),
            Err(ItemError::PartialNextAiring { .. })
        ));
    }

    #[test]
    fn unusable_next_airing_values_fail_the_item() {
        let mut m = media(Some(21));
        m.next_airing_episode = Some(dto::NextAiringEpisode {
            episode: Some(0),
            airing_at: Some(1_783_865_760),
        });
        assert!(matches!(
            parse_media(&m),
            Err(ItemError::UnusableNextAiring { .. })
        ));

        m.next_airing_episode = Some(dto::NextAiringEpisode {
            episode: Some(5),
            airing_at: Some(-1),
        });
        assert!(matches!(
            parse_media(&m),
            Err(ItemError::UnusableNextAiring { .. })
        ));
    }

    #[test]
    fn title_falls_back_through_the_precedence() {
        let mut m = media(Some(21));
        m.title = Some(dto::MediaTitle {
            romaji: Some("ONE PIECE".into()),
            english: None,
            native: Some("ワンピース".into()),
        });
        assert_eq!(
            parse_media(&m).expect("parse").display_title.as_str(),
            "ONE PIECE"
        );

        m.title = Some(dto::MediaTitle {
            romaji: None,
            english: None,
            native: Some("ワンピース".into()),
        });
        assert_eq!(
            parse_media(&m).expect("parse").display_title.as_str(),
            "ワンピース"
        );
    }

    #[test]
    fn titleless_item_falls_back_to_the_anilist_id() {
        // Keeps a notifiable schedule rather than dropping the observation.
        let mut m = media(Some(21));
        m.title = None;
        assert_eq!(
            parse_media(&m).expect("parse").display_title.as_str(),
            "AniList #21"
        );
    }

    #[test]
    fn blank_titles_are_treated_as_absent() {
        let mut m = media(Some(21));
        m.title = Some(dto::MediaTitle {
            romaji: Some("   ".into()),
            english: Some(String::new()),
            native: Some("ワンピース".into()),
        });
        assert_eq!(
            parse_media(&m).expect("parse").display_title.as_str(),
            "ワンピース"
        );
    }

    #[test]
    fn overlong_title_is_truncated_not_rejected() {
        let mut m = media(Some(21));
        m.title = Some(dto::MediaTitle {
            english: Some("x".repeat(900)),
            romaji: None,
            native: None,
        });
        let observation = parse_media(&m).expect("parse");
        assert_eq!(
            observation.display_title.as_str().chars().count(),
            MAX_TITLE_LEN
        );
    }

    #[test]
    fn bad_episode_count_becomes_none_without_failing_the_item() {
        let mut m = media(Some(21));
        m.episodes = Some(0);
        assert_eq!(parse_media(&m).expect("parse").episode_count, None);
        m.episodes = Some(-3);
        assert_eq!(parse_media(&m).expect("parse").episode_count, None);
    }

    #[test]
    fn detail_returns_not_found_for_explicit_null() {
        let data = dto::DetailData { media: None };
        assert_eq!(parse_detail(id(21), Some(&data)), DetailResult::NotFound);
    }

    #[test]
    fn detail_returns_not_found_for_absent_data() {
        assert_eq!(parse_detail(id(21), None), DetailResult::NotFound);
    }

    #[test]
    fn detail_rejects_a_mismatched_id() {
        // A response about a different show must never be applied.
        let data = dto::DetailData {
            media: Some(media(Some(99))),
        };
        assert_eq!(
            parse_detail(id(21), Some(&data)),
            DetailResult::IdMismatch {
                requested: 21,
                returned: 99,
            }
        );
    }

    #[test]
    fn batch_maps_every_requested_id() {
        let requested = [id(21), id(11061)];
        let mut second = media(Some(11061));
        second.next_airing_episode = None;
        let data = page(vec![Some(media(Some(21))), Some(second)]);

        let results = parse_batch(&requested, Some(&data)).expect("no integrity error");
        assert_eq!(results.len(), 2);
        assert!(matches!(results[&id(21)], ItemResult::Observed(_)));
        assert!(matches!(results[&id(11061)], ItemResult::Observed(_)));
    }

    #[test]
    fn omitted_id_is_missing_not_absent_schedule() {
        // The distinction the whole outcome matrix turns on.
        let requested = [id(21), id(11061)];
        let data = page(vec![Some(media(Some(21)))]);
        let results = parse_batch(&requested, Some(&data)).expect("no integrity error");
        assert_eq!(results[&id(11061)], ItemResult::Missing);
    }

    #[test]
    fn duplicate_id_invalidates_the_response() {
        let requested = [id(21)];
        let data = page(vec![Some(media(Some(21))), Some(media(Some(21)))]);
        assert_eq!(
            parse_batch(&requested, Some(&data)),
            Err(BatchIntegrityError::DuplicateId(21))
        );
    }

    #[test]
    fn unrequested_id_invalidates_the_response() {
        let requested = [id(21)];
        let data = page(vec![Some(media(Some(99)))]);
        assert_eq!(
            parse_batch(&requested, Some(&data)),
            Err(BatchIntegrityError::UnrequestedId(99))
        );
    }

    #[test]
    fn an_invalid_item_does_not_cost_its_siblings() {
        let requested = [id(21), id(11061)];
        let mut broken = media(Some(11061));
        broken.next_airing_episode = Some(dto::NextAiringEpisode {
            episode: Some(5),
            airing_at: None,
        });
        let data = page(vec![Some(media(Some(21))), Some(broken)]);

        let results = parse_batch(&requested, Some(&data)).expect("no integrity error");
        assert!(matches!(results[&id(21)], ItemResult::Observed(_)));
        assert!(matches!(results[&id(11061)], ItemResult::Invalid(_)));
    }

    #[test]
    fn a_null_element_leaves_its_id_missing() {
        let requested = [id(21)];
        let data = page(vec![None]);
        let results = parse_batch(&requested, Some(&data)).expect("no integrity error");
        assert_eq!(results[&id(21)], ItemResult::Missing);
    }

    #[test]
    fn an_empty_batch_response_marks_everything_missing() {
        let requested = [id(21), id(11061)];
        let results = parse_batch(&requested, None).expect("no integrity error");
        assert!(results.values().all(|r| *r == ItemResult::Missing));
    }

    #[test]
    fn search_drops_unidentifiable_items() {
        let data = page(vec![Some(media(Some(21))), Some(media(None)), None]);
        let candidates = parse_search(Some(&data));
        assert_eq!(candidates.len(), 1);
        assert_eq!(candidates[0].anilist_id, id(21));
    }

    #[test]
    fn search_of_nothing_is_empty_not_an_error() {
        assert!(parse_search(None).is_empty());
    }

    #[test]
    fn every_normalized_status_survives_a_round_trip() {
        for (raw, expected) in [
            ("RELEASING", MediaStatus::Releasing),
            ("NOT_YET_RELEASED", MediaStatus::NotYetReleased),
            ("FINISHED", MediaStatus::Finished),
            ("CANCELLED", MediaStatus::Cancelled),
            ("HIATUS", MediaStatus::Hiatus),
        ] {
            let mut m = media(Some(21));
            m.status = Some(raw.into());
            assert_eq!(parse_media(&m).expect("parse").status, expected, "{raw}");
        }
    }
}
