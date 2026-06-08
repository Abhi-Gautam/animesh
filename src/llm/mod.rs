//! Anthropic API wrapper.
//!
//! Used by the canonicalization service. Trait-shaped so tests use a
//! stub instead of hitting the real API. The only call site for real
//! HTTP is [`AnthropicClient`]; everywhere else takes
//! `Arc<dyn LlmClient>`.
//!
//! Models: callers pass a model id string. The canonical service uses
//! [`models::CANONICALIZE_DEFAULT`] (`claude-haiku-4-5-20251001`)
//! because canonicalization is high-volume, low-stakes (rate-limit on
//! the API + cost matter more than reasoning depth here).
//!
//! Temperature: callers set it. The canonicalizer uses `0.0` for
//! determinism — same inputs → same outputs → cacheable.

use anyhow::{anyhow, Context, Result};
use async_trait::async_trait;
use serde::{Deserialize, Serialize};

const DEFAULT_API_URL: &str = "https://api.anthropic.com/v1/messages";
const ANTHROPIC_VERSION: &str = "2023-06-01";

/// Default models the project uses. Keep these constants in one place
/// so swapping models is a single edit.
pub mod models {
    /// High-volume, low-latency model for canonicalization decisions.
    /// Cheap enough that we can call it for every new source row.
    pub const CANONICALIZE_DEFAULT: &str = "claude-haiku-4-5-20251001";
}

/// One request to the LLM. The shape mirrors Anthropic's `messages`
/// endpoint but is provider-neutral at the trait boundary.
#[derive(Debug, Clone)]
pub struct LlmRequest {
    pub model: String,
    pub system: Option<String>,
    pub user: String,
    pub max_tokens: u32,
    pub temperature: f32,
}

/// The LLM's reply. Text-only for now; tool-use lands when a caller
/// needs it.
#[derive(Debug, Clone)]
pub struct LlmResponse {
    pub text: String,
}

/// The contract every LLM impl satisfies. Async because the real
/// client is HTTP; the trait is Send+Sync so it can travel through
/// the sync loop's tokio tasks behind an `Arc`.
#[async_trait]
pub trait LlmClient: Send + Sync {
    async fn complete(&self, req: LlmRequest) -> Result<LlmResponse>;
}

/// Real Anthropic HTTP client. The only caller of `reqwest::Client` in
/// this module. Constructed once at startup via
/// [`AnthropicClient::from_env`] which reads `ANTHROPIC_API_KEY`; the
/// canonical service holds an `Arc<dyn LlmClient>` and never knows
/// whether it's hitting the real API or a stub.
pub struct AnthropicClient {
    http: reqwest::Client,
    url: String,
    api_key: String,
}

impl std::fmt::Debug for AnthropicClient {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        // Never debug-print the API key.
        f.debug_struct("AnthropicClient")
            .field("url", &self.url)
            .field("api_key", &"<redacted>")
            .finish()
    }
}

impl AnthropicClient {
    /// Build from the `ANTHROPIC_API_KEY` env var. Returns an error
    /// with a clear remediation message if it's missing — the caller
    /// (the canonical service builder) decides whether to fall back
    /// to a no-op LLM or fail fast.
    pub fn from_env() -> Result<Self> {
        let api_key = std::env::var("ANTHROPIC_API_KEY")
            .map_err(|_| anyhow!("ANTHROPIC_API_KEY is not set; canonicalization needs it"))?;
        Ok(Self {
            http: reqwest::Client::new(),
            url: DEFAULT_API_URL.to_string(),
            api_key,
        })
    }

    /// Override the API URL (used by tests against a mockito server).
    #[allow(dead_code)] // Used by integration tests we'll add later.
    pub fn with_url(mut self, url: String) -> Self {
        self.url = url;
        self
    }
}

#[async_trait]
impl LlmClient for AnthropicClient {
    async fn complete(&self, req: LlmRequest) -> Result<LlmResponse> {
        let body = AnthropicRequestBody::from(&req);
        let resp = self
            .http
            .post(&self.url)
            .header("x-api-key", &self.api_key)
            .header("anthropic-version", ANTHROPIC_VERSION)
            .json(&body)
            .send()
            .await
            .context("POST to Anthropic /messages")?;
        let status = resp.status();
        if !status.is_success() {
            let body = resp.text().await.unwrap_or_default();
            return Err(anyhow!("Anthropic HTTP {status}: {body}"));
        }
        let parsed: AnthropicResponse = resp
            .json()
            .await
            .context("deserialize Anthropic response")?;
        let text = parsed
            .content
            .into_iter()
            .filter_map(|b| match b {
                AnthropicBlock::Text { text } => Some(text),
            })
            .collect::<Vec<_>>()
            .join("");
        Ok(LlmResponse { text })
    }
}

