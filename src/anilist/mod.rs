//! AniList HTTP client.
//!
//! The only module in the crate that imports `reqwest`. Exposes both
//! a raw `query<T, V>` escape hatch (used by the legacy schedule path
//! until T24 rewires it) and typed `search` / `by_id` /
//! `schedule_window` methods. Rate-limit headers from each response
//! are parsed and held in-memory; later tasks will persist them via
//! the kv store for the `doctor` surface.

use std::sync::Mutex;

use anyhow::{anyhow, Context, Result};
use reqwest::Client;
use serde::{de::DeserializeOwned, Deserialize, Serialize};

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
}
