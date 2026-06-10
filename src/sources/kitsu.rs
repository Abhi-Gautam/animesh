//! Kitsu JSON:API parser for anime source observations.

use anyhow::{anyhow, Context, Result};
use reqwest::{Client, Url};
use serde::Deserialize;
use std::collections::BTreeMap;

use crate::ids::ReleaseKind;
use crate::ingest::{
    AliasObservation, ExternalIdObservation, HttpMethod, ImageObservation, RawSourcePayload,
    ReleaseEventObservation, SourceObservation, SourceParser, TimePrecision,
};
use crate::search::SearchScope;
use crate::sources::{stable_hash, SourceAdapter, SourceFuture};

const DEFAULT_BASE_URL: &str = "https://kitsu.io/api/edge";
const ANIME_SEARCH_SCOPES: &[SearchScope] = &[SearchScope::Anime];
const NO_ENRICHMENT_SCOPES: &[SearchScope] = &[];

pub struct KitsuSource {
    client: Client,
    parser: KitsuParser,
    base_url: String,
}

impl KitsuSource {
    pub fn new() -> Self {
        Self::with_base_url(DEFAULT_BASE_URL)
    }

    #[allow(dead_code)]
    pub fn with_base_url(base_url: impl Into<String>) -> Self {
        Self {
            client: Client::new(),
            parser: KitsuParser,
            base_url: base_url.into(),
        }
    }
}

impl Default for KitsuSource {
    fn default() -> Self {
        Self::new()
    }
}

impl SourceAdapter for KitsuSource {
    fn source(&self) -> &'static str {
        "kitsu"
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
                &[
                    ("filter[text]", query.to_string()),
                    ("page[limit]", limit.to_string()),
                ],
            )?;
            let response_json = get_json(&self.client, url.as_str()).await?;
            Ok(vec![raw_payload(
                "anime_search",
                &format!("kitsu:anime_search:{query}:limit:{limit}"),
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
            let url = url_with_path(&self.base_url, &format!("anime/{source_id}"))?;
            let response_json = get_json(&self.client, url.as_str()).await?;
            Ok(Some(raw_payload(
                "anime",
                &format!("kitsu:anime:{source_id}"),
                response_json,
                now,
            )))
        })
    }
}

pub struct KitsuParser;

impl SourceParser for KitsuParser {
    fn source(&self) -> &'static str {
        "kitsu"
    }

    fn parse_search(&self, payload: &RawSourcePayload) -> Result<Vec<SourceObservation>> {
        let resp: SearchResponse =
            serde_json::from_str(&payload.response_json).context("parse Kitsu search response")?;
        resp.data
            .into_iter()
            .filter(|item| item.item_type == "anime")
            .map(|item| item_to_observation(item, payload))
            .collect()
    }

    fn parse_fetch(&self, payload: &RawSourcePayload) -> Result<Option<SourceObservation>> {
        let resp: FetchResponse =
            serde_json::from_str(&payload.response_json).context("parse Kitsu anime response")?;
        if resp.data.item_type != "anime" {
            return Ok(None);
        }
        Ok(Some(item_to_observation(resp.data, payload)?))
    }
}

#[derive(Debug, Deserialize)]
struct SearchResponse {
    #[serde(default)]
    data: Vec<KitsuItem>,
}

#[derive(Debug, Deserialize)]
struct FetchResponse {
    data: KitsuItem,
}

#[derive(Debug, Deserialize)]
struct KitsuItem {
    id: String,
    #[serde(rename = "type")]
    item_type: String,
    attributes: KitsuAttributes,
}

#[derive(Debug, Deserialize)]
struct KitsuAttributes {
    #[serde(rename = "canonicalTitle", default)]
    canonical_title: Option<String>,
    #[serde(default)]
    titles: BTreeMap<String, String>,
    #[serde(default)]
    synopsis: Option<String>,
    #[serde(default)]
    status: Option<String>,
    #[serde(rename = "subtype", default)]
    subtype: Option<String>,

