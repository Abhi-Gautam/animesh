//! Maps TVmaze wire shapes onto domain observations.

use super::dto::{Episode, Show};
use crate::domain::ids::{BoundedText, EpisodeNumber, SourceKey, TvMazeId, UnixTimestamp};
use crate::domain::media::{
    MediaObservation, MediaStatus, NextAiring, SearchCandidate, TitleSet, MAX_RAW_LEN,
    MAX_TITLE_LEN, PARSER_VERSION,
};

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ItemError {
    InvalidId(Option<i64>),
    UnusableAirstamp(String),
}

impl std::fmt::Display for ItemError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::InvalidId(id) => write!(f, "invalid TVmaze id {id:?}"),
            Self::UnusableAirstamp(stamp) => write!(f, "unusable airstamp {stamp}"),
        }
    }
}

#[derive(Debug)]
pub enum DetailResult {
    Observed(Box<MediaObservation>),
    NotFound,
    Invalid(ItemError),
    IdMismatch { requested: i64, returned: i64 },
}

fn bounded(value: Option<&str>, max: usize) -> Option<BoundedText> {
    value.and_then(|v| BoundedText::truncating(max, v))
}

fn parse_id(raw: Option<i64>) -> Result<TvMazeId, ItemError> {
    let raw = raw.ok_or(ItemError::InvalidId(None))?;
    TvMazeId::new(raw).map_err(|_| ItemError::InvalidId(Some(raw)))
}

fn titles(name: Option<&str>) -> TitleSet {
    TitleSet {
        english: bounded(name, MAX_TITLE_LEN),
        romaji: None,
        native: None,
    }
}

fn display_title(titles: &TitleSet, id: TvMazeId) -> Result<BoundedText, ItemError> {
    if let Some(title) = titles.display_title() {
        return Ok(title.clone());
    }
    BoundedText::truncating(MAX_TITLE_LEN, &format!("TVmaze #{id}"))
        .ok_or(ItemError::InvalidId(Some(id.get())))
}

fn season_year(premiered: Option<&str>) -> Option<i32> {
    let year = premiered?.get(..4)?.parse::<i64>().ok()?;
    crate::domain::media::normalize_season_year(Some(year))
}

fn next_airing(episode: Option<&Episode>) -> Result<Option<NextAiring>, ItemError> {
    let Some(episode) = episode else {
        return Ok(None);
    };
    let Some(number) = episode.number else {
        return Ok(None);
    };
    let Some(stamp) = episode.airstamp.as_deref() else {
        return Ok(None);
    };
    let parsed = chrono::DateTime::parse_from_rfc3339(stamp)
        .map_err(|_| ItemError::UnusableAirstamp(stamp.to_owned()))?;
    let airing_at = UnixTimestamp::new(parsed.timestamp())
        .map_err(|_| ItemError::UnusableAirstamp(stamp.to_owned()))?;
    let ep = EpisodeNumber::new(number).map_err(|_| ItemError::InvalidId(Some(number)))?;
    Ok(Some(NextAiring {
        episode: ep,
        airing_at,
        season: episode.season.filter(|s| *s >= 1),
    }))
}

pub fn parse_show(show: &Show) -> Result<MediaObservation, ItemError> {
    let id = parse_id(show.id)?;
    let titles = titles(show.name.as_deref());
    let display_title = display_title(&titles, id)?;
    let next = next_airing(show.embedded.as_ref().and_then(|e| e.nextepisode.as_ref()))?;
    Ok(MediaObservation {
        source_key: SourceKey::tvmaze(id),
        display_title,
        titles,
        status: MediaStatus::normalize_tvmaze(show.status.as_deref()),
        status_raw: bounded(show.status.as_deref(), MAX_RAW_LEN),
        format_raw: bounded(show.show_type.as_deref(), MAX_RAW_LEN),
        episode_count: None,
        season_year: season_year(show.premiered.as_deref()),
        next_airing: next,
        parser_version: PARSER_VERSION,
    })
}

pub fn parse_candidate(show: &Show) -> Result<SearchCandidate, ItemError> {
    let observation = parse_show(show)?;
    Ok(SearchCandidate {
        source: observation.source_key.source,
        source_id: observation.source_key.id,
        display_title: observation.display_title,
        titles: observation.titles,
        status: observation.status,
        format: observation.format_raw,
        episode_count: observation.episode_count,
        season_year: observation.season_year,
    })
}

