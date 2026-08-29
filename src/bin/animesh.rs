//! The `animesh` CLI.
//!
//! A thin IPC client. It never opens the database and never constructs an
//! AniList client; every product command goes through the app process so there
//! is exactly one source client and one policy.

use std::process::ExitCode;

use animesh::cli::{self, args::Cli, json};
use animesh::error::AppError;
use clap::error::ErrorKind;
use clap::Parser;

fn main() -> ExitCode {
    let cli = match Cli::try_parse() {
        Ok(cli) => cli,
        Err(error) => return clap_failure(error),
    };

    // A current-thread runtime: the CLI makes one request and exits, so a
    // worker pool would cost threads it never uses.
    let runtime = match tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
    {
        Ok(runtime) => runtime,
        Err(error) => {
            if wants_json() {
                println!(
                    "{}",
                    json::failure(&AppError::internal(format!(
                        "could not start the async runtime: {error}"
                    )))
                );
            } else {
                eprintln!("animesh: could not start the async runtime: {error}");
            }
            return ExitCode::from(2);
        }
    };

    runtime.block_on(cli::run(cli))
}

fn wants_json() -> bool {
    std::env::args().any(|a| a == "--json")
}

fn clap_failure(error: clap::Error) -> ExitCode {
    match error.kind() {
        ErrorKind::DisplayHelp | ErrorKind::DisplayVersion => {
            let _ = error.print();
            ExitCode::SUCCESS
        }
        _ if wants_json() => {
            let message = clap_message(&error);
            println!("{}", json::failure(&AppError::invalid_argument(message)));
            ExitCode::from(1)
        }
        _ => error.exit(),
    }
}

fn clap_message(error: &clap::Error) -> String {
    let rendered = error.to_string();
    rendered
        .lines()
        .find(|line| !line.is_empty() && !line.starts_with("Usage:"))
        .unwrap_or("invalid arguments")
        .trim()
        .trim_start_matches("error: ")
        .to_owned()
}
