//! AniList search client — the only source adapter that survived the trim.

use anyhow::{anyhow, Context, Result};
use reqwest::Client;
use serde::Deserialize;

const DEFAULT_BASE_URL: &str = "https://graphql.anilist.co";

pub(crate) struct AniListClient {
    client: Client,
    base_url: String,
}

impl AniListClient {
    pub(crate) fn new() -> Self {
        Self::with_base_url(DEFAULT_BASE_URL.into())
    }

    pub(crate) fn with_base_url(base_url: String) -> Self {
        Self {
            client: Client::new(),
            base_url,
        }
    }

    /// Search anime by free-form query. Returns raw JSON response.
    pub(crate) async fn search_raw(&self, query: &str, per_page: u32) -> Result<String> {
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
        let variables = serde_json::json!({ "search": query, "perPage": per_page });
        let request = serde_json::json!({ "query": body, "variables": variables });
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
        Ok(response_json)
    }

    /// Search and deserialize into structured results.
    pub(crate) async fn search(&self, query: &str, per_page: u32) -> Result<Vec<Media>> {
        let json = self.search_raw(query, per_page).await?;
        let resp: GraphQlResponse<PageMedia> =
            serde_json::from_str(&json).context("deserialize AniList search response")?;
        Ok(resp.data.page.media)
    }
}

impl Default for AniListClient {
    fn default() -> Self {
        Self::new()
    }
}

// ---------------------------------------------------------------------------
// Response shapes — minimal, only what search returns.
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

#[derive(Debug, Clone, Deserialize)]
pub(crate) struct Media {
    pub id: i64,
    pub title: MediaTitle,
    pub status: Option<String>,
    pub format: Option<String>,
    #[serde(rename = "nextAiringEpisode")]
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
                         "status": "RELEASING", "episodes": null, "format": "TV", "nextAiringEpisode": {"episode": 1100, "airingAt": 1700000000}},
                        {"id": 11061, "title": {"romaji": "Hunter x Hunter", "english": "Hunter x Hunter (2011)", "native": "ハンター×ハンター"},
                         "status": "FINISHED", "episodes": 148, "format": "TV", "nextAiringEpisode": null}
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
            next_airing_episode: None,
        };
        assert_eq!(m.display_title(), "(untitled)");
    }
}
