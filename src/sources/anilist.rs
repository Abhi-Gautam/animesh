//! AniList HTTP client + Source-trait adapter.
//!
//! Exposes both the typed legacy methods (`search`, `by_id`,
//! `schedule_window`) that the existing TUI/commands call directly,
//! and the generic [`Source`] impl that the canonical service + sync
//! loop drive once they land. Rate-limit headers from each response
//! are parsed and held in-memory; later tasks will persist them via
//! the kv store for the `doctor` surface.

use std::sync::Mutex;

use anyhow::{anyhow, Context, Result};
use async_trait::async_trait;
use reqwest::Client;
use serde::{de::DeserializeOwned, Deserialize, Serialize};

use crate::ids::ReleaseKind;

use super::{Source, SourceRecord, StreamingLink};

const DEFAULT_BASE_URL: &str = "https://graphql.anilist.co";

/// Snapshot of the most recent AniList rate-limit headers.
#[derive(Debug, Clone, Copy, Default)]
pub struct RateLimit {
    /// `X-RateLimit-Remaining` — requests left in the current window.
    pub remaining: Option<i64>,
    /// `X-RateLimit-Reset` — unix timestamp at which the window resets.
    pub reset_at: Option<i64>,
}

pub struct AniListClient {
    client: Client,
    base_url: String,
    last_rate_limit: Mutex<RateLimit>,
}

impl AniListClient {
    pub fn new() -> Self {
        Self::with_base_url(DEFAULT_BASE_URL.into())
    }

    pub fn with_base_url(base_url: String) -> Self {
        Self {
            client: Client::new(),
            base_url,
            last_rate_limit: Mutex::new(RateLimit::default()),
        }
    }

    /// Snapshot of the most recent rate-limit headers we saw. Used by
    /// `doctor`.
    pub fn rate_limit(&self) -> RateLimit {
        *self.last_rate_limit.lock().expect("rate-limit lock poisoned")
    }

    /// Raw GraphQL escape hatch. Used by the legacy schedule code
    /// path until T24 migrates it to `schedule_window`. New call
    /// sites should prefer the typed methods below.
    pub async fn query<T: DeserializeOwned, V: Serialize>(
        &self,
        query: &str,
        variables: V,
    ) -> Result<T> {
        let resp = self
            .client
            .post(&self.base_url)
            .json(&serde_json::json!({ "query": query, "variables": variables }))
            .send()
            .await
            .context("POST to AniList")?;
        self.record_rate_limit(resp.headers());
        let status = resp.status();
        if !status.is_success() {
            let body = resp.text().await.unwrap_or_default();
            return Err(anyhow!("AniList HTTP {status}: {body}"));
        }
        resp.json::<T>().await.context("deserialize AniList response")
    }

    fn record_rate_limit(&self, headers: &reqwest::header::HeaderMap) {
        let get = |k: &str| -> Option<i64> {
            headers
                .get(k)
                .and_then(|v| v.to_str().ok())
                .and_then(|s| s.parse().ok())
        };
        let snap = RateLimit {
            remaining: get("x-ratelimit-remaining"),
            reset_at: get("x-ratelimit-reset"),
        };
        if snap.remaining.is_some() || snap.reset_at.is_some() {
            *self.last_rate_limit.lock().expect("rate-limit lock poisoned") = snap;
        }
    }

    /// Search for anime by free-form query. Ordered by AniList's
    /// SEARCH_MATCH relevance.
    pub async fn search(&self, query: &str, per_page: u32) -> Result<Vec<Media>> {
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
        let resp: GraphQlResponse<PageMedia> = self.query(body, vars).await?;
        Ok(resp.data.page.media)
    }

    /// Fetch a single anime by AniList numeric ID. Returns `None` if
    /// AniList responds with `data.Media: null`. Pulls the full
    /// TUI-detail-pane payload (cover, description, score, studios,
    /// streaming external links).
    pub async fn by_id(&self, id: i64) -> Result<Option<Media>> {
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
        let resp: GraphQlResponse<MediaData> = self.query(body, vars).await?;
        Ok(resp.data.media)
    }

    /// Fetch airing schedule entries in `[start, end)`. Caps at
    /// `per_page` per call; callers needing more results should
    /// paginate. v0.3 uses a single window; pagination lands when SP-3
    /// needs it.
    pub async fn schedule_window(
        &self,
        start: i64,
        end: i64,
        per_page: u32,
    ) -> Result<Vec<AiringSchedule>> {
        let body = r#"
            query ($start: Int, $end: Int, $perPage: Int) {
              Page(perPage: $perPage) {
                airingSchedules(airingAt_greater: $start, airingAt_lesser: $end) {
                  airingAt
                  episode
                  media {
                    id
                    title { romaji english native }
                    status
                    episodes
                    format
                    nextAiringEpisode { episode airingAt }
                  }
                }
              }
            }
        "#;
        let vars = serde_json::json!({
            "start": start, "end": end, "perPage": per_page,
        });
        let resp: GraphQlResponse<PageAiring> = self.query(body, vars).await?;
        Ok(resp.data.page.airing_schedules)
    }
}

