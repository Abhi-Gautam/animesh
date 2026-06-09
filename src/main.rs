//! animesh — entry point. Opens the ratatui shell. No subcommands.

mod commands;
mod errors;
mod ids;
mod ingest;
mod library;
mod search;
mod sources;
mod store;
mod time;
mod tui;

use anyhow::Result;

use errors::{classify, ExitKind};

fn main() {
    // Multi-thread runtime so the TUI's synchronous run loop can
    // `block_in_place` on async AniList calls (`:sync`, `:follow`)
    // without deadlocking.
    let rt = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .expect("build tokio runtime");
    let _guard = rt.enter();
    let result: Result<()> = tui::run();
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
