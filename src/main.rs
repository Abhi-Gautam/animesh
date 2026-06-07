//! animesh — interactive TUI for tracking anime.
//!
//! There are no subcommands. The binary opens the ratatui shell;
//! everything happens through the in-app `:` palette / keymap.

mod anilist;
mod commands;
mod errors;
mod store;
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
    let result: Result<()> = rt.block_on(async { tui::run() });
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
