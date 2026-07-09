//! AniList GraphQL client — search + media-by-id.

use std::time::Duration;

use anyhow::{anyhow, Context, Result};
use reqwest::Client;
use serde::Deserialize;

const DEFAULT_BASE_URL: &str = "https://graphql.anilist.co";
const REQUEST_TIMEOUT: Duration = Duration::from_secs(10);

pub(crate) struct AniListClient {
    client: Client,
    base_url: String,
}

impl AniListClient {
    pub(crate) fn new() -> Self {
        Self::with_base_url(DEFAULT_BASE_URL.into())
    }

    pub(crate) fn with_base_url(base_url: String) -> Self {
        let client = Client::builder()
            .timeout(REQUEST_TIMEOUT)
            .user_agent(concat!("animesh/", env!("CARGO_PKG_VERSION")))
            .build()
            .expect("build reqwest client");
        Self { client, base_url }
    }

    async fn post_graphql<T>(&self, query: &str, variables: serde_json::Value) -> Result<T>
    where
        T: for<'de> Deserialize<'de>,
    {
        let request = serde_json::json!({ "query": query, "variables": variables });
        let resp = self
            .client
            .post(&self.base_url)
            .json(&request)
            .send()
            .await
            .context("POST to AniList")?;
        let status = resp.status();
        let response_json = resp.text().await.context("read AniList response body")?;

        // Body may not even be a GraphQL envelope (e.g. a plain-text rate-limit
        // page) — fall through to the raw status+body error in that case.
        if let Ok(envelope) = serde_json::from_str::<GraphQlResponse<T>>(&response_json) {
            if let Some(data) = envelope.data {
                return Ok(data);
            }
            if let Some(err) = envelope.errors.first() {
                return Err(anyhow!("AniList error: {}", err.message));
            }
        }

        Err(anyhow!("AniList HTTP {status}: {response_json}"))
    }

    /// Search anime by free-form query.
    pub(crate) async fn search(&self, query: &str, per_page: u32) -> Result<Vec<Media>> {
        let body = r#"
            query ($search: String, $perPage: Int) {
              Page(perPage: $perPage) {
                media(search: $search, type: ANIME, sort: SEARCH_MATCH) {
                  id
                  title { romaji english native }
                  status
                  episodes
                  format
                  seasonYear
                }
              }
            }
        "#;
        let variables = serde_json::json!({ "search": query, "perPage": per_page });
        let resp: PageMedia = self.post_graphql(body, variables).await?;
        Ok(resp.page.media)
    }

    /// Fetch a single anime by AniList media id.
    ///
    /// Returns `Ok(None)` when AniList has no anime with that id.
    pub(crate) async fn media(&self, id: i64) -> Result<Option<Media>> {
        let body = r#"
            query ($id: Int) {
              Media(id: $id, type: ANIME) {
                id
                title { romaji english native }
                status
                episodes
                format
                seasonYear
                nextAiringEpisode {
                  episode
                  airingAt
                  timeUntilAiring
                }
              }
            }
        "#;
        let variables = serde_json::json!({ "id": id });
        let resp: MediaData = self.post_graphql(body, variables).await?;
        Ok(resp.media)
    }
}

impl Default for AniListClient {
    fn default() -> Self {
        Self::new()
    }
}

// ---------------------------------------------------------------------------
// Response shapes
// ---------------------------------------------------------------------------

#[derive(Debug, Deserialize)]
struct GraphQlResponse<T> {
    data: Option<T>,
    #[serde(default)]
    errors: Vec<GraphQlError>,
}

#[derive(Debug, Deserialize)]
struct GraphQlError {
    message: String,
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
pub(crate) struct Media {
    pub id: i64,
    pub title: MediaTitle,
    pub status: Option<String>,
    pub format: Option<String>,
    pub episodes: Option<i64>,
    #[serde(rename = "seasonYear")]
    pub season_year: Option<i64>,
    #[serde(rename = "nextAiringEpisode", default)]
    pub next_airing_episode: Option<NextAiringEpisode>,
}

#[derive(Debug, Clone, Deserialize)]
pub(crate) struct MediaTitle {
    pub romaji: Option<String>,
    pub english: Option<String>,
    pub native: Option<String>,
}

#[derive(Debug, Clone, Copy, Deserialize)]
pub(crate) struct NextAiringEpisode {
    pub episode: i64,
    #[serde(rename = "airingAt")]
    pub airing_at: i64,
    #[serde(rename = "timeUntilAiring")]
    pub time_until_airing: Option<i64>,
}

impl Media {
    /// Pick the best display title: English > romaji > native > placeholder.
    pub(crate) fn display_title(&self) -> &str {
        self.title
            .english
            .as_deref()
            .or(self.title.romaji.as_deref())
            .or(self.title.native.as_deref())
            .unwrap_or("(untitled)")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn search_parses_results() {
        let mut server = mockito::Server::new_async().await;
        let body = r#"{
            "data": {
                "Page": {
                    "media": [
                        {"id": 21, "title": {"romaji": "ONE PIECE", "english": "One Piece", "native": "ワンピース"},
                         "status": "RELEASING", "episodes": null, "format": "TV", "seasonYear": 1999},
                        {"id": 11061, "title": {"romaji": "Hunter x Hunter", "english": "Hunter x Hunter (2011)", "native": "ハンター×ハンター"},
                         "status": "FINISHED", "episodes": 148, "format": "TV", "seasonYear": 2011}
                    ]
                }
            }
        }"#;
        let _m = server
            .mock("POST", "/")
            .with_status(200)
            .with_body(body)
            .create_async()
            .await;

        let client = AniListClient::with_base_url(server.url());
        let results = client.search("one piece", 10).await.unwrap();

        assert_eq!(results.len(), 2);
        assert_eq!(results[0].id, 21);
        assert_eq!(results[0].display_title(), "One Piece");
        assert_eq!(results[1].id, 11061);
        assert_eq!(results[1].episodes, Some(148));
    }