impl Default for AniListClient {
    fn default() -> Self {
        Self::new()
    }
}

// ---------------------------------------------------------------------------
// Source trait impl. The projection from AniList's `Media` to the
// generic `SourceRecord` lives here — adapter-specific knowledge stays
// inside the adapter.
// ---------------------------------------------------------------------------

/// Kinds AniList handles. AniList only covers anime in this codebase;
/// extending to manga would be another entry here.
const ANILIST_KINDS: &[ReleaseKind] = &[ReleaseKind::Anime];

#[async_trait]
impl Source for AniListClient {
    fn name(&self) -> &'static str {
        "anilist"
    }

    fn kinds(&self) -> &[ReleaseKind] {
        ANILIST_KINDS
    }

    async fn search(&self, query: &str, limit: u32) -> Result<Vec<SourceRecord>> {
        let media = AniListClient::search(self, query, limit).await?;
        Ok(media.iter().map(media_to_record).collect())
    }

    async fn fetch(&self, source_id: &str) -> Result<Option<SourceRecord>> {
        let id: i64 = source_id
            .parse()
            .with_context(|| format!("anilist source_id must be numeric, got {source_id:?}"))?;
        let media = AniListClient::by_id(self, id).await?;
        Ok(media.as_ref().map(media_to_record))
    }
}