pub fn parse_detail(requested: TvMazeId, body: &str) -> DetailResult {
    let show: Show = match serde_json::from_str(body) {
        Ok(show) => show,
        Err(_) => return DetailResult::NotFound,
    };
    match parse_show(&show) {
        Ok(observation) => {
            let returned = observation.source_key.id.get();
            if returned == requested.get() {
                DetailResult::Observed(Box::new(observation))
            } else {
                DetailResult::IdMismatch {
                    requested: requested.get(),
                    returned,
                }
            }
        }
        Err(error) => DetailResult::Invalid(error),
    }
}

/// English-language, currently running shows from a schedule payload.
pub fn parse_simulcast(body: &str) -> Vec<SearchCandidate> {
    let Ok(rows) = serde_json::from_str::<Vec<crate::sources::tvmaze::dto::ScheduleRow>>(body)
    else {
        return Vec::new();
    };
    let mut seen = std::collections::BTreeSet::new();
    let mut out = Vec::new();
    for row in rows {
        let Some(show) = row.show() else {
            continue;
        };
        if show.language.as_deref() != Some("English") {
            continue;
        }
        if show.status.as_deref() != Some("Running") {
            continue;
        }
        // News/sports/talk dominate a US calendar day. The radar is for shows.
        match show.show_type.as_deref() {
            Some("Scripted" | "Animation") => {}
            _ => continue,
        }
        let Ok(id) = parse_id(show.id) else {
            continue;
        };
        if !seen.insert(id) {
            continue;
        }
        if let Ok(candidate) = parse_candidate(show) {
            out.push(candidate);
        }
        if out.len() >= 20 {
            break;
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn show_json() -> &'static str {
        r#"{"id":82,"name":"Game of Thrones","type":"Scripted","language":"English","status":"Ended","premiered":"2011-04-17","_embedded":null}"#
    }

    #[test]
    fn parses_an_ended_show_without_a_next_episode() {
        let observation =
            parse_show(&serde_json::from_str(show_json()).expect("dto")).expect("parse");
        assert_eq!(observation.source_key.to_string(), "tvmaze:82");
        assert_eq!(observation.status, MediaStatus::Finished);
        assert!(observation.next_airing.is_none());
    }

    #[test]
    fn seasonal_next_airing_carries_the_season() {
        let show: Show = serde_json::from_str(
            r#"{"id":1,"name":"X","status":"Running","_embedded":{"nextepisode":{"season":4,"number":20,"airstamp":"2026-08-29T01:00:00+00:00"}}}"#,
        )
        .expect("dto");
        let next = parse_show(&show).expect("parse").next_airing.expect("next");
        assert_eq!(next.episode.get(), 20);
        assert_eq!(next.season, Some(4));
    }

    #[test]
    fn simulcast_keeps_english_running_shows_and_drops_the_rest() {
        let body = r#"[
            {"show":{"id":1,"name":"Network","type":"Scripted","language":"English","status":"Running"}},
            {"show":{"id":2,"name":"Donghua","type":"Scripted","language":"Chinese","status":"Running"}},
            {"show":{"id":3,"name":"Ended","type":"Scripted","language":"English","status":"Ended"}},
            {"show":{"id":4,"name":"News","type":"News","language":"English","status":"Running"}},
            {"show":{"id":1,"name":"Network","type":"Scripted","language":"English","status":"Running"}}
        ]"#;
        let hits = parse_simulcast(body);
        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0].source_id.get(), 1);
    }

    #[test]
    fn simulcast_reads_an_inlined_broadcast_show_and_an_embedded_web_show() {
        let body = r#"[
            {"show":{"id":10,"name":"Broadcast","type":"Scripted","language":"English","status":"Running"}},
            {"_embedded":{"show":{"id":11,"name":"Stream","type":"Animation","language":"English","status":"Running"}}}
        ]"#;
        let hits = parse_simulcast(body);
        assert_eq!(hits.len(), 2);
        assert_eq!(hits[0].source_id.get(), 10);
        assert_eq!(hits[1].source_id.get(), 11);
    }

    #[test]
    fn simulcast_caps_at_twenty() {
        let rows: Vec<String> = (1..=25)
            .map(|id| {
                format!(
                    r#"{{"_embedded":{{"show":{{"id":{id},"name":"S{id}","type":"Scripted","language":"English","status":"Running"}}}}}}"#
                )
            })
            .collect();
        let body = format!("[{}]", rows.join(","));
        assert_eq!(parse_simulcast(&body).len(), 20);
    }

    #[test]
    fn detail_rejects_an_id_mismatch() {
        let body = r#"{"id":99,"name":"Other","status":"Running"}"#;
        match parse_detail(TvMazeId::new(82).expect("id"), body) {
            DetailResult::IdMismatch {
                requested: 82,
                returned: 99,
            } => {}
            other => panic!("expected mismatch, got {other:?}"),
        }
    }
}
