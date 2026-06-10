//! iTunes Search API parser for media and artist candidates.

use anyhow::{anyhow, Context, Result};
use reqwest::{Client, Url};
use serde::Deserialize;

use crate::ids::ReleaseKind;
use crate::ingest::{
    AliasObservation, ExternalIdObservation, HttpMethod, ImageObservation, LinkObservation,
    RawSourcePayload, ReleaseEventObservation, SourceObservation, SourceParser, TimePrecision,
};
use crate::search::SearchScope;
use crate::sources::{stable_hash, SourceAdapter, SourceFuture};

const DEFAULT_BASE_URL: &str = "https://itunes.apple.com";
const SEARCH_SCOPES: &[SearchScope] = &[SearchScope::Music, SearchScope::Film];
const ENRICHMENT_SCOPES: &[SearchScope] = &[SearchScope::Music, SearchScope::Film];

pub(crate) struct ItunesSource {
    client: Client,
    parser: ItunesParser,
    base_url: String,
}

impl ItunesSource {
    pub(crate) fn new() -> Self {
        Self::with_base_url(DEFAULT_BASE_URL)
    }

    #[allow(dead_code)]
    pub(crate) fn with_base_url(base_url: impl Into<String>) -> Self {
        Self {
            client: Client::new(),
            parser: ItunesParser,
            base_url: base_url.into(),
        }
    }
}

impl Default for ItunesSource {
    fn default() -> Self {
        Self::new()
    }
}

impl SourceAdapter for ItunesSource {
    fn source(&self) -> &'static str {
        "itunes"
    }

    fn parser(&self) -> &dyn SourceParser {
        &self.parser
    }

    fn search_scopes(&self) -> &'static [SearchScope] {
        SEARCH_SCOPES
    }

    fn enrichment_scopes(&self) -> &'static [SearchScope] {
        ENRICHMENT_SCOPES
    }

    fn search<'a>(
        &'a self,
        query: &'a str,
        limit: u32,
        now: i64,
    ) -> SourceFuture<'a, Vec<RawSourcePayload>> {
        Box::pin(async move {
            let url = url_with_params(
                &self.base_url,
                "search",
                &[("term", query.to_string()), ("limit", limit.to_string())],
            )?;
            let response_json = get_json(&self.client, url.as_str()).await?;
            Ok(vec![raw_payload(
                "search",
                &format!("itunes:search:{query}:limit:{limit}"),
                response_json,
                now,
            )])
        })
    }

    fn ingest<'a>(
        &'a self,
        source_id: &'a str,
        now: i64,
    ) -> SourceFuture<'a, Option<RawSourcePayload>> {
        Box::pin(async move {
            let lookup_id = source_id
                .rsplit_once(':')
                .map(|(_, id)| id)
                .unwrap_or(source_id);
            let url = url_with_params(&self.base_url, "lookup", &[("id", lookup_id.to_string())])?;
            let response_json = get_json(&self.client, url.as_str()).await?;
            Ok(Some(raw_payload(
                "lookup",
                &format!("itunes:lookup:{source_id}"),
                response_json,
                now,
            )))
        })
    }
}

pub(crate) struct ItunesParser;

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

async fn get_json(client: &Client, url: &str) -> Result<String> {
    let resp = client
        .get(url)
        .send()
        .await
        .with_context(|| format!("GET {url}"))?;
    let status = resp.status();
    let body = resp.text().await.context("read iTunes response body")?;
    if !status.is_success() {
        return Err(anyhow!("iTunes HTTP {status}: {body}"));
    }
    Ok(body)
}

fn raw_payload(
    endpoint: &str,
    request_key: &str,
    response_json: String,
    now: i64,
) -> RawSourcePayload {
    let request_hash = stable_hash(request_key);
    let response_hash = stable_hash(&response_json);
    RawSourcePayload {
        id: format!("raw:itunes:{endpoint}:{request_hash}:{response_hash}"),
        source: "itunes".into(),
        endpoint: endpoint.into(),
        method: HttpMethod::Get,
        request_key: request_key.into(),
        request_hash,
        request_json: None,
        http_status: 200,
        response_hash,
        response_json,
        fetched_at: now,
        expires_at: None,
        created_at: now,
    }
}

fn url_with_path(base_url: &str, path: &str) -> Result<Url> {
    let base = format!("{}/", base_url.trim_end_matches('/'));
    Url::parse(&base)
        .context("parse iTunes base URL")?
        .join(path)
        .with_context(|| format!("join iTunes path {path:?}"))
}

fn url_with_params(base_url: &str, path: &str, params: &[(&str, String)]) -> Result<Url> {
    let mut url = url_with_path(base_url, path)?;
    url.query_pairs_mut()
        .extend_pairs(params.iter().map(|(k, v)| (*k, v.as_str())));
    Ok(url)
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
