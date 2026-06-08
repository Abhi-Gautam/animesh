//! `animesh context` — dump the taste graph for an external LLM agent.
//!
//! Reads `~/.config/animesh/config.toml` for the locale + region,
//! opens the durable library, and renders [`ContextExporter`] output
//! to stdout. Format defaults to JSON; pass `--format md` for the
//! human-readable Markdown variant.

use std::path::PathBuf;
use std::sync::Arc;

use anyhow::{anyhow, Context, Result};

use crate::context::{ContextExporter, ExportFormat};
use crate::errors::user_error;
use crate::library::Library;
use crate::store::resolve_db_path;
use crate::time::SystemClock;

/// CLI entry point. `args` is the argv after `context` (so
/// `animesh context --format md` calls this with `["--format", "md"]`).
pub async fn run(args: &[String]) -> Result<()> {
    let opts = parse_args(args)?;
    let library = Arc::new(
        Library::open(&db_path()?, Arc::new(SystemClock)).context("open library for context")?,
    );
    let exporter = ContextExporter::new(library);
    let body = exporter.export(opts.format, opts.since, opts.limit)?;
    println!("{body}");
    Ok(())
}

#[derive(Debug, PartialEq)]
struct Opts {
    format: ExportFormat,
    /// Unix-seconds threshold — only engagement events at or after
    /// this time get included. Defaults to 0 (all).
    since: i64,
    /// Cap on event count. 0 = no cap.
    limit: u32,
}

impl Default for Opts {
    fn default() -> Self {
        Self {
            format: ExportFormat::Json,
            since: 0,
            limit: 0,
        }
    }
}

fn parse_args(args: &[String]) -> Result<Opts> {
    let mut opts = Opts::default();
    let mut i = 0;
    while i < args.len() {
        match args[i].as_str() {
            "--format" => {
                let v = args
                    .get(i + 1)
                    .ok_or_else(|| user_error(anyhow!("--format needs a value (json|md)")))?;
                opts.format = match v.as_str() {
                    "json" => ExportFormat::Json,
                    "md" | "markdown" => ExportFormat::Markdown,
                    other => {
                        return Err(user_error(anyhow!(
                            "unknown --format {other:?}; want json or md"
                        )))
                    }
                };
                i += 2;
            }
            "--since" => {
                let v = args
                    .get(i + 1)
                    .ok_or_else(|| user_error(anyhow!("--since needs a unix-seconds value")))?;
                opts.since = v
                    .parse()
                    .map_err(|_| user_error(anyhow!("--since must be an integer, got {v:?}")))?;
                i += 2;
            }
            "--limit" => {
                let v = args
                    .get(i + 1)
                    .ok_or_else(|| user_error(anyhow!("--limit needs an integer")))?;
                opts.limit = v
                    .parse()
                    .map_err(|_| user_error(anyhow!("--limit must be an integer, got {v:?}")))?;
                i += 2;
            }
            "--help" | "-h" => {
                eprintln!(
                    "Usage: animesh context [--format json|md] [--since SECS] [--limit N]"
                );
                std::process::exit(0);
            }
            other => {
                return Err(user_error(anyhow!(
                    "unknown context flag {other:?}; try --help"
                )));
            }
        }
    }
    Ok(opts)
}

fn db_path() -> Result<PathBuf> {
    resolve_db_path()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn s(items: &[&str]) -> Vec<String> {
        items.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn parse_args_defaults_to_json_all_unlimited() {
        let opts = parse_args(&[]).unwrap();
        assert_eq!(opts, Opts::default());
    }

    #[test]
    fn parse_args_accepts_format_md() {
        let opts = parse_args(&s(&["--format", "md"])).unwrap();
        assert_eq!(opts.format, ExportFormat::Markdown);
    }

    #[test]
    fn parse_args_accepts_format_markdown_alias() {
        let opts = parse_args(&s(&["--format", "markdown"])).unwrap();
        assert_eq!(opts.format, ExportFormat::Markdown);
    }

    #[test]
    fn parse_args_rejects_unknown_format() {
        let err = parse_args(&s(&["--format", "xml"])).unwrap_err();
        assert!(format!("{err}").contains("xml"));
    }

    #[test]
    fn parse_args_rejects_missing_format_value() {
        let err = parse_args(&s(&["--format"])).unwrap_err();
        assert!(format!("{err}").contains("--format"));
    }

    #[test]
    fn parse_args_parses_since_and_limit() {
        let opts = parse_args(&s(&["--since", "1000", "--limit", "5"])).unwrap();
        assert_eq!(opts.since, 1000);
        assert_eq!(opts.limit, 5);
    }

    #[test]
    fn parse_args_rejects_non_integer_since() {
        let err = parse_args(&s(&["--since", "not-a-number"])).unwrap_err();
        assert!(format!("{err}").contains("--since"));
    }

    #[test]
    fn parse_args_rejects_unknown_flag() {
        let err = parse_args(&s(&["--bogus"])).unwrap_err();
        assert!(format!("{err}").contains("--bogus"));
    }

    // NB: the run() function itself reads from the real DB path and
    // would need a temp HOME to test cleanly. The integration of
    // ContextExporter is covered by its own snapshot tests; this
    // module's tests focus on the argv parse surface.
}
