//! TVMaze parser.
//!
//! TVMaze is the first non-AniList source because it gives clean TV
//! identity, external IDs, web-channel metadata, and episode `airstamp`
//! values that exercise the timezone boundary.

use anyhow::{anyhow, Context, Result};
use reqwest::{Client, Url};
use serde::Deserialize;

use crate::ids::ReleaseKind;
use crate::ingest::{
    AliasObservation, ExternalIdObservation, HttpMethod, ImageObservation, LinkObservation,
    RawSourcePayload, ReleaseEventObservation, SourceObservation, SourceParser, TimePrecision,
};
use crate::sources::{stable_hash, SourceAdapter, SourceFuture};

const DEFAULT_BASE_URL: &str = "https://api.tvmaze.com";

pub(crate) struct TvMazeSource {
    client: Client,
    parser: TvMazeParser,
    base_url: String,
}

impl TvMazeSource {
    pub(crate) fn new() -> Self {
        Self::with_base_url(DEFAULT_BASE_URL)
    }

    #[allow(dead_code)]
    pub(crate) fn with_base_url(base_url: impl Into<String>) -> Self {
        Self {
            client: Client::new(),
            parser: TvMazeParser,
            base_url: base_url.into(),
        }
    }
}

impl Default for TvMazeSource {
    fn default() -> Self {
        Self::new()
    }
}

impl SourceAdapter for TvMazeSource {
    fn source(&self) -> &'static str {
        "tvmaze"
    }

    fn parser(&self) -> &dyn SourceParser {
        &self.parser
    }

    fn search<'a>(
        &'a self,
        query: &'a str,
        _limit: u32,
        now: i64,
    ) -> SourceFuture<'a, Vec<RawSourcePayload>> {
        Box::pin(async move {
            let url = url_with_params(&self.base_url, "search/shows", &[("q", query.to_string())])?;
            let response_json = get_json(&self.client, url.as_str()).await?;
            Ok(vec![raw_payload(
                "search_shows",
                &format!("tvmaze:search_shows:{query}"),
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
            let url = url_with_params(
                &self.base_url,
                &format!("shows/{source_id}"),
                &[("embed", "episodes".to_string())],
            )?;
            let response_json = get_json(&self.client, url.as_str()).await?;
            Ok(Some(raw_payload(
                "show",
                &format!("tvmaze:show:{source_id}:embed:episodes"),
                response_json,
                now,
            )))
        })
    }
}

pub(crate) struct TvMazeParser;

impl SourceParser for TvMazeParser {
    fn source(&self) -> &'static str {
        "tvmaze"
    }

    fn parse_search(&self, payload: &RawSourcePayload) -> Result<Vec<SourceObservation>> {
        let rows: Vec<SearchHit> =
            serde_json::from_str(&payload.response_json).context("parse TVMaze search response")?;
        rows.into_iter()
            .map(|hit| show_to_observation(hit.show, payload))
            .collect()
    }

    fn parse_fetch(&self, payload: &RawSourcePayload) -> Result<Option<SourceObservation>> {
        let show: Show =
            serde_json::from_str(&payload.response_json).context("parse TVMaze show response")?;
        Ok(Some(show_to_observation(show, payload)?))
    }
}

#[derive(Debug, Deserialize)]
struct SearchHit {
    show: Show,
}

#[derive(Debug, Deserialize)]
struct Show {
    id: i64,
    name: String,
    #[serde(default)]
    language: Option<String>,
    #[serde(default)]
    genres: Vec<String>,
    #[serde(default)]
    status: Option<String>,

    #[serde(rename = "officialSite", default)]
    official_site: Option<String>,
    #[serde(default)]
    network: Option<Channel>,
    #[serde(rename = "webChannel", default)]
    web_channel: Option<Channel>,
    #[serde(default)]
    externals: Option<Externals>,
    #[serde(default)]
    image: Option<Image>,
    #[serde(default)]
    summary: Option<String>,
    #[serde(rename = "_embedded", default)]
    embedded: Option<Embedded>,
}

#[derive(Debug, Deserialize)]
struct Embedded {
    #[serde(default)]
    episodes: Vec<Episode>,
}

#[derive(Debug, Deserialize)]
struct Channel {
    name: String,
    #[serde(rename = "officialSite", default)]
    official_site: Option<String>,
}

#[derive(Debug, Deserialize)]
struct Externals {
    #[serde(default)]
    tvrage: Option<i64>,
    #[serde(default)]
    thetvdb: Option<i64>,
    #[serde(default)]
    imdb: Option<String>,
}

