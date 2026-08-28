//! The CLI: parsing, IPC, rendering, exit codes.

pub mod args;
pub mod json;
pub mod render;
pub mod skill;

use std::process::ExitCode;

use crate::domain::time::{SystemClock, WallClock};
use crate::error::AppError;
use crate::ipc::client;
use crate::ipc::protocol::{Request, Response};
use crate::paths::AppPaths;

use args::{Cli, Command, ServiceAction, SkillAction};

/// What one command produced, before anyone decides how to print it.
///
/// The two output modes have to agree on what happened, so they render the same
/// value rather than each deriving it. A human string built in `execute` would
/// make `--json` a second implementation of every command.
#[derive(Debug)]
pub enum Outcome {
    /// The daemon answered. Carries the full reply, not a summary of it.
    Replied(Response),
    /// Handled without the socket — `service` and `skill`.
    Local {
        /// Names the command in JSON output, where there is no reply kind.
        kind: &'static str,
        message: String,
    },
}

/// Runs one command and returns the process exit status.
pub async fn run(cli: Cli) -> ExitCode {
    let json = cli.json;
    let now = SystemClock.now();

    match execute(cli).await {
        Ok(outcome) => {
            let output = if json {
                json::success(&outcome)
            } else {
                render_human(&outcome, now)
            };
            if !output.is_empty() {
                println!("{output}");
            }
            ExitCode::SUCCESS
        }
        Err(error) => {
            let code = ExitCode::from(u8::try_from(error.exit_category().code()).unwrap_or(2));
            if json {
                // One stream, one document: an agent parses stdout and never
                // has to correlate it with prose on stderr.
                println!("{}", json::failure(&error));
            } else {
                // Stderr is diagnostics; stdout stays data-only so piping works.
                eprintln!("animesh: {}", error.message);
                if let Some(seconds) = error.retry_after_secs {
                    eprintln!("         try again in about {seconds}s");
                }
            }
            code
        }
    }
}

/// Renders an outcome for a person.
fn render_human(outcome: &Outcome, now: crate::domain::ids::UnixTimestamp) -> String {
    let response = match outcome {
        Outcome::Local { message, .. } => return message.clone(),
        Outcome::Replied(response) => response,
    };

    match response {
        Response::SearchAnime(candidates) | Response::SearchTv(candidates) => {
            render::search(candidates)
        }
        Response::FollowAnilist(result) | Response::FollowTv(result) => {
            render::follow_result(result, now)
        }
        Response::Upcoming(rows) => {
            warn_about_staleness(rows);
            render::upcoming(rows, now)
        }
        Response::ListFollows(rows) => render::follows(rows, now),
        Response::Drop(summary) => format!("Stopped following {}.", summary.display_title),
        Response::TriggerRefresh(accepted) => match accepted.disposition {
            crate::domain::read_models::RefreshDisposition::Started => {
                "Refreshed. Run 'animesh next' to see the schedule.".to_owned()
            }
            crate::domain::read_models::RefreshDisposition::AlreadyRunning => {
                "Skipped: AniList is rate limiting. It will resume automatically.".to_owned()
            }
        },
        Response::Status(snapshot) => render::health(snapshot, now),
    }
}

async fn execute(cli: Cli) -> Result<Outcome, AppError> {
    // Two commands do not go over the socket: `service` is what makes the
    // socket exist, and `skill` only writes files an agent will read.
    match &cli.command {
        Command::Service { action } => {
            let message = match action {
                ServiceAction::Start => crate::service::start()?,
                ServiceAction::Stop => crate::service::stop()?,
                ServiceAction::Restart => crate::service::restart()?,
                ServiceAction::Status => crate::service::status()?,
            };
            return Ok(Outcome::Local {
                kind: "service",
                message,
            });
        }
        Command::Skill { action } => {
            let message = match action {
                SkillAction::Install { force } => skill::install(*force)?,
                SkillAction::Uninstall => skill::uninstall()?,
                SkillAction::Status => skill::status()?,
            };
            return Ok(Outcome::Local {
                kind: "skill",
                message,
            });
        }
        _ => {}
    }

    let paths = AppPaths::production().map_err(|e| AppError::internal(e.to_string()))?;

    let request = match cli.command {
        Command::Search { query, tv } => {
            let joined = query.join(" ");
            if tv {
                Request::SearchTv {
                    query: {
                        let trimmed = joined.trim();
                        if trimmed.is_empty() {
                            None
                        } else {
                            Some(trimmed.to_owned())
                        }
                    },
                }
            } else {
                Request::SearchAnime { query: joined }
            }
        }
        Command::Follow { id, tv } => match (id, tv) {
            (crate::cli::args::FollowTarget::TvMaze(id), _) => Request::FollowTv { id },
            (crate::cli::args::FollowTarget::AniList(_), true) => {
                return Err(AppError::invalid_argument(
                    "anilist:N cannot be combined with --tv; drop the flag or use tvmaze:N",
                ));
            }
            (crate::cli::args::FollowTarget::AniList(id), false) => Request::FollowAnilist { id },
            (crate::cli::args::FollowTarget::Bare(id), true) => Request::FollowTv {
                id: crate::domain::ids::TvMazeId::from_numeric(id),
            },
            (crate::cli::args::FollowTarget::Bare(id), false) => Request::FollowAnilist {
                id: crate::domain::ids::AniListId::from_numeric(id),
            },
        },
        Command::Next { limit } => Request::Upcoming { limit },
        Command::List => Request::ListFollows,
        Command::Drop { media_id } => Request::Drop { media_id },
        Command::Refresh => Request::TriggerRefresh,
        Command::Status => Request::Status,
        // Both returned above, before any socket work.
        Command::Service { .. } | Command::Skill { .. } => {
            return Ok(Outcome::Local {
                kind: "none",
                message: String::new(),
            })
        }
    };

    let expected = request.name();
    let response = client::send(&paths, request).await?;
    if response.name() != expected {
        return Err(mismatched(&response));
    }
    Ok(Outcome::Replied(response))
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
