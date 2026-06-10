//! Jikan parser for MyAnimeList anime enrichment observations.

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

const DEFAULT_BASE_URL: &str = "https://api.jikan.moe/v4";
const ANIME_SEARCH_SCOPES: &[SearchScope] = &[SearchScope::Anime];
const NO_ENRICHMENT_SCOPES: &[SearchScope] = &[];

pub(crate) struct JikanSource {
    client: Client,
    parser: JikanParser,
    base_url: String,
}

impl JikanSource {
    pub(crate) fn new() -> Self {
        Self::with_base_url(DEFAULT_BASE_URL)
    }

    #[allow(dead_code)]
    pub(crate) fn with_base_url(base_url: impl Into<String>) -> Self {
        Self {
            client: Client::new(),
            parser: JikanParser,
            base_url: base_url.into(),
        }
    }
}

impl Default for JikanSource {
    fn default() -> Self {
        Self::new()
    }
}

impl SourceAdapter for JikanSource {
    fn source(&self) -> &'static str {
        "jikan"
    }

    fn parser(&self) -> &dyn SourceParser {
        &self.parser
    }

    fn search_scopes(&self) -> &'static [SearchScope] {
        ANIME_SEARCH_SCOPES
    }

    fn enrichment_scopes(&self) -> &'static [SearchScope] {
        NO_ENRICHMENT_SCOPES
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
                "anime",
                &[("q", query.to_string()), ("limit", limit.to_string())],
            )?;
            let response_json = get_json(&self.client, url.as_str()).await?;
            Ok(vec![raw_payload(
                "anime_search",
                &format!("jikan:anime_search:{query}:limit:{limit}"),
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
            let url = url_with_path(&self.base_url, &format!("anime/{source_id}/full"))?;
            let response_json = get_json(&self.client, url.as_str()).await?;
            Ok(Some(raw_payload(
                "anime_full",
                &format!("jikan:anime_full:{source_id}"),
                response_json,
                now,
            )))
        })
    }
}

pub(crate) struct JikanParser;

impl SourceParser for JikanParser {
    fn source(&self) -> &'static str {
        "jikan"
    }

    fn parse_search(&self, payload: &RawSourcePayload) -> Result<Vec<SourceObservation>> {
        let resp: SearchResponse = serde_json::from_str(&payload.response_json)
            .context("parse Jikan anime search response")?;
        resp.data
            .into_iter()
            .map(|anime| anime_to_observation(anime, payload))
            .collect()
    }

    fn parse_fetch(&self, payload: &RawSourcePayload) -> Result<Option<SourceObservation>> {
        let resp: FetchResponse =
            serde_json::from_str(&payload.response_json).context("parse Jikan anime response")?;
        Ok(Some(anime_to_observation(resp.data, payload)?))
    }
}

#[derive(Debug, Deserialize)]
struct SearchResponse {
    #[serde(default)]
    data: Vec<Anime>,
}

#[derive(Debug, Deserialize)]
struct FetchResponse {
    data: Anime,
}

#[derive(Debug, Deserialize)]
struct Anime {
    mal_id: i64,
    #[serde(default)]
    url: Option<String>,
    title: String,
    #[serde(default)]
    title_english: Option<String>,
    #[serde(default)]
    title_japanese: Option<String>,
    #[serde(default)]
    titles: Vec<Title>,
    #[serde(rename = "type", default)]
    anime_type: Option<String>,

    #[serde(default)]
    status: Option<String>,
    #[serde(default)]
    synopsis: Option<String>,
    #[serde(default)]
    aired: Option<Aired>,
    #[serde(default)]
    images: Option<Images>,
    #[serde(default)]
    genres: Vec<NamedResource>,
    #[serde(default)]
    themes: Vec<NamedResource>,
    #[serde(default)]
    demographics: Vec<NamedResource>,
    #[serde(default)]
    studios: Vec<NamedResource>,
    #[serde(default)]
    producers: Vec<NamedResource>,
    #[serde(default)]
    external: Vec<ExternalLink>,
    #[serde(default)]
    streaming: Vec<ExternalLink>,
}

#[derive(Debug, Deserialize)]
struct Title {
    #[serde(rename = "type", default)]
    title_type: Option<String>,
    title: String,
}

#[derive(Debug, Deserialize)]
struct Aired {
    #[serde(default)]
    from: Option<String>,
    #[serde(default)]
    to: Option<String>,
    #[serde(default)]
    string: Option<String>,
}

#[derive(Debug, Deserialize)]
struct Images {
    #[serde(default)]
    jpg: Option<ImageSet>,
    #[serde(default)]
    webp: Option<ImageSet>,
}

#[derive(Debug, Deserialize)]
struct ImageSet {
    #[serde(default)]
    image_url: Option<String>,
    #[serde(default)]
    small_image_url: Option<String>,
    #[serde(default)]
    large_image_url: Option<String>,
}

