//! iTunes Search API parser for media and artist candidates.

use anyhow::{Context, Result};
use serde::Deserialize;

use crate::ids::ReleaseKind;
use crate::ingest::{
    AliasObservation, ExternalIdObservation, ImageObservation, LinkObservation, RawSourcePayload,
    ReleaseEventObservation, SourceObservation, SourceParser, TimePrecision,
};

pub struct ItunesParser;

impl SourceParser for ItunesParser {
    fn source(&self) -> &'static str {
        "itunes"
    }

    fn parse_search(&self, payload: &RawSourcePayload) -> Result<Vec<SourceObservation>> {
        let resp: SearchResponse =
            serde_json::from_str(&payload.response_json).context("parse iTunes search response")?;
        resp.results
            .into_iter()
            .filter_map(|result| result_to_observation(result, payload).transpose())
            .collect()
    }

    fn parse_fetch(&self, payload: &RawSourcePayload) -> Result<Option<SourceObservation>> {
        let resp: SearchResponse =
            serde_json::from_str(&payload.response_json).context("parse iTunes lookup response")?;
        match resp.results.into_iter().next() {
            Some(result) => result_to_observation(result, payload),
            None => Ok(None),
        }
    }
}

#[derive(Debug, Deserialize)]
struct SearchResponse {
    #[serde(default)]
    results: Vec<ItunesResult>,
}

#[derive(Debug, Deserialize)]
struct ItunesResult {
    #[serde(rename = "wrapperType", default)]
    wrapper_type: Option<String>,
    #[serde(default)]
    kind: Option<String>,
    #[serde(rename = "artistId", default)]
    artist_id: Option<i64>,
    #[serde(rename = "collectionId", default)]
    collection_id: Option<i64>,
    #[serde(rename = "trackId", default)]
    track_id: Option<i64>,
    #[serde(rename = "artistName", default)]
    artist_name: Option<String>,
    #[serde(rename = "collectionName", default)]
    collection_name: Option<String>,
    #[serde(rename = "trackName", default)]
    track_name: Option<String>,
    #[serde(rename = "primaryGenreName", default)]
    primary_genre_name: Option<String>,
    #[serde(rename = "releaseDate", default)]
    release_date: Option<String>,
    #[serde(rename = "artistLinkUrl", default)]
    artist_link_url: Option<String>,
    #[serde(rename = "collectionViewUrl", default)]
    collection_view_url: Option<String>,
    #[serde(rename = "trackViewUrl", default)]
    track_view_url: Option<String>,
    #[serde(rename = "artworkUrl100", default)]
    artwork_url_100: Option<String>,
    #[serde(default)]
    description: Option<String>,
    #[serde(rename = "longDescription", default)]
    long_description: Option<String>,
    #[serde(rename = "shortDescription", default)]
    short_description: Option<String>,
}

fn result_to_observation(
    result: ItunesResult,
    payload: &RawSourcePayload,
) -> Result<Option<SourceObservation>> {
    let Some(source_id) = pick_source_id(&result) else {
        return Ok(None);
    };
    let kind = release_kind(&result);
    let display_title = display_title(&result).unwrap_or_else(|| source_id.clone());

    let mut aliases = Vec::new();
    push_alias(&mut aliases, result.track_name.as_deref(), "track", 0.9);
    push_alias(
        &mut aliases,
        result.collection_name.as_deref(),
        "collection",
        0.85,
    );
    push_alias(&mut aliases, result.artist_name.as_deref(), "artist", 0.8);
    push_alias(
        &mut aliases,
        result.primary_genre_name.as_deref(),
        "genre",
        0.45,
    );
    push_alias(&mut aliases, result.kind.as_deref(), "itunes_kind", 0.35);

    let mut external_ids = Vec::new();
    if let Some(id) = result.artist_id {
        external_ids.push(external("itunes_artist_id", id));
    }
    if let Some(id) = result.collection_id {
        external_ids.push(external("itunes_collection_id", id));
    }
    if let Some(id) = result.track_id {
        external_ids.push(external("itunes_track_id", id));
    }

    let mut links = Vec::new();
    push_link(&mut links, "artist", result.artist_link_url.as_deref());
    push_link(
        &mut links,
        "collection",
        result.collection_view_url.as_deref(),
    );
    push_link(&mut links, "track", result.track_view_url.as_deref());

    let images = result
        .artwork_url_100
        .as_ref()
        .map(|url| {
            vec![ImageObservation {
                image_kind: "artwork_100".into(),
                url: url.clone(),
                width: Some(100),
                height: Some(100),
            }]
        })
        .unwrap_or_default();

    let release_events = match result.release_date.as_deref() {
        Some(date) => vec![release_date_event(&source_id, date, payload.fetched_at)?],
        None => vec![],
    };

    Ok(Some(SourceObservation {
        source: "itunes".into(),
        source_id,
        raw_payload_id: payload.id.clone(),
        kind,
        display_title,
        raw_title: result.track_name.clone().or(result.collection_name.clone()),
        description: result
            .long_description
            .clone()
            .or(result.description.clone())
            .or(result.short_description.clone()),
        status: None,
        observed_at: payload.fetched_at,
        source_updated_at: None,
        aliases,
        external_ids,
        release_events,
        links,
        images,
    }))
}

fn pick_source_id(result: &ItunesResult) -> Option<String> {
    if result.wrapper_type.as_deref() == Some("artist") {
        return result.artist_id.map(|id| format!("artist:{id}"));
    }
    result
        .track_id
        .map(|id| format!("track:{id}"))
        .or_else(|| result.collection_id.map(|id| format!("collection:{id}")))
        .or_else(|| result.artist_id.map(|id| format!("artist:{id}")))
}

