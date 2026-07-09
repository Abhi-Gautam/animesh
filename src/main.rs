//! animesh — AniList search + next-airing schedule from the command line.
//!
//! Usage:
//!   cargo run -- search "one piece"
//!   cargo run -- schedule 21

mod commands;
mod sources;

use anyhow::Result;

fn usage() -> ! {
    eprintln!(
        "\
animesh — personal anime release radar (CLI slice)

Usage:
  animesh search <query>     Search AniList; print candidate list with ids
  animesh schedule <id>      Next airing episode for an AniList media id

Examples:
  animesh search \"one piece\"
  animesh schedule 21
  animesh schedule 11061"
    );
    std::process::exit(1);
}

#[tokio::main]
async fn main() -> Result<()> {
    let mut args = std::env::args().skip(1);
    let cmd = match args.next() {
        Some(c) => c,
        None => usage(),
    };

    match cmd.as_str() {
        "search" => {
            let query = args.next().unwrap_or_else(|| usage());
            if query.trim().is_empty() {
                usage();
            }
            commands::search::run(&query).await
        }
        "schedule" => {
            let id = args.next().unwrap_or_else(|| usage());
            commands::schedule::run(&id).await
        }
        "-h" | "--help" | "help" => usage(),
        other => {
            eprintln!("unknown command: {other}\n");
            usage();
        }
    }
}