    #[serde(rename = "startDate", default)]
    start_date: Option<String>,
    #[serde(rename = "endDate", default)]
    end_date: Option<String>,
    #[serde(rename = "posterImage", default)]
    poster_image: Option<KitsuImage>,
    #[serde(default)]
    slug: Option<String>,
    #[serde(rename = "youtubeVideoId", default)]
    youtube_video_id: Option<String>,
}

#[derive(Debug, Deserialize)]
struct KitsuImage {
    #[serde(default)]
    tiny: Option<String>,
    #[serde(default)]
    small: Option<String>,
    #[serde(default)]
    medium: Option<String>,
    #[serde(default)]
    large: Option<String>,
    #[serde(default)]
    original: Option<String>,
}

async fn get_json(client: &Client, url: &str) -> Result<String> {
    let resp = client
        .get(url)
        .send()
        .await
        .with_context(|| format!("GET {url}"))?;
    let status = resp.status();
    let body = resp.text().await.context("read Kitsu response body")?;
    if !status.is_success() {
        return Err(anyhow!("Kitsu HTTP {status}: {body}"));
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
        id: format!("raw:kitsu:{endpoint}:{request_hash}:{response_hash}"),
        source: "kitsu".into(),
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
        .context("parse Kitsu base URL")?
        .join(path)
        .with_context(|| format!("join Kitsu path {path:?}"))
}

fn url_with_params(base_url: &str, path: &str, params: &[(&str, String)]) -> Result<Url> {
    let mut url = url_with_path(base_url, path)?;
    url.query_pairs_mut()
        .extend_pairs(params.iter().map(|(k, v)| (*k, v.as_str())));
    Ok(url)
}

fn item_to_observation(item: KitsuItem, payload: &RawSourcePayload) -> Result<SourceObservation> {
    let display_title = item
        .attributes
        .canonical_title
        .clone()
        .or_else(|| item.attributes.titles.values().next().cloned())
        .unwrap_or_else(|| format!("kitsu:{}", item.id));

    let mut aliases = Vec::new();
    aliases.push(AliasObservation {
        alias: display_title.clone(),
        locale: None,
        alias_kind: Some("canonical".into()),
        confidence: 1.0,
    });
    for (locale, title) in &item.attributes.titles {
        if !title.trim().is_empty() {
            aliases.push(AliasObservation {
                alias: title.clone(),
                locale: Some(locale.clone()),
                alias_kind: Some("title".into()),
                confidence: 0.95,
            });
        }
    }
    if let Some(subtype) = &item.attributes.subtype {
        aliases.push(AliasObservation {
            alias: subtype.clone(),
            locale: None,
            alias_kind: Some("subtype".into()),
            confidence: 0.35,
        });
    }

    let mut release_events = Vec::new();
    if let Some(date) = &item.attributes.start_date {
        release_events.push(date_event(
            &format!("kitsu:premiere:{}", item.id),
            "premiere",
            date,
            payload.fetched_at,
        )?);
    }
    if let Some(date) = &item.attributes.end_date {
        release_events.push(date_event(
            &format!("kitsu:finale:{}", item.id),
            "finale",
            date,
            payload.fetched_at,
        )?);
    }

    let mut images = Vec::new();
    if let Some(poster) = &item.attributes.poster_image {
        push_image(&mut images, "poster_tiny", poster.tiny.as_deref());
        push_image(&mut images, "poster_small", poster.small.as_deref());
        push_image(&mut images, "poster_medium", poster.medium.as_deref());
        push_image(&mut images, "poster_large", poster.large.as_deref());
        push_image(&mut images, "poster_original", poster.original.as_deref());
    }

    let mut external_ids = vec![ExternalIdObservation {
        id_kind: "kitsu".into(),
        id_value: item.id.clone(),
        confidence: 1.0,
    }];
    if let Some(slug) = &item.attributes.slug {
        external_ids.push(ExternalIdObservation {
            id_kind: "kitsu_slug".into(),
            id_value: slug.clone(),
            confidence: 0.85,
        });
    }

    let mut links = Vec::new();
    if let Some(slug) = &item.attributes.slug {
        links.push(crate::ingest::LinkObservation {
            site: "kitsu".into(),
            url: format!("https://kitsu.app/anime/{slug}"),
            link_kind: Some("source_page".into()),
        });
    }
    if let Some(video_id) = &item.attributes.youtube_video_id {
        links.push(crate::ingest::LinkObservation {
            site: "youtube".into(),
            url: format!("https://www.youtube.com/watch?v={video_id}"),
            link_kind: Some("trailer".into()),
        });
    }

    Ok(SourceObservation {
        source: "kitsu".into(),
        source_id: item.id,
        raw_payload_id: payload.id.clone(),
        kind: ReleaseKind::Anime,
        display_title,
        raw_title: item.attributes.canonical_title,
        description: item.attributes.synopsis,
        status: item.attributes.status,
        observed_at: payload.fetched_at,
        source_updated_at: None,
        aliases,
        external_ids,
        release_events,
        links,
        images,
    })
}

fn date_event(
    id: &str,
    kind: &str,
    date: &str,
    observed_at: i64,
) -> Result<ReleaseEventObservation> {
    let scheduled_at = chrono::NaiveDate::parse_from_str(date, "%Y-%m-%d")
        .map(|d| {
            d.and_hms_opt(0, 0, 0)
                .expect("midnight is valid")
                .and_utc()
                .timestamp()
        })
        .with_context(|| format!("parse Kitsu date {date:?}"))?;
    Ok(ReleaseEventObservation {
        id: id.into(),
        event_kind: kind.into(),
        title: None,
        season: None,
        episode: None,
        local_date: Some(date.into()),
        local_time: None,
        source_timezone: None,
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
            id: "raw:kitsu:1".into(),
            source: "kitsu".into(),
            endpoint: "anime_search".into(),
            method: HttpMethod::Get,
            request_key: "kitsu:anime:cowboy".into(),
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
    fn parses_search_response_titles_dates_images_and_links() {
        let body = r#"{
            "data": [{
                "id": "1",
                "type": "anime",
                "attributes": {
                    "canonicalTitle": "Cowboy Bebop",
                    "titles": {"en": "Cowboy Bebop", "ja_jp": "カウボーイビバップ"},
                    "synopsis": "In the year 2071...",
                    "status": "finished",
                    "subtype": "TV",
                    "episodeCount": 26,
                    "startDate": "1998-04-03",
                    "endDate": "1999-04-24",
                    "posterImage": {"medium": "https://img/medium.jpg", "original": "https://img/original.jpg"},
                    "slug": "cowboy-bebop",
                    "youtubeVideoId": "abc123"
                }
            }]
        }"#;

        let out = KitsuParser.parse_search(&raw(body)).unwrap();
        assert_eq!(out.len(), 1);
        let anime = &out[0];
        assert_eq!(anime.kind, ReleaseKind::Anime);
        assert_eq!(anime.display_title, "Cowboy Bebop");
        assert!(anime
            .aliases
            .iter()
            .any(|a| a.alias == "カウボーイビバップ"));
        assert_eq!(anime.release_events.len(), 2);
        assert_eq!(anime.release_events[0].scheduled_at, Some(891_561_600));
        assert_eq!(anime.images.len(), 2);
        assert!(anime.links.iter().any(|l| l.site == "youtube"));
    }

    #[test]
    fn ignores_non_anime_fetch_payloads() {
        let body =
            r#"{"data":{"id":"1","type":"manga","attributes":{"canonicalTitle":"Berserk"}}}"#;
        let obs = KitsuParser.parse_fetch(&raw(body)).unwrap();
        assert!(obs.is_none());
    }
}