    #[tokio::test]
    async fn media_parses_next_airing() {
        let mut server = mockito::Server::new_async().await;
        let body = r#"{
            "data": {
                "Media": {
                    "id": 21,
                    "title": {"romaji": "ONE PIECE", "english": "One Piece", "native": "ワンピース"},
                    "status": "RELEASING",
                    "episodes": null,
                    "format": "TV",
                    "seasonYear": 1999,
                    "nextAiringEpisode": {"episode": 1169, "airingAt": 1783865760, "timeUntilAiring": 250000}
                }
            }
        }"#;
        let _m = server
            .mock("POST", "/")
            .with_status(200)
            .with_body(body)
            .create_async()
            .await;

        let client = AniListClient::with_base_url(server.url());
        let media = client.media(21).await.unwrap().expect("media present");
        assert_eq!(media.id, 21);
        assert_eq!(media.display_title(), "One Piece");
        let next = media.next_airing_episode.expect("next airing");
        assert_eq!(next.episode, 1169);
        assert_eq!(next.airing_at, 1783865760);
        assert_eq!(next.time_until_airing, Some(250000));
    }

    #[tokio::test]
    async fn media_parses_finished_no_next() {
        let mut server = mockito::Server::new_async().await;
        let body = r#"{
            "data": {
                "Media": {
                    "id": 11061,
                    "title": {"romaji": "Hunter x Hunter", "english": "Hunter x Hunter (2011)", "native": null},
                    "status": "FINISHED",
                    "episodes": 148,
                    "format": "TV",
                    "seasonYear": 2011,
                    "nextAiringEpisode": null
                }
            }
        }"#;
        let _m = server
            .mock("POST", "/")
            .with_status(200)
            .with_body(body)
            .create_async()
            .await;

        let client = AniListClient::with_base_url(server.url());
        let media = client.media(11061).await.unwrap().expect("media present");
        assert_eq!(media.status.as_deref(), Some("FINISHED"));
        assert_eq!(media.episodes, Some(148));
        assert!(media.next_airing_episode.is_none());
    }

    #[tokio::test]
    async fn media_not_found_returns_none() {
        // Real AniList behavior for an unknown id: HTTP 404, but the body
        // still carries a parseable `data.Media: null` alongside `errors`.
        let mut server = mockito::Server::new_async().await;
        let body = r#"{"errors":[{"message":"Not Found.","status":404,"locations":[{"line":1,"column":9}]}],"data":{"Media":null}}"#;
        let _m = server
            .mock("POST", "/")
            .with_status(404)
            .with_body(body)
            .create_async()
            .await;

        let client = AniListClient::with_base_url(server.url());
        let media = client.media(999999999).await.unwrap();
        assert!(media.is_none());
    }

    #[tokio::test]
    async fn non_2xx_status_yields_error() {
        let mut server = mockito::Server::new_async().await;
        let _m = server
            .mock("POST", "/")
            .with_status(429)
            .with_body("rate limited")
            .create_async()
            .await;

        let client = AniListClient::with_base_url(server.url());
        let err = client.search("one piece", 10).await.unwrap_err();
        let msg = format!("{err:#}");
        assert!(msg.contains("429"), "expected 429 in error, got: {msg}");
    }

    #[tokio::test]
    async fn graphql_error_with_null_data_surfaces_message() {
        let mut server = mockito::Server::new_async().await;
        let body = r#"{"data":null,"errors":[{"message":"Validation error"}]}"#;
        let _m = server
            .mock("POST", "/")
            .with_status(200)
            .with_body(body)
            .create_async()
            .await;

        let client = AniListClient::with_base_url(server.url());
        let err = client.search("one piece", 10).await.unwrap_err();
        let msg = format!("{err:#}");
        assert!(
            msg.contains("Validation error"),
            "expected GraphQL error message, got: {msg}"
        );
    }

    #[test]
    fn display_title_prefers_english() {
        let m = Media {
            id: 1,
            title: MediaTitle {
                romaji: Some("romaji".into()),
                english: Some("English".into()),
                native: Some("native".into()),
            },
            status: None,
            format: None,
            episodes: None,
            season_year: None,
            next_airing_episode: None,
        };
        assert_eq!(m.display_title(), "English");
    }

    #[test]
    fn display_title_falls_back_to_romaji() {
        let m = Media {
            id: 2,
            title: MediaTitle {
                romaji: Some("Romaji".into()),
                english: None,
                native: Some("native".into()),
            },
            status: None,
            format: None,
            episodes: None,
            season_year: None,
            next_airing_episode: None,
        };
        assert_eq!(m.display_title(), "Romaji");
    }

    #[test]
    fn display_title_falls_back_to_native() {
        let m = Media {
            id: 3,
            title: MediaTitle {
                romaji: None,
                english: None,
                native: Some("ネイティブ".into()),
            },
            status: None,
            format: None,
            episodes: None,
            season_year: None,
            next_airing_episode: None,
        };
        assert_eq!(m.display_title(), "ネイティブ");
    }

    #[test]
    fn display_title_falls_back_to_placeholder() {
        let m = Media {
            id: 4,
            title: MediaTitle {
                romaji: None,
                english: None,
                native: None,
            },
            status: None,
            format: None,
            episodes: None,
            season_year: None,
            next_airing_episode: None,
        };
        assert_eq!(m.display_title(), "(untitled)");
    }
}
