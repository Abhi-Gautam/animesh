//! macOS Notification Center via `osascript`.
//!
//! The native path is the only one that surfaces a tappable Notification
//! Center entry on macOS without setting up an `.app` bundle and code
//! signing. We shell out to `osascript -e 'display notification ...'`.
//!
//! The osascript invocation is not auditable for tap-target URL
//! (Notification Center doesn't expose deep links from `display
//! notification`). v0.5 includes the URL in the body so the user can
//! copy it; v0.6 will register an `.app` bundle or use UNUserNotification.

use std::process::Stdio;

use anyhow::{anyhow, Context, Result};
use async_trait::async_trait;

use crate::sync::VerifiedRelease;

use super::Notifier;

/// Strategy that builds the osascript argv. Decoupled from
/// std::process::Command so we can unit-test the shape of the
/// invocation without actually launching osascript.
pub trait OsascriptRunner: Send + Sync {
    fn run(&self, script: &str) -> Result<()>;
}

/// Real impl. Forks `osascript -e '<script>'` and waits for it. Errors
/// surface the non-zero exit + stderr.
pub struct OsascriptCmd;

impl OsascriptRunner for OsascriptCmd {
    fn run(&self, script: &str) -> Result<()> {
        let out = std::process::Command::new("osascript")
            .arg("-e")
            .arg(script)
            .stdout(Stdio::null())
            .stderr(Stdio::piped())
            .output()
            .context("spawn osascript")?;
        if !out.status.success() {
            let stderr = String::from_utf8_lossy(&out.stderr).into_owned();
            return Err(anyhow!("osascript exited {}: {stderr}", out.status));
        }
        Ok(())
    }
}

pub struct MacOsNotifier<R: OsascriptRunner = OsascriptCmd> {
    runner: R,
}

impl MacOsNotifier<OsascriptCmd> {
    pub fn new() -> Self {
        Self { runner: OsascriptCmd }
    }
}

impl Default for MacOsNotifier<OsascriptCmd> {
    fn default() -> Self {
        Self::new()
    }
}

impl<R: OsascriptRunner> MacOsNotifier<R> {
    pub fn with_runner(runner: R) -> Self {
        Self { runner }
    }
}

#[async_trait]
impl<R: OsascriptRunner + 'static> Notifier for MacOsNotifier<R> {
    fn channel(&self) -> &'static str {
        "macos"
    }

    async fn notify(&self, event: &VerifiedRelease) -> Result<()> {
        // We escape any double-quotes the source might have produced in
        // the streamer name or deep link so the AppleScript string
        // literal stays well-formed.
        let title = format!("Now on {}", escape_for_applescript(&event.streamer));
        let body = format!(
            "{} — open in {}",
            escape_for_applescript(&event.deep_link),
            escape_for_applescript(&event.streamer)
        );
        let script = format!(
            r#"display notification "{body}" with title "{title}""#
        );
        self.runner.run(&script)
    }
}

/// Escape `"` and `\` for inclusion inside an AppleScript double-quoted
/// string literal. AppleScript uses C-style backslash escapes inside
/// `"..."`, so this matches the language's escape rules exactly.
fn escape_for_applescript(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        match c {
            '"' => out.push_str(r#"\""#),
            '\\' => out.push_str(r"\\"),
            other => out.push(other),
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ids::{CanonicalId, ReleaseKind};
    use std::sync::Mutex;

    /// Test runner that records the script passed to it.
    struct RecordingRunner {
        scripts: Mutex<Vec<String>>,
        fail_with: Mutex<Option<String>>,
    }

    impl RecordingRunner {
        fn new() -> Self {
            Self {
                scripts: Mutex::new(Vec::new()),
                fail_with: Mutex::new(None),
            }
        }

        fn scripts(&self) -> Vec<String> {
            self.scripts.lock().unwrap().clone()
        }

        fn fail_next(&self, reason: &str) {
            *self.fail_with.lock().unwrap() = Some(reason.to_string());
        }
    }

    impl OsascriptRunner for RecordingRunner {
        fn run(&self, script: &str) -> Result<()> {
            if let Some(reason) = self.fail_with.lock().unwrap().take() {
                return Err(anyhow!(reason));
            }
            self.scripts.lock().unwrap().push(script.to_string());
            Ok(())
        }
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
    async fn builds_display_notification_script_with_title_and_body() {
        let runner = std::sync::Arc::new(RecordingRunner::new());
        struct ArcWrap(std::sync::Arc<RecordingRunner>);
        impl OsascriptRunner for ArcWrap {
            fn run(&self, s: &str) -> Result<()> {
                self.0.run(s)
            }
        }
        let notifier = MacOsNotifier::with_runner(ArcWrap(runner.clone()));
        notifier.notify(&event()).await.unwrap();
        let scripts = runner.scripts();
        assert_eq!(scripts.len(), 1);
        let s = &scripts[0];
        assert!(s.contains("display notification"));
        assert!(s.contains("with title"));
        assert!(s.contains("Now on Netflix"));
        assert!(s.contains("https://netflix.com/title/x"));
    }

    #[tokio::test]
    async fn escapes_double_quotes_in_streamer_name() {
        let runner = std::sync::Arc::new(RecordingRunner::new());
        struct ArcWrap(std::sync::Arc<RecordingRunner>);
        impl OsascriptRunner for ArcWrap {
            fn run(&self, s: &str) -> Result<()> {
                self.0.run(s)
            }
        }
        let notifier = MacOsNotifier::with_runner(ArcWrap(runner.clone()));
        let mut ev = event();
        ev.streamer = r#"Net"flix"#.into();
        notifier.notify(&ev).await.unwrap();
        let s = &runner.scripts()[0];
        assert!(s.contains(r#"Net\"flix"#));
    }

    #[tokio::test]
    async fn runner_failure_surfaces_as_error() {
        let runner = std::sync::Arc::new(RecordingRunner::new());
        runner.fail_next("osascript not found");
        struct ArcWrap(std::sync::Arc<RecordingRunner>);
        impl OsascriptRunner for ArcWrap {
            fn run(&self, s: &str) -> Result<()> {
                self.0.run(s)
            }
        }
        let notifier = MacOsNotifier::with_runner(ArcWrap(runner.clone()));
        let err = notifier.notify(&event()).await.unwrap_err();
        assert!(format!("{err}").contains("osascript"));
    }

    #[test]
    fn channel_name_is_macos() {
        let runner = RecordingRunner::new();
        let notifier = MacOsNotifier::with_runner(runner);
        assert_eq!(notifier.channel(), "macos");
    }

    #[test]
    fn escape_for_applescript_handles_quotes_and_backslashes() {
        assert_eq!(escape_for_applescript("foo"), "foo");
        assert_eq!(escape_for_applescript("f\"oo"), r#"f\"oo"#);
        assert_eq!(escape_for_applescript("a\\b"), r"a\\b");
    }
}
