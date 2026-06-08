//! Push-notification dispatch.
//!
//! Notifiers are sinks for [`VerifiedRelease`] events from the sync
//! loop's verify step. Two impls ship in v0.5:
//!
//!   * [`ntfy::NtfyNotifier`] — POSTs to a ntfy.sh topic over HTTP.
//!   * [`macos::MacOsNotifier`] — invokes `osascript` to surface a
//!     native macOS Notification Center entry.
//!
//! The [`Notifier`] trait is small — `channel()` for identification +
//! `notify()` for the send. Per-channel rate-limits / auth lives in
//! the impl, not the dispatcher.
//!
//! ## Dedupe
//!
//! [`Dispatcher::notify_once`] persists a "delivered" marker in the
//! kv store keyed on `(channel, canonical_id, deep_link)` so a
//! duplicate VerifiedRelease (same canonical re-verifies on a
//! subsequent sync tick) is a no-op. This is the "exactly-once per
//! (event, channel)" invariant from the design.

pub mod macos;
pub mod ntfy;

use std::sync::Arc;

use anyhow::{Context, Result};
use async_trait::async_trait;

use crate::library::Library;
use crate::sync::VerifiedRelease;

/// One push-notification channel.
#[async_trait]
pub trait Notifier: Send + Sync {
    /// Stable identifier used in kv dedupe keys. Match this to the
    /// channel name used by the doctor command.
    fn channel(&self) -> &'static str;

    /// Send. Errors surface to the dispatcher, which logs them and
    /// continues with the next notifier — one bad channel does not
    /// block others.
    async fn notify(&self, event: &VerifiedRelease) -> Result<()>;
}

/// Owns the set of registered notifiers and the dedupe state.
///
/// Hold one of these per process and share via `Arc<Dispatcher>`.
pub struct Dispatcher {
    library: Arc<Library>,
    notifiers: Vec<Box<dyn Notifier>>,
}

impl Dispatcher {
    pub fn new(library: Arc<Library>) -> Self {
        Self {
            library,
            notifiers: Vec::new(),
        }
    }

    pub fn with(mut self, n: Box<dyn Notifier>) -> Self {
        self.notifiers.push(n);
        self
    }

    /// Fan a single event out to every channel, skipping any that
    /// already delivered it. Returns the per-channel outcomes for
    /// the caller to log. A channel's `notify` failure does NOT mark
    /// the event delivered — next sync tick will retry.
    pub async fn notify_once(&self, event: &VerifiedRelease) -> Result<Vec<DispatchOutcome>> {
        let mut out = Vec::with_capacity(self.notifiers.len());
        for n in &self.notifiers {
            let key = dedupe_key(n.channel(), event);
            // Cheap kv lookup. Library uses Db's kv methods under the hood.
            let already = self.is_marked(&key)?;
            if already {
                out.push(DispatchOutcome::AlreadyDelivered {
                    channel: n.channel(),
                });
                continue;
            }
            match n.notify(event).await {
                Ok(()) => {
                    self.mark_delivered(&key, event.verified_at)?;
                    out.push(DispatchOutcome::Delivered {
                        channel: n.channel(),
                    });
                }
                Err(e) => {
                    out.push(DispatchOutcome::Failed {
                        channel: n.channel(),
                        reason: format!("{e:#}"),
                    });
                }
            }
        }
        Ok(out)
    }

    fn is_marked(&self, key: &str) -> Result<bool> {
        Ok(self.library.kv_get(key)?.is_some())
    }

    fn mark_delivered(&self, key: &str, at: i64) -> Result<()> {
        self.library
            .kv_set(key, &at.to_string())
            .context("mark_delivered kv_set")
    }
}

