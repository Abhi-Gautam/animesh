//! `animesh probe` — run one sync engine tick on demand.
//!
//! Useful for "did the source pick up that new streamer link yet?"
//! debugging without waiting for the next scheduled refresh. Reuses
//! the same SyncEngine the long-running sync loop drives, so the
//! semantics are identical.

use std::sync::Arc;
use std::time::Duration;

use anyhow::{anyhow, Context, Result};

use crate::config::Config;
use crate::errors::user_error;
use crate::library::Library;
use crate::llm::{AnthropicClient, LlmClient};
use crate::notifier::{
    macos::MacOsNotifier, ntfy::NtfyNotifier, Dispatcher, Notifier,
};
use crate::sources::{anilist::AniListClient, Source};
use crate::store::{resolve_db_path, TtlConfig};
use crate::sync::{SyncEngine, SyncReport};
use crate::time::SystemClock;

/// CLI entry point. `args` is the argv after `probe`.
pub async fn run(args: &[String]) -> Result<()> {
    let opts = parse_args(args)?;
    let library = Arc::new(
        Library::open(&resolve_db_path()?, Arc::new(SystemClock))
            .context("open library for probe")?,
    );
    let config = Config::load_or_default(&Config::default_path()?)?;
    let engine = build_engine(library, &config, opts.ntfy_topic.as_deref())?;
    let report = engine.tick().await;
    println!("{}", render_report(&report));
    Ok(())
}

#[derive(Debug, Default, PartialEq, Eq)]
struct Opts {
    ntfy_topic: Option<String>,
}

fn parse_args(args: &[String]) -> Result<Opts> {
    let mut opts = Opts::default();
    let mut i = 0;
    while i < args.len() {
        match args[i].as_str() {
            "--ntfy-topic" => {
                let v = args
                    .get(i + 1)
                    .ok_or_else(|| user_error(anyhow!("--ntfy-topic needs a value")))?;
                opts.ntfy_topic = Some(v.clone());
                i += 2;
            }
            "--help" | "-h" => {
                eprintln!("Usage: animesh probe [--ntfy-topic TOPIC]");
                std::process::exit(0);
            }
            other => {
                return Err(user_error(anyhow!(
                    "unknown probe flag {other:?}; try --help"
                )));
            }
        }
    }
    Ok(opts)
}

fn build_engine(
    library: Arc<Library>,
    config: &Config,
    ntfy_topic: Option<&str>,
) -> Result<SyncEngine> {
    let sources: Vec<Arc<dyn Source>> = vec![Arc::new(AniListClient::new())];
    let mut dispatcher = Dispatcher::new(Arc::clone(&library));
    if let Some(topic) = ntfy_topic {
        if !topic.is_empty() {
            let n: Box<dyn Notifier> = Box::new(NtfyNotifier::new(topic.to_string()));
            dispatcher = dispatcher.with(n);
        }
    }
    // macOS native notifier is always on. osascript is a no-op on
    // non-macOS systems (it'll just fail and the dispatcher logs the
    // error without aborting other channels).
    dispatcher = dispatcher.with(Box::new(MacOsNotifier::new()));
    let dispatcher = Arc::new(dispatcher);
    Ok(SyncEngine::new(
        library,
        sources,
        dispatcher,
        config.subscriptions.video.clone(),
        Arc::new(SystemClock),
        TtlConfig::from_env(),
        Duration::from_secs(600),
    ))
}

fn render_report(report: &SyncReport) -> String {
    let mut s = String::new();
    s.push_str(&format!(
        "examined {} canonical(s): {} refreshed, {} fresh\n",
        report.canonicals_examined,
        report.source_refs_refreshed,
        report.source_refs_skipped_fresh,
    ));
    if !report.verified_releases.is_empty() {
        s.push_str(&format!(
            "verified releases ({}):\n",
            report.verified_releases.len()
        ));
        for v in &report.verified_releases {
            s.push_str(&format!(
                "  - {} on {} -> {}\n",
                v.canonical_id, v.streamer, v.deep_link
            ));
        }
    } else {
        s.push_str("verified releases: 0\n");
    }
    if !report.errors.is_empty() {
        s.push_str(&format!("errors ({}):\n", report.errors.len()));
        for e in &report.errors {
            s.push_str(&format!("  - {e}\n"));
        }
    }
    s
}

// Keep the LlmClient import live so the lint that disallows reqwest
// outside known modules sees a single canonical AnthropicClient
// reference. The probe command itself doesn't use the LLM today; the
// canonical service is invoked by follow-time canonicalization
// (task #17 wires it into the TUI).
#[allow(dead_code)]
fn _unused_llm_anchor(_c: AnthropicClient) -> Box<dyn LlmClient> {
    unreachable!()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ids::{CanonicalId, ReleaseKind};
    use crate::sync::VerifiedRelease;

    fn s(items: &[&str]) -> Vec<String> {
        items.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn parse_args_defaults_are_empty() {
        assert_eq!(parse_args(&[]).unwrap(), Opts::default());
    }

    #[test]
    fn parse_args_accepts_ntfy_topic() {
        let opts = parse_args(&s(&["--ntfy-topic", "my-topic"])).unwrap();
        assert_eq!(opts.ntfy_topic.as_deref(), Some("my-topic"));
    }

    #[test]
    fn parse_args_rejects_missing_ntfy_topic_value() {
        let err = parse_args(&s(&["--ntfy-topic"])).unwrap_err();
        assert!(format!("{err}").contains("--ntfy-topic"));
    }

    #[test]
    fn parse_args_rejects_unknown_flag() {
        let err = parse_args(&s(&["--bogus"])).unwrap_err();
        assert!(format!("{err}").contains("--bogus"));
    }

    #[test]
    fn render_report_summarizes_counts_and_verified() {
        let report = SyncReport {
            canonicals_examined: 3,
            source_refs_refreshed: 2,
            source_refs_skipped_fresh: 1,
            verified_releases: vec![VerifiedRelease {
                canonical_id: CanonicalId::new(ReleaseKind::Tv, "severance").unwrap(),
                streamer: "Netflix".into(),
                deep_link: "https://netflix.com/x".into(),
                verified_at: 1_000,
            }],
            errors: vec![],
        };
        let out = render_report(&report);
        assert!(out.contains("examined 3"));
        assert!(out.contains("2 refreshed"));
        assert!(out.contains("1 fresh"));
        assert!(out.contains("Netflix"));
        assert!(out.contains("https://netflix.com/x"));
    }

    #[test]
    fn render_report_renders_errors_section_when_present() {
        let report = SyncReport {
            canonicals_examined: 1,
            errors: vec!["bad thing".into()],
            ..SyncReport::default()
        };
        let out = render_report(&report);
        assert!(out.contains("errors (1)"));
        assert!(out.contains("bad thing"));
    }

    #[test]
    fn render_report_says_zero_when_no_verified_releases() {
        let report = SyncReport {
            canonicals_examined: 5,
            source_refs_refreshed: 5,
            ..SyncReport::default()
        };
        let out = render_report(&report);
        assert!(out.contains("verified releases: 0"));
    }
}
