//! AniList source adapter.
//!
//! Source-specific HTTP and raw payload construction live here behind the
//! generic source port. The rest of the app sees plugged source adapters.

use anyhow::{anyhow, Context, Result};
use reqwest::Client;
use serde::{Deserialize, Serialize};

use crate::ids::ReleaseKind;
use crate::ingest::{
    AliasObservation, ExternalIdObservation, HttpMethod, ImageObservation, LinkObservation,
    RawSourcePayload, ReleaseEventObservation, SourceObservation, SourceParser, TimePrecision,
};
use crate::sources::{stable_hash, SourceAdapter, SourceFuture};

const DEFAULT_BASE_URL: &str = "https://graphql.anilist.co";

pub struct AniListClient {
    client: Client,
    base_url: String,
}

impl AniListClient {
    pub fn new() -> Self {
        Self::with_base_url(DEFAULT_BASE_URL.into())
    }

    pub fn with_base_url(base_url: String) -> Self {
        Self {
            client: Client::new(),
            base_url,
        }
    }

    /// Execute a GraphQL query and return both serialized request and raw
    /// response JSON. The ingestion pipeline uses this so raw payload storage
    /// remains the first durable step.
    pub async fn raw_query<V: Serialize>(
        &self,
        query: &str,
        variables: V,
    ) -> Result<(String, String)> {
        let request = serde_json::json!({ "query": query, "variables": variables });
        let request_json = serde_json::to_string(&request).context("serialize AniList request")?;
        let resp = self
            .client
            .post(&self.base_url)
            .json(&request)
            .send()
            .await
            .context("POST to AniList")?;
        let status = resp.status();
        let response_json = resp.text().await.context("read AniList response body")?;
        if !status.is_success() {
            return Err(anyhow!("AniList HTTP {status}: {response_json}"));
        }
        Ok((request_json, response_json))
    }

    /// Search for anime by free-form query. Ordered by AniList's
    /// SEARCH_MATCH relevance.
    #[allow(dead_code)]
    pub async fn search(&self, query: &str, per_page: u32) -> Result<Vec<Media>> {
        let resp: GraphQlResponse<PageMedia> =
            serde_json::from_str(&self.search_raw_json(query, per_page).await?.1)
                .context("deserialize AniList search response")?;
        Ok(resp.data.page.media)
    }

    /// Raw search used by ingestion. Returns `(request_json, response_json)`.
    pub async fn search_raw_json(&self, query: &str, per_page: u32) -> Result<(String, String)> {
        let body = r#"
            query ($search: String, $perPage: Int) {
              Page(perPage: $perPage) {
                media(search: $search, type: ANIME, sort: SEARCH_MATCH) {
                  id
                  title { romaji english native }
                  status
                  episodes
                  format
                  nextAiringEpisode { episode airingAt }
                }
              }
            }
        "#;
        let vars = serde_json::json!({ "search": query, "perPage": per_page });
        self.raw_query(body, vars).await
    }

    /// Fetch a single anime by AniList numeric ID. Returns `None` if
    /// AniList responds with `data.Media: null`. Pulls the full
    /// TUI-detail-pane payload (cover, description, score, studios,
    /// streaming external links).
    pub async fn by_id(&self, id: i64) -> Result<Option<Media>> {
        let resp: GraphQlResponse<MediaData> =
            serde_json::from_str(&self.by_id_raw_json(id).await?.1)
                .context("deserialize AniList by_id response")?;
        Ok(resp.data.media)
    }

    /// Raw fetch used by ingestion. Returns `(request_json, response_json)`.
    pub async fn by_id_raw_json(&self, id: i64) -> Result<(String, String)> {
        let body = r#"
            query ($id: Int) {
              Media(id: $id, type: ANIME) {
                id
                title { romaji english native }
                status
                episodes
                format
                nextAiringEpisode { episode airingAt }
                coverImage { large medium color }
                description(asHtml: false)
                averageScore
                studios(isMain: true) { nodes { name isAnimationStudio } }
                externalLinks { site url color type }
              }
            }
        "#;
        let vars = serde_json::json!({ "id": id });
        self.raw_query(body, vars).await
    }
}

impl Default for AniListClient {
    fn default() -> Self {
        Self::new()
    }
}

pub struct AniListSource {
    client: AniListClient,
    parser: AniListParser,
}