fn release_kind(result: &ItunesResult) -> ReleaseKind {
    let kind = result.kind.as_deref().unwrap_or_default();
    let wrapper = result.wrapper_type.as_deref().unwrap_or_default();
    if wrapper == "artist" {
        ReleaseKind::MusicArtist
    } else if kind.contains("movie") || wrapper == "movie" {
        ReleaseKind::Film
    } else if kind.contains("tv") {
        ReleaseKind::Tv
    } else {
        ReleaseKind::MusicArtist
    }
}

fn display_title(result: &ItunesResult) -> Option<String> {
    if result.wrapper_type.as_deref() == Some("artist") {
        return result.artist_name.clone();
    }
    result
        .collection_name
        .clone()
        .or(result.track_name.clone())
        .or(result.artist_name.clone())
}

fn push_alias(
    aliases: &mut Vec<AliasObservation>,
    value: Option<&str>,
    alias_kind: &str,
    confidence: f64,
) {
    if let Some(value) = value.filter(|v| !v.trim().is_empty()) {
        aliases.push(AliasObservation {
            alias: value.to_string(),
            locale: None,
            alias_kind: Some(alias_kind.into()),
            confidence,
        });
    }
}

fn external(kind: &str, id: i64) -> ExternalIdObservation {
    ExternalIdObservation {
        id_kind: kind.into(),
        id_value: id.to_string(),
        confidence: 1.0,
    }
}

fn push_link(links: &mut Vec<LinkObservation>, kind: &str, url: Option<&str>) {
    if let Some(url) = url {
        links.push(LinkObservation {
            site: "itunes".into(),
            url: url.into(),
            link_kind: Some(kind.into()),
        });
    }
}

fn release_date_event(
    source_id: &str,
    date: &str,
    observed_at: i64,
) -> Result<ReleaseEventObservation> {
    let scheduled_at = chrono::DateTime::parse_from_rfc3339(date)
        .map(|dt| dt.timestamp())
        .or_else(|_| {
            chrono::NaiveDate::parse_from_str(date, "%Y-%m-%d").map(|d| {
                d.and_hms_opt(0, 0, 0)
                    .expect("midnight is valid")
                    .and_utc()
                    .timestamp()
            })
        })
        .with_context(|| format!("parse iTunes releaseDate {date:?}"))?;
    Ok(ReleaseEventObservation {
        id: format!("itunes:release:{source_id}"),
        event_kind: "release".into(),
        title: None,
        season: None,
        episode: None,
        local_date: Some(date.chars().take(10).collect()),
        local_time: None,
        source_timezone: Some("UTC".into()),
        scheduled_at: Some(scheduled_at),
        precision: TimePrecision::Date,
        confidence: 0.8,
        observed_at,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ingest::HttpMethod;

    fn raw(body: &str) -> RawSourcePayload {
        RawSourcePayload {
            id: "raw:itunes:1".into(),
            source: "itunes".into(),
            endpoint: "search".into(),
            method: HttpMethod::Get,
            request_key: "itunes:search".into(),
            request_hash: "req".into(),
            request_json: None,
            http_status: 200,
            response_hash: "resp".into(),
            response_json: body.into(),
            fetched_at: 1_000,
            expires_at: None,
            created_at: 1_000,
        }
    }

    #[test]
    fn parses_artist_search_result() {
        let body = r#"{
            "resultCount": 1,
            "results": [{
                "wrapperType": "artist",
                "artistType": "Artist",
                "artistName": "Taylor Swift",
                "artistLinkUrl": "https://music.apple.com/us/artist/taylor-swift/159260351",
                "artistId": 159260351,
                "primaryGenreName": "Pop"
            }]
        }"#;

        let out = ItunesParser.parse_search(&raw(body)).unwrap();
        assert_eq!(out.len(), 1);
        let artist = &out[0];
        assert_eq!(artist.kind, ReleaseKind::MusicArtist);
        assert_eq!(artist.source_id, "artist:159260351");
        assert_eq!(artist.display_title, "Taylor Swift");
        assert!(artist.aliases.iter().any(|a| a.alias == "Pop"));
        assert!(artist
            .links
            .iter()
            .any(|l| l.link_kind.as_deref() == Some("artist")));
    }

    #[test]
    fn parses_movie_result_with_release_date_and_artwork() {
        let body = r#"{
            "resultCount": 1,
            "results": [{
                "wrapperType": "track",
                "kind": "feature-movie",
                "trackId": 123,
                "trackName": "Dune: Part Two",
                "artistName": "Denis Villeneuve",
                "trackViewUrl": "https://itunes.apple.com/movie/dune-part-two/id123",
                "artworkUrl100": "https://is1-ssl.mzstatic.com/image/thumb.jpg",
                "releaseDate": "2024-03-01T08:00:00Z",
                "primaryGenreName": "Sci-Fi & Fantasy",
                "longDescription": "Paul joins Chani."
            }]
        }"#;

        let out = ItunesParser.parse_search(&raw(body)).unwrap();
        let movie = &out[0];
        assert_eq!(movie.kind, ReleaseKind::Film);
        assert_eq!(movie.source_id, "track:123");
        assert_eq!(movie.display_title, "Dune: Part Two");
        assert_eq!(movie.release_events[0].scheduled_at, Some(1_709_280_000));
        assert_eq!(movie.images[0].width, Some(100));
    }
}
