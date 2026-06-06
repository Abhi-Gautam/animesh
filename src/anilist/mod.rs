use reqwest::Client;
use serde::{de::DeserializeOwned, Serialize};

const ANILIST_API_URL: &str = "https://graphql.anilist.co";

/// A client for interacting with the AniList API
pub struct AniListClient {
    client: Client,
}

impl AniListClient {
    /// Create a new AniList API client
    pub fn new() -> Self {
        Self {
            client: Client::new(),
        }
    }

    /// Execute a GraphQL query
    pub async fn query<T: DeserializeOwned, V: Serialize>(
        &self,
        query: &str,
        variables: V,
    ) -> Result<T, reqwest::Error> {
        let response = self
            .client
            .post(ANILIST_API_URL)
            .json(&serde_json::json!({
                "query": query,
                "variables": variables,
            }))
            .send()
            .await?;

        response.json().await
    }
}

impl Default for AniListClient {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_query() {
        let client = AniListClient::new();
        let query = r#"
            query {
                Page {
                    media {
                        id
                        title {
                            romaji
                        }
                    }
                }
            }
        "#;

        let result: serde_json::Value = client.query(query, serde_json::json!({})).await.unwrap();
        assert!(result.get("data").is_some());
    }
}