impl AniListSource {
    pub fn new() -> Self {
        Self {
            client: AniListClient::new(),
            parser: AniListParser,
        }
    }

    #[allow(dead_code)]
    pub fn with_client(client: AniListClient) -> Self {
        Self {
            client,
            parser: AniListParser,
        }
    }
}

impl Default for AniListSource {
    fn default() -> Self {
        Self::new()
    }
}

impl SourceAdapter for AniListSource {
    fn source(&self) -> &'static str {
        "anilist"
    }

    fn parser(&self) -> &dyn SourceParser {
        &self.parser
    }

    fn search<'a>(
        &'a self,
        query: &'a str,
        limit: u32,
        now: i64,
    ) -> SourceFuture<'a, Vec<RawSourcePayload>> {
        Box::pin(async move {
            let (request_json, response_json) = self.client.search_raw_json(query, limit).await?;
            Ok(vec![raw_payload(
                "search_media",
                &format!("anilist:search:{query}:limit:{limit}"),
                Some(request_json),
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
            let id = source_id
                .parse::<i64>()
                .with_context(|| format!("AniList source_id must be numeric, got {source_id:?}"))?;
            let (request_json, response_json) = self.client.by_id_raw_json(id).await?;
            Ok(Some(raw_payload(
                "media",
                &format!("anilist:media:{source_id}"),
                Some(request_json),
                response_json,
                now,
            )))
        })
    }
}

fn raw_payload(
    endpoint: &str,
    request_key: &str,
    request_json: Option<String>,
    response_json: String,
    now: i64,
) -> RawSourcePayload {
    let request_hash = stable_hash(request_json.as_deref().unwrap_or(request_key));
    let response_hash = stable_hash(&response_json);
    RawSourcePayload {
        id: format!("raw:anilist:{endpoint}:{request_hash}:{response_hash}"),
        source: "anilist".into(),
        endpoint: endpoint.into(),
        method: HttpMethod::Post,
        request_key: request_key.into(),
        request_hash,
        request_json,
        http_status: 200,
        response_hash,
        response_json,
        fetched_at: now,
        expires_at: None,
        created_at: now,
    }
}

// ---------------------------------------------------------------------------
// AniList response shapes. Kept minimal — we deserialize only what the
// CLI actually uses.
// ---------------------------------------------------------------------------

#[derive(Debug, Deserialize)]
struct GraphQlResponse<T> {
    data: T,
}

#[derive(Debug, Deserialize)]
struct PageMedia {
    #[serde(rename = "Page")]
    page: MediaPage,
}

#[derive(Debug, Deserialize)]
struct MediaPage {
    media: Vec<Media>,
}

#[derive(Debug, Deserialize)]
struct MediaData {
    #[serde(rename = "Media")]
    media: Option<Media>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct Media {
    pub id: i64,
    pub title: MediaTitle,
    pub status: Option<String>,
    pub episodes: Option<i64>,
    pub format: Option<String>,
    #[serde(rename = "nextAiringEpisode")]
    pub next_airing_episode: Option<NextAiringEpisode>,
    // Extended fields fetched for the TUI's detail pane.
    #[serde(rename = "coverImage", default)]
    pub cover_image: Option<MediaCoverImage>,
    #[serde(default)]
    pub description: Option<String>,
    #[serde(rename = "averageScore", default)]
    pub average_score: Option<i64>,
    #[serde(default)]
    pub studios: Option<MediaStudios>,
    #[serde(rename = "externalLinks", default)]
    pub external_links: Option<Vec<MediaExternalLink>>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct MediaCoverImage {
    #[serde(default)]
    pub large: Option<String>,
    #[serde(default)]
    pub medium: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct MediaStudios {
    #[serde(default)]
    pub nodes: Vec<MediaStudioNode>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct MediaStudioNode {
    pub name: String,
    #[serde(rename = "isAnimationStudio", default)]
    pub is_animation_studio: bool,
}

#[derive(Debug, Clone, Deserialize, serde::Serialize)]
pub struct MediaExternalLink {
    #[serde(default)]
    pub site: Option<String>,
    #[serde(default)]
    pub url: Option<String>,
    #[serde(default)]
    pub color: Option<String>,
    #[serde(rename = "type", default)]
    pub link_type: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct MediaTitle {
    pub romaji: Option<String>,
    pub english: Option<String>,
    pub native: Option<String>,
}

#[derive(Debug, Clone, Copy, Deserialize)]
pub struct NextAiringEpisode {
    pub episode: i64,
    #[serde(rename = "airingAt")]
    pub airing_at: i64,
}

pub struct AniListParser;

impl SourceParser for AniListParser {
    fn source(&self) -> &'static str {
        "anilist"
    }

    fn parse_search(&self, payload: &RawSourcePayload) -> Result<Vec<SourceObservation>> {
        let resp: GraphQlResponse<PageMedia> = serde_json::from_str(&payload.response_json)
            .context("parse AniList search response")?;
        resp.data
            .page
            .media
            .into_iter()
            .map(|media| media_to_observation(media, payload))
            .collect()
    }

    fn parse_fetch(&self, payload: &RawSourcePayload) -> Result<Option<SourceObservation>> {
        let resp: GraphQlResponse<MediaData> =
            serde_json::from_str(&payload.response_json).context("parse AniList media response")?;
        resp.data
            .media
            .map(|media| media_to_observation(media, payload))
            .transpose()
    }
}

fn media_to_observation(media: Media, payload: &RawSourcePayload) -> Result<SourceObservation> {
    let mut aliases = Vec::new();
    push_title_alias(
        &mut aliases,
        media.title.english.as_deref(),
        Some("en"),
        "title_english",
        1.0,
    );
    push_title_alias(
        &mut aliases,
        media.title.romaji.as_deref(),
        None,
        "title_romaji",
        0.95,
    );
    push_title_alias(
        &mut aliases,
        media.title.native.as_deref(),
        Some("ja"),
        "title_native",
        0.95,
    );
    if let Some(format) = &media.format {
        aliases.push(AliasObservation {
            alias: format.clone(),
            locale: None,
            alias_kind: Some("format".into()),
            confidence: 0.4,
        });
    }
    if let Some(studios) = &media.studios {
        for studio in &studios.nodes {
            aliases.push(AliasObservation {
                alias: studio.name.clone(),
                locale: None,
                alias_kind: Some(
                    if studio.is_animation_studio {
                        "animation_studio"
                    } else {
                        "studio"
                    }
                    .into(),
                ),
                confidence: 0.65,
            });
        }
    }

    let mut release_events = Vec::new();
    if let Some(next) = media.next_airing_episode {
        release_events.push(ReleaseEventObservation {
            id: format!("anilist:airing:{}:{}", media.id, next.episode),
            event_kind: "episode".into(),
            title: None,
            season: None,
            episode: Some(next.episode),
            local_date: None,
            local_time: None,
            source_timezone: Some("UTC".into()),
            scheduled_at: Some(next.airing_at),
            precision: TimePrecision::Instant,
            confidence: 0.95,
            observed_at: payload.fetched_at,
        });
    }

    let mut links = Vec::new();
    if let Some(external_links) = &media.external_links {
        for link in external_links {
            if let Some(url) = &link.url {
                links.push(LinkObservation {
                    site: link.site.clone().unwrap_or_else(|| "external".into()),
                    url: url.clone(),
                    link_kind: link.link_type.as_ref().map(|t| t.to_ascii_lowercase()),
                });
            }
        }
    }

    let mut images = Vec::new();
    if let Some(cover) = &media.cover_image {
        if let Some(url) = &cover.medium {
            images.push(image_obs("cover_medium", url));
        }
        if let Some(url) = &cover.large {
            images.push(image_obs("cover_large", url));
        }
    }

    Ok(SourceObservation {
        source: "anilist".into(),
        source_id: media.id.to_string(),
        raw_payload_id: payload.id.clone(),
        kind: ReleaseKind::Anime,
        display_title: media.display_title().to_string(),
        raw_title: media.title.romaji.clone(),
        description: media.description.clone(),
        status: media.status.clone(),
        observed_at: payload.fetched_at,
        source_updated_at: None,
        aliases,
        external_ids: vec![ExternalIdObservation {
            id_kind: "anilist".into(),
            id_value: media.id.to_string(),
            confidence: 1.0,
        }],
        release_events,
        links,
        images,
    })
}

fn push_title_alias(
    aliases: &mut Vec<AliasObservation>,
    title: Option<&str>,
    locale: Option<&str>,
    alias_kind: &str,
    confidence: f64,
) {
    if let Some(title) = title.filter(|t| !t.trim().is_empty()) {
        aliases.push(AliasObservation {
            alias: title.to_string(),
            locale: locale.map(str::to_string),
            alias_kind: Some(alias_kind.into()),
            confidence,
        });
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

impl Media {
    /// Pick a sensible display title: English if present, else romaji,
    /// else native, else a placeholder.
    pub fn display_title(&self) -> &str {
        self.title
            .english
            .as_deref()
            .or(self.title.romaji.as_deref())
            .or(self.title.native.as_deref())
            .unwrap_or("(untitled)")
    }

    /// Pick the highest-resolution cover image URL we have.
    pub fn cover_url(&self) -> Option<&str> {
        self.cover_image
            .as_ref()
            .and_then(|c| c.large.as_deref().or(c.medium.as_deref()))
    }

    /// Comma-joined studios. Animation studios first.
    pub fn studios_joined(&self) -> Option<String> {
        let s = self.studios.as_ref()?;
        if s.nodes.is_empty() {
            return None;
        }
        let mut animation: Vec<&str> = s
            .nodes
            .iter()
            .filter(|n| n.is_animation_studio)
            .map(|n| n.name.as_str())
            .collect();
        let other: Vec<&str> = s
            .nodes
            .iter()
            .filter(|n| !n.is_animation_studio)
            .map(|n| n.name.as_str())
            .collect();
        animation.extend(other);
        Some(animation.join(", "))
    }

    /// External links that look like streaming services.
    pub fn streaming_links(&self) -> Vec<&MediaExternalLink> {
        match &self.external_links {
            None => Vec::new(),
            Some(links) => links
                .iter()
                .filter(|l| {
                    l.link_type
                        .as_deref()
                        .map(|t| t.eq_ignore_ascii_case("STREAMING"))
                        .unwrap_or(false)
                })
                .collect(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn body_for_search() -> &'static str {
        r#"{
            "data": {
                "Page": {
                    "media": [
                        {"id": 21, "title": {"romaji": "ONE PIECE", "english": "One Piece", "native": "ワンピース"},
                         "status": "RELEASING", "episodes": null, "format": "TV", "nextAiringEpisode": {"episode": 1100, "airingAt": 1700000000}},
                        {"id": 11061, "title": {"romaji": "Hunter x Hunter", "english": "Hunter x Hunter (2011)", "native": "ハンター×ハンター"},
                         "status": "FINISHED", "episodes": 148, "format": "TV", "nextAiringEpisode": null}
                    ]
                }
            }
        }"#
    }

    #[tokio::test]
    async fn search_parses_results() {
        let mut server = mockito::Server::new_async().await;
        let m = server
            .mock("POST", "/")
            .with_status(200)
            .with_header("content-type", "application/json")
            .with_body(body_for_search())
            .create_async()
            .await;
        let client = AniListClient::with_base_url(server.url());
        let results = client.search("piece", 50).await.unwrap();
        assert_eq!(results.len(), 2);
        assert_eq!(results[0].display_title(), "One Piece");
        assert_eq!(results[0].next_airing_episode.unwrap().episode, 1100);
        m.assert_async().await;
    }

    #[tokio::test]
    async fn by_id_returns_some_for_existing_show() {
        let mut server = mockito::Server::new_async().await;
        let body = r#"{
            "data": { "Media": {
                "id": 21,
                "title": {"romaji": "ONE PIECE", "english": "One Piece", "native": "ワンピース"},
                "status": "RELEASING", "episodes": null, "format": "TV",
                "nextAiringEpisode": null
            }}
        }"#;
        let _m = server
            .mock("POST", "/")
            .with_status(200)
            .with_body(body)
            .create_async()
            .await;
        let client = AniListClient::with_base_url(server.url());
        let media = client.by_id(21).await.unwrap();
        assert!(media.is_some());
        assert_eq!(media.unwrap().display_title(), "One Piece");
    }

    #[tokio::test]
    async fn by_id_returns_none_for_missing_show() {
        let mut server = mockito::Server::new_async().await;
        let body = r#"{"data": {"Media": null}}"#;
        let _m = server
            .mock("POST", "/")
            .with_status(200)
            .with_body(body)
            .create_async()
            .await;
        let client = AniListClient::with_base_url(server.url());
        assert!(client.by_id(999_999_999).await.unwrap().is_none());
    }

    #[tokio::test]
    async fn non_2xx_status_yields_error_with_body() {
        let mut server = mockito::Server::new_async().await;
        let _m = server
            .mock("POST", "/")
            .with_status(429)
            .with_body("rate limited")
            .create_async()
            .await;
        let client = AniListClient::with_base_url(server.url());
        let err = client.search("x", 10).await.expect_err("expected error");
        let msg = format!("{err:#}");
        assert!(msg.contains("429"), "msg: {msg}");
        assert!(msg.contains("rate limited"), "msg: {msg}");
    }

    #[test]
    fn parser_maps_search_payload_to_observations() {
        use crate::ingest::{HttpMethod, RawSourcePayload};

        let raw = RawSourcePayload {
            id: "raw:anilist:1".into(),
            source: "anilist".into(),
            endpoint: "search_media".into(),
            method: HttpMethod::Post,
            request_key: "anilist:search:piece".into(),
            request_hash: "req".into(),
            request_json: None,
            http_status: 200,
            response_hash: "resp".into(),
            response_json: body_for_search().into(),
            fetched_at: 1_000,
            expires_at: None,
            created_at: 1_000,
        };

        let out = AniListParser.parse_search(&raw).unwrap();
        assert_eq!(out.len(), 2);
        assert_eq!(out[0].source_id, "21");
        assert_eq!(out[0].kind, ReleaseKind::Anime);
        assert_eq!(out[0].display_title, "One Piece");
        assert!(out[0]
            .aliases
            .iter()
            .any(|a| a.alias == "ワンピース" && a.alias_kind.as_deref() == Some("title_native")));
        assert_eq!(out[0].release_events[0].scheduled_at, Some(1_700_000_000));
    }

    #[test]
    fn parser_maps_fetch_payload_links_images_and_studios() {
        use crate::ingest::{HttpMethod, RawSourcePayload};

        let body = r#"{
            "data": { "Media": {
                "id": 21,
                "title": {"romaji": "ONE PIECE", "english": "One Piece", "native": "ワンピース"},
                "status": "RELEASING",
                "episodes": null,
                "format": "TV",
                "nextAiringEpisode": null,
                "coverImage": {"large": "https://img/large.jpg", "medium": "https://img/medium.jpg"},
                "description": "Pirates",
                "averageScore": 86,
                "studios": {"nodes": [{"name": "Toei Animation", "isAnimationStudio": true}]},
                "externalLinks": [{"site": "Official Site", "url": "https://one-piece.com", "type": "INFO"}]
            }}
        }"#;
        let raw = RawSourcePayload {
            id: "raw:anilist:2".into(),
            source: "anilist".into(),
            endpoint: "media".into(),
            method: HttpMethod::Post,
            request_key: "anilist:media:21".into(),
            request_hash: "req".into(),
            request_json: None,
            http_status: 200,
            response_hash: "resp".into(),
            response_json: body.into(),
            fetched_at: 1_000,
            expires_at: None,
            created_at: 1_000,
        };

        let obs = AniListParser.parse_fetch(&raw).unwrap().unwrap();
        assert_eq!(obs.description.as_deref(), Some("Pirates"));
        assert!(obs.aliases.iter().any(|a| a.alias == "Toei Animation"));
        assert!(obs.links.iter().any(|l| l.site == "Official Site"));
        assert_eq!(obs.images.len(), 2);
    }

    #[test]
    fn display_title_prefers_english_then_romaji_then_native() {
        let m = Media {
            id: 1,
            title: MediaTitle {
                romaji: Some("a".into()),
                english: Some("b".into()),
                native: Some("c".into()),
            },
            status: None,
            episodes: None,
            format: None,
            next_airing_episode: None,
            cover_image: None,
            description: None,
            average_score: None,
            studios: None,
            external_links: None,
        };
        assert_eq!(m.display_title(), "b");
        let m = Media {
            title: MediaTitle {
                romaji: Some("a".into()),
                english: None,
                native: Some("c".into()),
            },
            ..m
        };
        assert_eq!(m.display_title(), "a");
        let m = Media {
            title: MediaTitle {
                romaji: None,
                english: None,
                native: Some("c".into()),
            },
            ..m
        };
        assert_eq!(m.display_title(), "c");
    }
}
