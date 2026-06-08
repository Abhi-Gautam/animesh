//! ntfy.sh push notifier.
//!
//! POSTs to `https://ntfy.sh/{topic}` (or a self-hosted endpoint) with
//! the title + deep link in the body. The title is the title; the URL
//! is set via the `X-Click` header so taps on iOS/Android open the
//! deep link directly.

use anyhow::{anyhow, Context, Result};
use async_trait::async_trait;

use crate::sync::VerifiedRelease;

use super::Notifier;

const DEFAULT_BASE: &str = "https://ntfy.sh";

pub struct NtfyNotifier {
    http: reqwest::Client,
    base: String,
    topic: String,
}

impl NtfyNotifier {
    pub fn new(topic: impl Into<String>) -> Self {
        Self {
            http: reqwest::Client::new(),
            base: DEFAULT_BASE.to_string(),
            topic: topic.into(),
        }
    }

    pub fn with_base(mut self, base: impl Into<String>) -> Self {
        self.base = base.into();
        self
    }
}

#[async_trait]
impl Notifier for NtfyNotifier {
    fn channel(&self) -> &'static str {
        "ntfy"
    }

    async fn notify(&self, event: &VerifiedRelease) -> Result<()> {
        if self.topic.is_empty() {
            return Err(anyhow!("ntfy topic is empty; nothing to send"));
        }
        let url = format!("{}/{}", self.base.trim_end_matches('/'), self.topic);
        let title = format!("Now on {}", event.streamer);
        // The body carries the deep link as plain text so users see
        // it; the X-Click header gives the OS the one-tap target.
        let body = format!("{} — open in {}", event.deep_link, event.streamer);
        let resp = self
            .http
            .post(&url)
            .header("Title", title)
            .header("X-Click", &event.deep_link)
            .body(body)
            .send()
            .await
            .context("POST to ntfy")?;
        let status = resp.status();
        if !status.is_success() {
            let body = resp.text().await.unwrap_or_default();
            return Err(anyhow!("ntfy HTTP {status}: {body}"));
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ids::{CanonicalId, ReleaseKind};

    fn event() -> VerifiedRelease {
        VerifiedRelease {
            canonical_id: CanonicalId::new(ReleaseKind::Tv, "severance").unwrap(),
            streamer: "Netflix".into(),
            deep_link: "https://netflix.com/title/x".into(),
            verified_at: 1_000,
        }
    }

    #[tokio::test]
    async fn posts_to_topic_with_title_and_click_header() {
        let mut server = mockito::Server::new_async().await;
        let m = server
            .mock("POST", "/topic-abc")
            .match_header("Title", "Now on Netflix")
            .match_header("X-Click", "https://netflix.com/title/x")
            .with_status(200)
            .create_async()
            .await;
        let notifier = NtfyNotifier::new("topic-abc").with_base(server.url());
        notifier.notify(&event()).await.unwrap();
        m.assert_async().await;
    }

    #[tokio::test]
    async fn empty_topic_returns_clear_error() {
        let notifier = NtfyNotifier::new("");
        let err = notifier.notify(&event()).await.unwrap_err();
        assert!(format!("{err}").contains("topic"));
    }

    #[tokio::test]
    async fn non_2xx_status_surfaces_body() {
        let mut server = mockito::Server::new_async().await;
        let _m = server
            .mock("POST", "/abc")
            .with_status(403)
            .with_body("forbidden")
            .create_async()
            .await;
        let notifier = NtfyNotifier::new("abc").with_base(server.url());
        let err = notifier.notify(&event()).await.unwrap_err();
        let msg = format!("{err:#}");
        assert!(msg.contains("403"), "got: {msg}");
        assert!(msg.contains("forbidden"), "got: {msg}");
    }

    #[tokio::test]
    async fn trailing_slash_in_base_does_not_double_up() {
        let mut server = mockito::Server::new_async().await;
        let m = server
            .mock("POST", "/abc")
            .with_status(200)
            .create_async()
            .await;
        let notifier =
            NtfyNotifier::new("abc").with_base(format!("{}/", server.url()));
        notifier.notify(&event()).await.unwrap();
        m.assert_async().await;
    }

    #[test]
    fn channel_name_is_ntfy() {
        let notifier = NtfyNotifier::new("x");
        assert_eq!(notifier.channel(), "ntfy");
    }
}