#[derive(Debug, Deserialize)]
struct NamedResource {
    name: String,
}

#[derive(Debug, Deserialize)]
struct ExternalLink {
    name: String,
    url: String,
}

async fn get_json(client: &Client, url: &str) -> Result<String> {
    let resp = client
        .get(url)
        .send()
        .await
        .with_context(|| format!("GET {url}"))?;
    let status = resp.status();
    let body = resp.text().await.context("read Jikan response body")?;
    if !status.is_success() {
        return Err(anyhow!("Jikan HTTP {status}: {body}"));
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
        id: format!("raw:jikan:{endpoint}:{request_hash}:{response_hash}"),
        source: "jikan".into(),
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
        .context("parse Jikan base URL")?
        .join(path)
        .with_context(|| format!("join Jikan path {path:?}"))
}

fn url_with_params(base_url: &str, path: &str, params: &[(&str, String)]) -> Result<Url> {
    let mut url = url_with_path(base_url, path)?;
    url.query_pairs_mut()
        .extend_pairs(params.iter().map(|(k, v)| (*k, v.as_str())));
    Ok(url)
}

fn anime_to_observation(anime: Anime, payload: &RawSourcePayload) -> Result<SourceObservation> {
    let display_title = anime
        .title_english
        .clone()
        .filter(|t| !t.trim().is_empty())
        .unwrap_or_else(|| anime.title.clone());

    let mut aliases = Vec::new();
    push_alias(&mut aliases, &anime.title, None, "title", 1.0);
    if let Some(title) = &anime.title_english {
        push_alias(&mut aliases, title, Some("en"), "title_english", 1.0);
    }
    if let Some(title) = &anime.title_japanese {
        push_alias(&mut aliases, title, Some("ja"), "title_japanese", 0.95);
    }
    for title in &anime.titles {
        push_alias(
            &mut aliases,
            &title.title,
            None,
            title.title_type.as_deref().unwrap_or("title"),
            0.9,
        );
    }
    if let Some(anime_type) = &anime.anime_type {
        push_alias(&mut aliases, anime_type, None, "type", 0.35);
    }
    for genre in &anime.genres {
        push_alias(&mut aliases, &genre.name, None, "genre", 0.5);
    }
    for theme in &anime.themes {
        push_alias(&mut aliases, &theme.name, None, "theme", 0.45);
    }
    for demo in &anime.demographics {
        push_alias(&mut aliases, &demo.name, None, "demographic", 0.45);
    }
    for studio in &anime.studios {
        push_alias(&mut aliases, &studio.name, None, "studio", 0.65);
    }
    for producer in &anime.producers {
        push_alias(&mut aliases, &producer.name, None, "producer", 0.55);
    }

    let mut release_events = Vec::new();
    if let Some(aired) = &anime.aired {
        if let Some(from) = &aired.from {
            release_events.push(date_event(
                &format!("jikan:premiere:{}", anime.mal_id),
                "premiere",
                from,
                payload.fetched_at,
            )?);
        }
        if let Some(to) = &aired.to {
            release_events.push(date_event(
                &format!("jikan:finale:{}", anime.mal_id),
                "finale",
                to,
                payload.fetched_at,
            )?);
        }
    }

    let mut links = Vec::new();
    if let Some(url) = &anime.url {
        links.push(LinkObservation {
            site: "myanimelist".into(),
            url: url.clone(),
            link_kind: Some("source_page".into()),
        });
    }
    for external in &anime.external {
        links.push(LinkObservation {
            site: external.name.clone(),
            url: external.url.clone(),
            link_kind: Some("external".into()),
        });
    }
    for streaming in &anime.streaming {
        links.push(LinkObservation {
            site: streaming.name.clone(),
            url: streaming.url.clone(),
            link_kind: Some("streaming_link".into()),
        });
    }

    let mut images = Vec::new();
    if let Some(all_images) = &anime.images {
        if let Some(jpg) = &all_images.jpg {
            push_image(&mut images, "jpg_small", jpg.small_image_url.as_deref());
            push_image(&mut images, "jpg", jpg.image_url.as_deref());
            push_image(&mut images, "jpg_large", jpg.large_image_url.as_deref());
        }
        if let Some(webp) = &all_images.webp {
            push_image(&mut images, "webp_small", webp.small_image_url.as_deref());
            push_image(&mut images, "webp", webp.image_url.as_deref());
            push_image(&mut images, "webp_large", webp.large_image_url.as_deref());
        }
    }

    let mut description = anime.synopsis.clone();
    if description.is_none() {
        description = anime.aired.as_ref().and_then(|aired| aired.string.clone());
    }

    Ok(SourceObservation {
        source: "jikan".into(),
        source_id: anime.mal_id.to_string(),
        raw_payload_id: payload.id.clone(),
        kind: ReleaseKind::Anime,
        display_title,
        raw_title: Some(anime.title),
        description,
        status: anime.status,
        observed_at: payload.fetched_at,
        source_updated_at: None,
        aliases,
        external_ids: vec![ExternalIdObservation {
            id_kind: "mal_anime".into(),
            id_value: anime.mal_id.to_string(),
            confidence: 1.0,
        }],
        release_events,
        links,
        images,
    })
}

fn push_alias(
    aliases: &mut Vec<AliasObservation>,
    alias: &str,
    locale: Option<&str>,
    alias_kind: &str,
    confidence: f64,
) {
    if !alias.trim().is_empty() {
        aliases.push(AliasObservation {
            alias: alias.into(),
            locale: locale.map(str::to_string),
            alias_kind: Some(alias_kind.into()),
            confidence,
        });
    }
}

fn date_event(
    id: &str,
    kind: &str,
    raw_date: &str,
    observed_at: i64,
) -> Result<ReleaseEventObservation> {
    let date = raw_date.chars().take(10).collect::<String>();
    let scheduled_at = chrono::DateTime::parse_from_rfc3339(raw_date)
        .map(|dt| dt.timestamp())
        .or_else(|_| {
            chrono::NaiveDate::parse_from_str(&date, "%Y-%m-%d").map(|d| {
                d.and_hms_opt(0, 0, 0)
                    .expect("midnight is valid")
                    .and_utc()
                    .timestamp()
            })
        })
        .with_context(|| format!("parse Jikan aired date {raw_date:?}"))?;
    Ok(ReleaseEventObservation {
        id: id.into(),
        event_kind: kind.into(),
        title: None,
        season: None,
        episode: None,
        local_date: Some(date),
        local_time: None,
        source_timezone: Some("UTC".into()),
        scheduled_at: Some(scheduled_at),
        precision: TimePrecision::Date,
        confidence: 0.8,
        observed_at,
    })
}

fn push_image(images: &mut Vec<ImageObservation>, kind: &str, url: Option<&str>) {
    if let Some(url) = url {
        images.push(ImageObservation {
            image_kind: kind.into(),
            url: url.into(),
            width: None,
            height: None,
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ingest::HttpMethod;

    fn raw(body: &str) -> RawSourcePayload {
        RawSourcePayload {
            id: "raw:jikan:1".into(),
            source: "jikan".into(),
            endpoint: "anime_search".into(),
            method: HttpMethod::Get,
            request_key: "jikan:anime:cowboy".into(),
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
    fn parses_search_result_identity_titles_dates_images_and_links() {
        let body = r#"{
            "data": [{
                "mal_id": 1,
                "url": "https://myanimelist.net/anime/1/Cowboy_Bebop",
                "title": "Cowboy Bebop",
                "title_english": "Cowboy Bebop",
                "title_japanese": "カウボーイビバップ",
                "titles": [{"type": "Synonym", "title": "COWBOY BEBOP"}],
                "type": "TV",
                "episodes": 26,
                "status": "Finished Airing",
                "synopsis": "Crime is timeless.",
                "aired": {"from": "1998-04-03T00:00:00+00:00", "to": "1999-04-24T00:00:00+00:00", "string": "Apr 3, 1998 to Apr 24, 1999"},
                "images": {"jpg": {"image_url": "https://img.jpg", "large_image_url": "https://large.jpg"}},
                "genres": [{"name": "Action"}, {"name": "Sci-Fi"}],
                "studios": [{"name": "Sunrise"}],
                "producers": [{"name": "Bandai Visual"}],
                "external": [{"name": "Official Site", "url": "https://cowboy-bebop.net/"}],
                "streaming": [{"name": "Crunchyroll", "url": "https://crunchyroll.example/cowboy"}]
            }]
        }"#;

        let out = JikanParser.parse_search(&raw(body)).unwrap();
        assert_eq!(out.len(), 1);
        let anime = &out[0];
        assert_eq!(anime.kind, ReleaseKind::Anime);
        assert_eq!(anime.source_id, "1");
        assert_eq!(anime.display_title, "Cowboy Bebop");
        assert!(anime.aliases.iter().any(|a| a.alias == "Sunrise"));
        assert_eq!(anime.release_events.len(), 2);
        assert_eq!(anime.release_events[0].scheduled_at, Some(891_561_600));
        assert!(anime.links.iter().any(|l| l.site == "myanimelist"));
        assert!(anime.images.iter().any(|i| i.image_kind == "jpg_large"));
    }

    #[test]
    fn parses_fetch_response() {
        let body = r#"{"data":{"mal_id":5114,"title":"Fullmetal Alchemist: Brotherhood","status":"Finished Airing"}}"#;
        let obs = JikanParser.parse_fetch(&raw(body)).unwrap().unwrap();
        assert_eq!(obs.source_id, "5114");
        assert_eq!(obs.display_title, "Fullmetal Alchemist: Brotherhood");
    }
}
