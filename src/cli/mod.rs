//! The CLI: parsing, IPC, rendering, exit codes.

pub mod args;
pub mod render;

use std::process::ExitCode;

use crate::domain::time::{SystemClock, WallClock};
use crate::error::AppError;
use crate::ipc::client;
use crate::ipc::protocol::{Request, Response};
use crate::paths::AppPaths;

use args::{Cli, Command};

/// Runs one command and returns the process exit status.
pub async fn run(cli: Cli) -> ExitCode {
    match execute(cli).await {
        Ok(output) => {
            if !output.is_empty() {
                println!("{output}");
            }
            ExitCode::SUCCESS
        }
        Err(error) => {
            // Stderr is diagnostics; stdout stays data-only so piping works.
            eprintln!("animesh: {}", error.message);
            if let Some(seconds) = error.retry_after_secs {
                eprintln!("         try again in about {seconds}s");
            }
            ExitCode::from(u8::try_from(error.exit_category().code()).unwrap_or(2))
        }
    }
}

async fn execute(cli: Cli) -> Result<String, AppError> {
    let paths = AppPaths::production().map_err(|e| AppError::internal(e.to_string()))?;
    let now = SystemClock.now();

    match cli.command {
        Command::Search { query } => {
            let query = query.join(" ");
            let response = client::send(&paths, Request::SearchAnime { query }).await?;
            expect_search(response).map(|c| render::search(&c))
        }

        Command::Follow { id } => {
            let response = client::send(&paths, Request::FollowAnilist { id }).await?;
            match response {
                Response::FollowAnilist(result) => Ok(render::follow_result(&result, now)),
                other => Err(mismatched(&other)),
            }
        }

        Command::Next { limit } => {
            let response = client::send(&paths, Request::Upcoming { limit }).await?;
            match response {
                Response::Upcoming(rows) => {
                    warn_about_staleness(&rows);
                    Ok(render::upcoming(&rows, now))
                }
                other => Err(mismatched(&other)),
            }
        }

        Command::List => {
            let response = client::send(&paths, Request::ListFollows).await?;
            match response {
                Response::ListFollows(rows) => Ok(render::follows(&rows, now)),
                other => Err(mismatched(&other)),
            }
        }

        Command::Drop { media_id } => {
            let response = client::send(&paths, Request::Drop { media_id }).await?;
            match response {
                Response::Drop(summary) => {
                    Ok(format!("Stopped following {}.", summary.display_title))
                }
                other => Err(mismatched(&other)),
            }
        }

        Command::Refresh => {
            let response = client::send(&paths, Request::TriggerRefresh).await?;
            match response {
                Response::TriggerRefresh(accepted) => Ok(match accepted.disposition {
                    crate::domain::read_models::RefreshDisposition::Started => {
                        "Refresh started.".to_owned()
                    }
                    crate::domain::read_models::RefreshDisposition::AlreadyRunning => {
                        "A refresh is already running.".to_owned()
                    }
                }),
                other => Err(mismatched(&other)),
            }
        }

        Command::Status => {
            let response = client::send(&paths, Request::Status).await?;
            match response {
                Response::Status(snapshot) => Ok(render::health(&snapshot, now)),
                other => Err(mismatched(&other)),
            }
        }
    }
}

fn expect_search(
    response: Response,
) -> Result<Vec<crate::domain::media::SearchCandidate>, AppError> {
    match response {
        Response::SearchAnime(candidates) => Ok(candidates),
        other => Err(mismatched(&other)),
    }
}

/// Warns on stderr without suppressing the rows themselves.
fn warn_about_staleness(rows: &[crate::domain::read_models::UpcomingRelease]) {
    use crate::domain::read_models::Freshness;

    let stale = rows
        .iter()
        .filter(|r| r.freshness != Freshness::Fresh)
        .count();
    if stale > 0 {
        eprintln!(
            "animesh: {stale} of {} entries have not refreshed recently",
            rows.len()
        );
    }
}

fn mismatched(response: &Response) -> AppError {
    AppError::internal(format!(
        "the app answered a {} to a different request",
        response.name()
    ))
}