#[derive(Debug, Deserialize)]
struct Image {
    #[serde(default)]
    medium: Option<String>,
    #[serde(default)]
    original: Option<String>,
}

#[derive(Debug, Deserialize)]
struct Episode {
    id: i64,
    #[serde(default)]
    name: Option<String>,
    #[serde(default)]
    season: Option<i64>,
    #[serde(default)]
    number: Option<i64>,
    #[serde(default)]
    airdate: Option<String>,
    #[serde(default)]
    airtime: Option<String>,
    #[serde(default)]
    airstamp: Option<String>,
}

async fn get_json(client: &Client, url: &str) -> Result<String> {
    let resp = client
        .get(url)
        .send()
        .await
        .with_context(|| format!("GET {url}"))?;
    let status = resp.status();
    let body = resp.text().await.context("read TVMaze response body")?;
    if !status.is_success() {
        return Err(anyhow!("TVMaze HTTP {status}: {body}"));
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
        id: format!("raw:tvmaze:{endpoint}:{request_hash}:{response_hash}"),
        source: "tvmaze".into(),
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
        .context("parse TVMaze base URL")?
        .join(path)
        .with_context(|| format!("join TVMaze path {path:?}"))
}

fn url_with_params(base_url: &str, path: &str, params: &[(&str, String)]) -> Result<Url> {
    let mut url = url_with_path(base_url, path)?;
    url.query_pairs_mut()
        .extend_pairs(params.iter().map(|(k, v)| (*k, v.as_str())));
    Ok(url)
}

fn show_to_observation(show: Show, payload: &RawSourcePayload) -> Result<SourceObservation> {
    let source_id = show.id.to_string();
    let mut aliases = Vec::new();
    if let Some(language) = &show.language {
        aliases.push(AliasObservation {
            alias: show.name.clone(),
            locale: Some(language.clone()),
            alias_kind: Some("primary".into()),
            confidence: 1.0,
        });
    }
    for genre in &show.genres {
        aliases.push(AliasObservation {
            alias: genre.clone(),
            locale: None,
            alias_kind: Some("genre".into()),
            confidence: 0.6,
        });
    }

    let mut external_ids = Vec::new();
    if let Some(ext) = &show.externals {
        if let Some(id) = ext.tvrage {
            external_ids.push(external("tvrage", id.to_string(), 0.9));
        }
        if let Some(id) = ext.thetvdb {
            external_ids.push(external("thetvdb", id.to_string(), 1.0));
        }
        if let Some(id) = &ext.imdb {
            external_ids.push(external("imdb", id.clone(), 1.0));
        }
    }

    let mut links = Vec::new();
    if let Some(url) = &show.official_site {
        links.push(LinkObservation {
            site: "official".into(),
            url: url.clone(),
            link_kind: Some("official_site".into()),
        });
    }
    if let Some(channel) = show.web_channel.as_ref().or(show.network.as_ref()) {
        if let Some(url) = &channel.official_site {
            links.push(LinkObservation {
                site: channel.name.clone(),
                url: url.clone(),
                link_kind: Some("channel".into()),
            });
        }
    }

    let mut images = Vec::new();
    if let Some(image) = &show.image {
        if let Some(url) = &image.medium {
            images.push(image_obs("poster_medium", url));
        }
        if let Some(url) = &image.original {
            images.push(image_obs("poster_original", url));
        }
    }

    let mut release_events = Vec::new();
    if let Some(embedded) = &show.embedded {
        for ep in &embedded.episodes {
            release_events.push(episode_to_event(ep, payload.fetched_at)?);
        }
    }

    // TVMaze descriptions are small HTML snippets. Strip tags enough for
    // downstream search/context; detailed sanitization can happen in UI.
    let description = show.summary.as_deref().map(strip_html);

    Ok(SourceObservation {
        source: "tvmaze".into(),
        source_id,
        raw_payload_id: payload.id.clone(),
        kind: ReleaseKind::Tv,
        display_title: show.name,
        raw_title: None,
        description,
        status: show.status,
        observed_at: payload.fetched_at,
        source_updated_at: None,
        aliases,
        external_ids,
        release_events,
        links,
        images,
    })
}

fn episode_to_event(ep: &Episode, observed_at: i64) -> Result<ReleaseEventObservation> {
    let scheduled_at = match ep.airstamp.as_deref() {
        Some(s) => Some(
            chrono::DateTime::parse_from_rfc3339(s)
                .with_context(|| format!("parse TVMaze episode airstamp {s:?}"))?
                .timestamp(),
        ),
        None => None,
    };
    let (precision, local_date, local_time, source_timezone) = match ep.airstamp.as_deref() {
        Some(s) => (
            TimePrecision::Instant,
            ep.airdate.clone(),
            ep.airtime.clone().filter(|t| !t.is_empty()),
            offset_suffix(s),
        ),
        None if ep.airdate.is_some() => (TimePrecision::Date, ep.airdate.clone(), None, None),
        None => (TimePrecision::Unknown, None, None, None),
    };
    Ok(ReleaseEventObservation {
        id: format!("tvmaze:episode:{}", ep.id),
        event_kind: "episode".into(),
        title: ep.name.clone(),
        season: ep.season,
        episode: ep.number,
        local_date,
        local_time,
        source_timezone,
        scheduled_at,
        precision,
        confidence: if scheduled_at.is_some() { 0.95 } else { 0.7 },
        observed_at,
    })
}

fn external(kind: &str, value: String, confidence: f64) -> ExternalIdObservation {
    ExternalIdObservation {
        id_kind: kind.into(),
        id_value: value,
        confidence,
    }
}

fn image_obs(kind: &str, url: &str) -> ImageObservation {
    ImageObservation {
        image_kind: kind.into(),
        url: url.into(),
        width: None,
        height: None,
    }
}

fn strip_html(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut in_tag = false;
    for c in s.chars() {
        match c {
            '<' => in_tag = true,
            '>' => in_tag = false,
            _ if !in_tag => out.push(c),
            _ => {}
        }
    }
    out.trim().to_string()
}

fn offset_suffix(rfc3339: &str) -> Option<String> {
    if rfc3339.ends_with('Z') {
        return Some("Z".into());
    }
    rfc3339
        .rsplit_once(['+', '-'])
        .map(|(_, suffix)| suffix)
        .filter(|suffix| suffix.len() == 5 && suffix.as_bytes().get(2) == Some(&b':'))
        .map(|_| {
            let sign_pos = rfc3339.len().saturating_sub(6);
            rfc3339[sign_pos..].to_string()
        })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ingest::HttpMethod;

    fn raw(body: &str) -> RawSourcePayload {
        RawSourcePayload {
            id: "raw:1".into(),
            source: "tvmaze".into(),
            endpoint: "search_shows".into(),
            method: HttpMethod::Get,
            request_key: "tvmaze:search:severance".into(),
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
    fn parses_search_result_identity_and_external_ids() {
        let body = r#"[{"score":1.0,"show":{"id":44933,"name":"Severance","language":"English","genres":["Drama","Science-Fiction"],"status":"Running","premiered":"2022-02-18","officialSite":"https://tv.apple.com/show/severance","webChannel":{"name":"Apple TV","officialSite":"https://tv.apple.com/"},"externals":{"tvrage":null,"thetvdb":371980,"imdb":"tt11280740"},"image":{"medium":"https://img/medium.jpg","original":"https://img/original.jpg"},"summary":"<p>Hello</p>"}}]"#;
        let out = TvMazeParser.parse_search(&raw(body)).unwrap();
        assert_eq!(out.len(), 1);
        let s = &out[0];
        assert_eq!(s.source_id, "44933");
        assert_eq!(s.display_title, "Severance");
        assert_eq!(s.description.as_deref(), Some("Hello"));
        assert!(s
            .external_ids
            .iter()
            .any(|e| e.id_kind == "imdb" && e.id_value == "tt11280740"));
        assert!(s
            .external_ids
            .iter()
            .any(|e| e.id_kind == "thetvdb" && e.id_value == "371980"));
        assert!(s.links.iter().any(|l| l.site == "Apple TV"));
        assert_eq!(s.images.len(), 2);
    }

    #[test]
    fn parses_embedded_episodes_to_utc_events() {
        let body = r#"{"id":44933,"name":"Severance","language":"English","genres":[],"status":"Running","_embedded":{"episodes":[{"id":2238231,"name":"Good News About Hell","season":1,"number":1,"airdate":"2022-02-18","airtime":"","airstamp":"2022-02-18T12:00:00+00:00"}]}}"#;
        let obs = TvMazeParser.parse_fetch(&raw(body)).unwrap().unwrap();
        assert_eq!(obs.release_events.len(), 1);
        let ev = &obs.release_events[0];
        assert_eq!(ev.id, "tvmaze:episode:2238231");
        assert_eq!(ev.season, Some(1));
        assert_eq!(ev.episode, Some(1));
        assert_eq!(ev.scheduled_at, Some(1_645_185_600));
        assert_eq!(ev.precision, TimePrecision::Instant);
    }
}
