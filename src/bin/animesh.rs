//! The `animesh` CLI.
//!
//! A thin IPC client. It never opens the database and never constructs an
//! AniList client; every product command goes through the app process so there
//! is exactly one source client and one policy.

use std::process::ExitCode;

use animesh::cli::{self, args::Cli};
use clap::Parser;

fn main() -> ExitCode {
    let cli = Cli::parse();

    // A current-thread runtime: the CLI makes one request and exits, so a
    // worker pool would cost threads it never uses.
    let runtime = match tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
    {
        Ok(runtime) => runtime,
        Err(error) => {
            eprintln!("animesh: could not start the async runtime: {error}");
            return ExitCode::from(2);
        }
    };

    runtime.block_on(cli::run(cli))
}
