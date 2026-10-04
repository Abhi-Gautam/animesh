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
    /// Human `next`: just-aired band plus the future list. JSON uses
    /// [`Outcome::Replied`] with one or the other.
    Schedule {
        dropped: Vec<crate::domain::read_models::UpcomingRelease>,
        upcoming: Vec<crate::domain::read_models::UpcomingRelease>,
    },
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
        Outcome::Schedule { dropped, upcoming } => {
            warn_about_staleness(upcoming);
            warn_about_staleness(dropped);
            return render::schedule(dropped, upcoming, now);
        }
        Outcome::Replied(response) => response,
    };

    match response {
        Response::Search(results) => render::search(
            &results
                .items
                .iter()
                .map(|hit| hit.candidate.clone())
                .collect::<Vec<_>>(),
        ),
        Response::ResolveSearch(items) => render::search(
            &items
                .iter()
                .map(|hit| hit.candidate.clone())
                .collect::<Vec<_>>(),
        ),
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
        Response::View(snapshot) => format!("Data revision {}.", snapshot.stamp.revision),
        Response::Refresh(operation) => format!("Refresh {:?}.", operation.state),
        Response::WaitForRevision(stamp) => format!("Data revision {}.", stamp.revision),
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
    let json = cli.json;

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
            (crate::cli::args::FollowTarget::Bare(_), false) => {
                return Err(AppError::invalid_argument(crate::cli::args::BARE_FOLLOW));
            }
        },
        Command::Next { limit, dropped } => {
            if json || dropped {
                Request::Upcoming { limit, dropped }
            } else {
                // Human next: future list plus a short just-aired band that
                // does not count against `-n`.
                let dropped = send_request(
                    &paths,
                    Request::Upcoming {
                        limit: Some(crate::domain::read_models::DROPPED_BAND),
                        dropped: true,
                    },
                )
                .await?;
                let upcoming = send_request(
                    &paths,
                    Request::Upcoming {
                        limit,
                        dropped: false,
                    },
                )
                .await?;
                let dropped = match dropped {
                    Response::Upcoming(rows) => rows,
                    other => return Err(mismatched(&other)),
                };
                let upcoming = match upcoming {
                    Response::Upcoming(rows) => rows,
                    other => return Err(mismatched(&other)),
                };
                return Ok(Outcome::Schedule { dropped, upcoming });
            }
        }
        Command::List => Request::ListFollows,
        Command::Drop { id } => {
            let media_id = resolve_drop(&paths, id).await?;
            Request::Drop { media_id }
        }
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
    let response = send_request(&paths, request).await?;
    if response.name() != expected {
        return Err(mismatched(&response));
    }
    Ok(Outcome::Replied(response))
}

async fn send_request(
    paths: &crate::paths::AppPaths,
    request: Request,
) -> Result<Response, AppError> {
    client::send(paths, request).await
}

async fn resolve_drop(
    paths: &crate::paths::AppPaths,
    target: crate::cli::args::DropTarget,
) -> Result<crate::domain::ids::MediaId, AppError> {
    match target {
        crate::cli::args::DropTarget::Media(id) => Ok(id),
        crate::cli::args::DropTarget::Source(key) => {
            let response = send_request(paths, Request::ListFollows).await?;
            let rows = match response {
                Response::ListFollows(rows) => rows,
                other => return Err(mismatched(&other)),
            };
            rows.into_iter()
                .find(|row| row.source == key.source && row.source_id == key.id)
                .map(|row| row.media_id)
                .ok_or_else(|| AppError::not_found(format!("not following {key}")))
        }
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