/// Per-channel outcome from a single dispatch.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DispatchOutcome {
    Delivered { channel: &'static str },
    AlreadyDelivered { channel: &'static str },
    Failed { channel: &'static str, reason: String },
}

/// Build a deterministic dedupe key for `(channel, canonical, link)`.
/// The verified_at is intentionally NOT in the key — a re-verification
/// at a later time for the same link must be a no-op, not a re-send.
fn dedupe_key(channel: &str, event: &VerifiedRelease) -> String {
    format!(
        "notify:{}:{}:{}",
        channel,
        event.canonical_id,
        event.deep_link
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ids::{CanonicalId, ReleaseKind};
    use crate::time::FixedClock;
    use std::sync::Mutex;

    /// Notifier that records every call instead of sending.
    struct RecordingNotifier {
        channel: &'static str,
        calls: Mutex<Vec<VerifiedRelease>>,
        fail_with: Mutex<Option<String>>,
    }

    impl RecordingNotifier {
        fn new(channel: &'static str) -> Self {
            Self {
                channel,
                calls: Mutex::new(Vec::new()),
                fail_with: Mutex::new(None),
            }
        }

        fn fail_next(&self, reason: &str) {
            *self.fail_with.lock().unwrap() = Some(reason.to_string());
        }

        fn calls(&self) -> Vec<VerifiedRelease> {
            self.calls.lock().unwrap().clone()
        }
    }

    #[async_trait]
    impl Notifier for RecordingNotifier {
        fn channel(&self) -> &'static str {
            self.channel
        }
        async fn notify(&self, event: &VerifiedRelease) -> Result<()> {
            if let Some(reason) = self.fail_with.lock().unwrap().take() {
                anyhow::bail!(reason);
            }
            self.calls.lock().unwrap().push(event.clone());
            Ok(())
        }
    }

    fn lib() -> Arc<Library> {
        Arc::new(Library::open_in_memory(Arc::new(FixedClock(1_000))).unwrap())
    }

    fn event() -> VerifiedRelease {
        VerifiedRelease {
            canonical_id: CanonicalId::new(ReleaseKind::Tv, "severance").unwrap(),
            streamer: "Netflix".into(),
            deep_link: "https://netflix.com/title/x".into(),
            verified_at: 1_000,
        }
    }

    #[tokio::test]
    async fn fan_out_calls_every_notifier_once() {
        let lib = lib();
        let a = RecordingNotifier::new("ntfy");
        let b = RecordingNotifier::new("macos");
        // We need to hand ownership of the notifiers to the
        // dispatcher AND inspect their state later, so we leak via
        // Arc<RecordingNotifier> instead of Box<dyn Notifier>. The
        // trait impl is on Arc<RecordingNotifier> through &self
        // delegation.
        let a = Arc::new(a);
        let b = Arc::new(b);
        struct ArcWrap(Arc<RecordingNotifier>);
        #[async_trait]
        impl Notifier for ArcWrap {
            fn channel(&self) -> &'static str {
                self.0.channel()
            }
            async fn notify(&self, e: &VerifiedRelease) -> Result<()> {
                self.0.notify(e).await
            }
        }
        let disp = Dispatcher::new(Arc::clone(&lib))
            .with(Box::new(ArcWrap(Arc::clone(&a))))
            .with(Box::new(ArcWrap(Arc::clone(&b))));
        let outcomes = disp.notify_once(&event()).await.unwrap();
        assert_eq!(outcomes.len(), 2);
        assert!(matches!(outcomes[0], DispatchOutcome::Delivered { .. }));
        assert!(matches!(outcomes[1], DispatchOutcome::Delivered { .. }));
        assert_eq!(a.calls().len(), 1);
        assert_eq!(b.calls().len(), 1);
    }

    #[tokio::test]
    async fn dedupe_skips_already_delivered_per_channel() {
        let lib = lib();
        let a = Arc::new(RecordingNotifier::new("ntfy"));
        struct ArcWrap(Arc<RecordingNotifier>);
        #[async_trait]
        impl Notifier for ArcWrap {
            fn channel(&self) -> &'static str {
                self.0.channel()
            }
            async fn notify(&self, e: &VerifiedRelease) -> Result<()> {
                self.0.notify(e).await
            }
        }
        let disp = Dispatcher::new(Arc::clone(&lib)).with(Box::new(ArcWrap(Arc::clone(&a))));
        disp.notify_once(&event()).await.unwrap();
        let outcomes = disp.notify_once(&event()).await.unwrap();
        assert_eq!(outcomes.len(), 1);
        assert!(matches!(outcomes[0], DispatchOutcome::AlreadyDelivered { .. }));
        assert_eq!(a.calls().len(), 1, "second dispatch must not call notifier");
    }

    #[tokio::test]
    async fn failure_in_one_channel_does_not_block_others() {
        let lib = lib();
        let fail = Arc::new(RecordingNotifier::new("ntfy"));
        let ok = Arc::new(RecordingNotifier::new("macos"));
        fail.fail_next("simulated outage");
        struct ArcWrap(Arc<RecordingNotifier>);
        #[async_trait]
        impl Notifier for ArcWrap {
            fn channel(&self) -> &'static str {
                self.0.channel()
            }
            async fn notify(&self, e: &VerifiedRelease) -> Result<()> {
                self.0.notify(e).await
            }
        }
        let disp = Dispatcher::new(Arc::clone(&lib))
            .with(Box::new(ArcWrap(Arc::clone(&fail))))
            .with(Box::new(ArcWrap(Arc::clone(&ok))));
        let outcomes = disp.notify_once(&event()).await.unwrap();
        assert!(matches!(outcomes[0], DispatchOutcome::Failed { .. }));
        assert!(matches!(outcomes[1], DispatchOutcome::Delivered { .. }));
        // The ok channel marked delivered; fail did not.
        let disp2 = Dispatcher::new(Arc::clone(&lib))
            .with(Box::new(ArcWrap(Arc::clone(&fail))))
            .with(Box::new(ArcWrap(Arc::clone(&ok))));
        let retry = disp2.notify_once(&event()).await.unwrap();
        assert!(matches!(retry[0], DispatchOutcome::Delivered { .. }), "fail retried");
        assert!(
            matches!(retry[1], DispatchOutcome::AlreadyDelivered { .. }),
            "ok skipped on retry"
        );
    }

    #[test]
    fn dedupe_key_is_deterministic_and_carries_channel() {
        let k1 = dedupe_key("ntfy", &event());
        let k2 = dedupe_key("ntfy", &event());
        assert_eq!(k1, k2);
        let k3 = dedupe_key("macos", &event());
        assert_ne!(k1, k3);
    }

    #[test]
    fn dedupe_key_ignores_verified_at() {
        let mut e1 = event();
        let mut e2 = event();
        e1.verified_at = 1_000;
        e2.verified_at = 9_999_999;
        assert_eq!(dedupe_key("ntfy", &e1), dedupe_key("ntfy", &e2));
    }
}