fn media_to_record(m: &Media) -> SourceRecord {
    let mut aliases = Vec::new();
    if let Some(rom) = m.title.romaji.as_deref() {
        if Some(rom) != m.title.english.as_deref() {
            aliases.push(rom.to_string());
        }
    }
    if let Some(native) = m.title.native.as_deref() {
        aliases.push(native.to_string());
    }

    let streaming_links: Vec<StreamingLink> = m
        .streaming_links()
        .iter()
        .filter_map(|l| {
            let url = l.url.clone()?;
            let site = l.site.clone().unwrap_or_else(|| "unknown".to_string());
            Some(StreamingLink { site, url })
        })
        .collect();

    SourceRecord {
        source: "anilist",
        source_id: m.id.to_string(),
        kind: ReleaseKind::Anime,
        display_title: m.display_title().to_string(),
        raw_title: m
            .title
            .romaji
            .as_deref()
            .or(m.title.english.as_deref())
            .or(m.title.native.as_deref())
            .unwrap_or("(untitled)")
            .to_string(),
        aliases,
        status: m.status.clone(),
        cover_url: m.cover_url().map(|s| s.to_string()),
        description: m.description.clone(),
        streaming_links,
        next_episode_at: m.next_airing_episode.map(|n| n.airing_at),
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
struct PageAiring {
    #[serde(rename = "Page")]
    page: AiringPage,
}

#[derive(Debug, Deserialize)]
struct AiringPage {
    #[serde(rename = "airingSchedules")]
    airing_schedules: Vec<AiringSchedule>,
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
    #[serde(default)]
    pub color: Option<String>,
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

#[derive(Debug, Clone, Deserialize)]
pub struct AiringSchedule {
    #[serde(rename = "airingAt")]
    pub airing_at: i64,
    pub episode: i64,
    pub media: Media,
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
    async fn search_parses_results_and_records_rate_limit() {
        let mut server = mockito::Server::new_async().await;
        let m = server
            .mock("POST", "/")
            .with_status(200)
            .with_header("content-type", "application/json")
            .with_header("X-RateLimit-Remaining", "85")
            .with_header("X-RateLimit-Reset", "1700001000")
            .with_body(body_for_search())
            .create_async()
            .await;
        let client = AniListClient::with_base_url(server.url());
        let results = client.search("piece", 50).await.unwrap();
        assert_eq!(results.len(), 2);
        assert_eq!(results[0].display_title(), "One Piece");
        assert_eq!(results[0].next_airing_episode.unwrap().episode, 1100);
        let rl = client.rate_limit();
        assert_eq!(rl.remaining, Some(85));
        assert_eq!(rl.reset_at, Some(1700001000));
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
    async fn schedule_window_parses_airing_entries() {
        let mut server = mockito::Server::new_async().await;
        let body = r#"{
            "data": {"Page": {"airingSchedules": [
                {"airingAt": 1700000000, "episode": 1100,
                 "media": {"id": 21, "title": {"romaji": "ONE PIECE", "english": "One Piece", "native": "ワンピース"},
                           "status": "RELEASING", "episodes": null, "format": "TV", "nextAiringEpisode": null}}
            ]}}
        }"#;
        let _m = server
            .mock("POST", "/")
            .with_status(200)
            .with_body(body)
            .create_async()
            .await;
        let client = AniListClient::with_base_url(server.url());
        let entries = client.schedule_window(0, i64::MAX, 50).await.unwrap();
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].episode, 1100);
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

    // ------------------------------------------------------------------
    // Source-trait projection tests. The legacy `search` / `by_id`
    // methods are tested above; these confirm that wrapping the
    // adapter as a `Box<dyn Source>` lands the expected SourceRecord
    // shape downstream.
    // ------------------------------------------------------------------

    fn sample_media() -> Media {
        Media {
            id: 21,
            title: MediaTitle {
                romaji: Some("ONE PIECE".into()),
                english: Some("One Piece".into()),
                native: Some("ワンピース".into()),
            },
            status: Some("RELEASING".into()),
            episodes: Some(1100),
            format: Some("TV".into()),
            next_airing_episode: Some(NextAiringEpisode {
                episode: 1101,
                airing_at: 1_700_000_000,
            }),
            cover_image: Some(MediaCoverImage {
                large: Some("https://img.example/large.jpg".into()),
                medium: None,
                color: Some("#ff0000".into()),
            }),
            description: Some("Pirate's life for me.".into()),
            average_score: Some(87),
            studios: None,
            external_links: Some(vec![
                MediaExternalLink {
                    site: Some("Crunchyroll".into()),
                    url: Some("https://crunchyroll.com/one-piece".into()),
                    color: None,
                    link_type: Some("STREAMING".into()),
                },
                MediaExternalLink {
                    site: Some("Twitter".into()),
                    url: Some("https://twitter.com/onepiece".into()),
                    color: None,
                    link_type: Some("SOCIAL".into()),
                },
            ]),
        }
    }

    #[test]
    fn media_to_record_projects_identity_fields() {
        let r = media_to_record(&sample_media());
        assert_eq!(r.source, "anilist");
        assert_eq!(r.source_id, "21");
        assert_eq!(r.kind, ReleaseKind::Anime);
        assert_eq!(r.display_title, "One Piece");
        // raw_title prefers romaji per the AniList convention.
        assert_eq!(r.raw_title, "ONE PIECE");
    }

    #[test]
    fn media_to_record_collects_aliases_excluding_display() {
        let r = media_to_record(&sample_media());
        // Romaji + native, English skipped because it's already the display.
        assert!(r.aliases.contains(&"ワンピース".to_string()));
        assert!(r.aliases.iter().any(|a| a.contains("ONE PIECE")));
    }

    #[test]
    fn media_to_record_filters_streaming_links_only() {
        let r = media_to_record(&sample_media());
        assert_eq!(r.streaming_links.len(), 1);
        assert_eq!(r.streaming_links[0].site, "Crunchyroll");
        assert_eq!(
            r.streaming_links[0].url,
            "https://crunchyroll.com/one-piece"
        );
    }

    #[test]
    fn media_to_record_carries_next_episode_at() {
        let r = media_to_record(&sample_media());
        assert_eq!(r.next_episode_at, Some(1_700_000_000));
    }

    #[tokio::test]
    async fn source_trait_search_returns_normalized_records() {
        let mut server = mockito::Server::new_async().await;
        let _m = server
            .mock("POST", "/")
            .with_status(200)
            .with_body(body_for_search())
            .create_async()
            .await;
        let client: Box<dyn Source> = Box::new(AniListClient::with_base_url(server.url()));
        let records = client.search("piece", 50).await.unwrap();
        assert_eq!(records.len(), 2);
        assert_eq!(records[0].source, "anilist");
        assert_eq!(records[0].source_id, "21");
        assert_eq!(records[0].kind, ReleaseKind::Anime);
    }

    #[tokio::test]
    async fn source_trait_fetch_rejects_non_numeric_source_id() {
        let server = mockito::Server::new_async().await;
        let client: Box<dyn Source> = Box::new(AniListClient::with_base_url(server.url()));
        let err = client.fetch("not-a-number").await.unwrap_err();
        assert!(format!("{err:#}").contains("numeric"));
    }

    #[test]
    fn source_trait_metadata_matches_adapter_name_and_kinds() {
        let client: Box<dyn Source> = Box::new(AniListClient::new());
        assert_eq!(client.name(), "anilist");
        assert_eq!(client.kinds(), &[ReleaseKind::Anime]);
    }
}