// ---------------------------------------------------------------------------
// Anthropic wire types. Module-private; the public trait is provider-
// neutral.
// ---------------------------------------------------------------------------

#[derive(Debug, Serialize)]
struct AnthropicRequestBody {
    model: String,
    max_tokens: u32,
    temperature: f32,
    #[serde(skip_serializing_if = "Option::is_none")]
    system: Option<String>,
    messages: Vec<AnthropicMessage>,
}

impl AnthropicRequestBody {
    fn from(req: &LlmRequest) -> Self {
        Self {
            model: req.model.clone(),
            max_tokens: req.max_tokens,
            temperature: req.temperature,
            system: req.system.clone(),
            messages: vec![AnthropicMessage {
                role: "user".to_string(),
                content: req.user.clone(),
            }],
        }
    }
}

#[derive(Debug, Serialize)]
struct AnthropicMessage {
    role: String,
    content: String,
}

#[derive(Debug, Deserialize)]
struct AnthropicResponse {
    content: Vec<AnthropicBlock>,
}

#[derive(Debug, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
enum AnthropicBlock {
    Text { text: String },
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn anthropic_client_parses_text_response() {
        let mut server = mockito::Server::new_async().await;
        let _m = server
            .mock("POST", "/")
            .with_status(200)
            .with_body(
                r#"{"content":[{"type":"text","text":"hello world"}]}"#,
            )
            .create_async()
            .await;
        let client = AnthropicClient {
            http: reqwest::Client::new(),
            url: server.url(),
            api_key: "test".into(),
        };
        let resp = client
            .complete(LlmRequest {
                model: models::CANONICALIZE_DEFAULT.into(),
                system: None,
                user: "hi".into(),
                max_tokens: 100,
                temperature: 0.0,
            })
            .await
            .unwrap();
        assert_eq!(resp.text, "hello world");
    }

    #[tokio::test]
    async fn anthropic_client_concatenates_multi_block_text() {
        let mut server = mockito::Server::new_async().await;
        let _m = server
            .mock("POST", "/")
            .with_status(200)
            .with_body(
                r#"{"content":[{"type":"text","text":"part1 "},{"type":"text","text":"part2"}]}"#,
            )
            .create_async()
            .await;
        let client = AnthropicClient {
            http: reqwest::Client::new(),
            url: server.url(),
            api_key: "test".into(),
        };
        let resp = client
            .complete(LlmRequest {
                model: models::CANONICALIZE_DEFAULT.into(),
                system: None,
                user: "x".into(),
                max_tokens: 100,
                temperature: 0.0,
            })
            .await
            .unwrap();
        assert_eq!(resp.text, "part1 part2");
    }

    #[tokio::test]
    async fn anthropic_client_surfaces_non_2xx_status_with_body() {
        let mut server = mockito::Server::new_async().await;
        let _m = server
            .mock("POST", "/")
            .with_status(401)
            .with_body("invalid key")
            .create_async()
            .await;
        let client = AnthropicClient {
            http: reqwest::Client::new(),
            url: server.url(),
            api_key: "bad".into(),
        };
        let err = client
            .complete(LlmRequest {
                model: models::CANONICALIZE_DEFAULT.into(),
                system: None,
                user: "x".into(),
                max_tokens: 100,
                temperature: 0.0,
            })
            .await
            .unwrap_err();
        let msg = format!("{err:#}");
        assert!(msg.contains("401"), "got: {msg}");
        assert!(msg.contains("invalid key"));
    }

    #[test]
    fn from_env_errors_clearly_when_key_missing() {
        // Use a scope to make sure the env var is unset locally even
        // if the developer has it set in their shell.
        let saved = std::env::var("ANTHROPIC_API_KEY").ok();
        std::env::remove_var("ANTHROPIC_API_KEY");
        let err = AnthropicClient::from_env().unwrap_err();
        let msg = format!("{err}");
        assert!(msg.contains("ANTHROPIC_API_KEY"), "got: {msg}");
        if let Some(v) = saved {
            std::env::set_var("ANTHROPIC_API_KEY", v);
        }
    }
}
