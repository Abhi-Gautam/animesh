//! animesh — entry point.
//!
//! `animesh` with no args opens the ratatui shell (preserved v0.4
//! behavior). Subcommands route to `commands/*::run`:
//!
//!   * `context [--format json|md] [--since SECS] [--limit N]`
//!   * `sub list|add|remove ...`
//!   * `probe [--ntfy-topic TOPIC]`
//!
//! Each subcommand owns its argv parse and renders its own output.

mod canonical;
mod commands;
mod config;
mod context;
mod errors;
mod ids;
mod library;
mod llm;
mod notifier;
mod sources;
mod store;
mod sync;
mod time;
mod tui;

use anyhow::Result;

use errors::{classify, user_error, ExitKind};

fn main() {
    let args: Vec<String> = std::env::args().collect();
    // Multi-thread runtime so the TUI's synchronous run loop can
    // `block_in_place` on async AniList calls (`:sync`, `:follow`)
    // without deadlocking, and so the subcommands can await
    // sources/notifier/HTTP.
    let rt = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .expect("build tokio runtime");
    let result: Result<()> = rt.block_on(dispatch(&args));
    match result {
        Ok(()) => {}
        Err(e) => {
            let kind = classify(&e);
            eprintln!("error: {e:#}");
            if matches!(kind, ExitKind::Durable) {
                if let Ok(path) = store::resolve_db_path() {
                    eprintln!("       (durable error — DB at {})", path.display());
                }
            }
            std::process::exit(kind.code());
        }
    }
}

async fn dispatch(argv: &[String]) -> Result<()> {
    let sub = argv.get(1).map(String::as_str);
    let rest: Vec<String> = argv.iter().skip(2).cloned().collect();
    match sub {
        // Default + explicit TUI go to the ratatui shell. No args is
        // the v0.4-compatible path.
        None | Some("tui") => tui::run(),
        Some("context") => commands::context::run(&rest).await,
        Some("sub") => commands::sub::run(&rest).await,
        Some("probe") => commands::probe::run(&rest).await,
        Some("--help") | Some("-h") | Some("help") => {
            print_help();
            Ok(())
        }
        Some(other) => Err(user_error(anyhow::anyhow!(
            "unknown command: {other}\n\n{}",
            usage()
        ))),
    }
}

fn print_help() {
    println!("{}", usage());
}

fn usage() -> &'static str {
    "animesh — personal release radar\n\
     \n\
     USAGE:\n  \
       animesh [tui]                     open the interactive TUI (default)\n  \
       animesh context [--format json|md] [--since SECS] [--limit N]\n                                   \
                                         dump the taste graph for an LLM agent\n  \
       animesh sub list [video|audio|all]\n  \
       animesh sub add <video|audio> <name>\n  \
       animesh sub remove <video|audio> <name>\n                                   \
                                         manage streaming subscriptions\n  \
       animesh probe [--ntfy-topic TOPIC]\n                                   \
                                         run one sync engine tick + notify\n  \
       animesh --help                    this message"
}

#[cfg(test)]
mod tests {
    // The dispatch function is tested via its subcommand surfaces
    // (commands::*::tests). Argv -> command routing is small enough
    // that the per-command tests + a manual smoke run cover it.
}
