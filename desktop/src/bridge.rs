//! A daemon client. Source and library operations always travel over IPC.

use std::time::Duration;

use animesh::application::commands::{Request, Response};
use animesh::domain::command_center::{
    OperationState, OperationStatus, RefreshTarget, RevisionStamp, SearchHit, SearchResults,
    ViewData, ViewQuery, ViewSnapshot,
};
use animesh::domain::ids::{AniListId, MediaId, Source, SourceKey, TvMazeId};
use animesh::domain::media::SearchCandidate;
use animesh::domain::read_models::{FollowResult, FollowSummary};
use animesh::error::AppError;
use animesh::ipc::client;
use animesh::paths::AppPaths;
use serde::Serialize;
use tauri::{Emitter, State};

async fn request(paths: &AppPaths, command: Request) -> Result<Response, AppError> {
    client::send(paths, command).await
}

#[tauri::command]
pub async fn skill_status() -> Result<animesh::cli::skill::SkillStatus, AppError> {
    tauri::async_runtime::spawn_blocking(animesh::cli::skill::installation_status)
        .await
        .map_err(|_| AppError::internal("Could not check the Animesh skill"))?
}

#[tauri::command]
pub async fn install_skill() -> Result<animesh::cli::skill::SkillStatus, AppError> {
    tauri::async_runtime::spawn_blocking(|| {
        // Use the CLI's installer and conflict protection. No shell or daemon
        // is needed, and the webview cannot choose a path or overwrite edits.
        animesh::cli::skill::install(false)?;
        animesh::cli::skill::installation_status()
    })
    .await
    .map_err(|_| AppError::internal("Could not install the Animesh skill"))?
}

#[tauri::command]
pub async fn view(paths: State<'_, AppPaths>, query: ViewQuery) -> Result<ViewSnapshot, AppError> {
    read_view(&paths, query).await
}

async fn read_view(paths: &AppPaths, query: ViewQuery) -> Result<ViewSnapshot, AppError> {
    match request(paths, Request::View { query }).await? {
        Response::View(snapshot) => Ok(*snapshot),
        _ => Err(AppError::internal("Unexpected view response")),
    }
}

#[tauri::command]
pub async fn search_titles(
    paths: State<'_, AppPaths>,
    query: String,
) -> Result<SearchResults, AppError> {
    match request(&paths, Request::Search { query, kind: None }).await? {
        Response::Search(results) => Ok(results),
        _ => Err(AppError::internal("Unexpected search response")),
    }
}

#[tauri::command]
pub async fn resolve_search(
    paths: State<'_, AppPaths>,
    candidates: Vec<SearchCandidate>,
) -> Result<Vec<SearchHit>, AppError> {
    match request(&paths, Request::ResolveSearch { candidates }).await? {
        Response::ResolveSearch(results) => Ok(results),
        _ => Err(AppError::internal("Unexpected search state response")),
    }
}

#[tauri::command]
pub async fn follow_title(
    paths: State<'_, AppPaths>,
    key: SourceKey,
) -> Result<FollowResult, AppError> {
    let command = match key.source {
        Source::AniList => Request::FollowAnilist {
            id: AniListId::new(key.id.get())?,
        },
        Source::TvMaze => Request::FollowTv {
            id: TvMazeId::new(key.id.get())?,
        },
    };
    match request(&paths, command).await? {
        Response::FollowAnilist(result) | Response::FollowTv(result) => Ok(*result),
        _ => Err(AppError::internal("Unexpected follow response")),
    }
}

#[tauri::command]
pub async fn drop_title(
    paths: State<'_, AppPaths>,
    media_id: MediaId,
) -> Result<FollowSummary, AppError> {
    match request(&paths, Request::Drop { media_id }).await? {
        Response::Drop(summary) => Ok(*summary),
        _ => Err(AppError::internal("Unexpected drop response")),
    }
}

#[tauri::command]
pub async fn refresh(
    paths: State<'_, AppPaths>,
    target: RefreshTarget,
) -> Result<OperationStatus, AppError> {
    let mut operation = match request(&paths, Request::Refresh { target }).await? {
        Response::Refresh(operation) => operation,
        _ => return Err(AppError::internal("Unexpected refresh response")),
    };
    // Wait on revisions, not a source or database polling loop. A long refresh
    // remains visible as running after this bounded interactive wait.
    let deadline = tokio::time::Instant::now() + Duration::from_secs(90);
    loop {
        let snapshot = read_view(&paths, ViewQuery::Health).await?;
        if let ViewData::Health { operations, .. } = snapshot.view {
            if let Some(current) = operations
                .into_iter()
                .find(|current| current.id == operation.id)
            {
                operation = current;
            }
        }
        if !matches!(
            operation.state,
            OperationState::Queued | OperationState::Running
        ) || tokio::time::Instant::now() >= deadline
        {
            return Ok(operation);
        }
        wait(&paths, snapshot.stamp).await?;
    }
}

async fn wait(paths: &AppPaths, stamp: RevisionStamp) -> Result<RevisionStamp, AppError> {
    match request(
        paths,
        Request::WaitForRevision {
            instance_id: stamp.instance_id,
            after: stamp.revision,
        },
    )
    .await?
    {
        Response::WaitForRevision(stamp) => Ok(stamp),
        _ => Err(AppError::internal("Unexpected revision response")),
    }
}

fn source_url(key: SourceKey) -> String {
    match key.source {
        Source::AniList => format!("https://anilist.co/anime/{}", key.id),
        Source::TvMaze => format!("https://www.tvmaze.com/shows/{}", key.id),
    }
}

#[tauri::command]
pub async fn open_source(key: SourceKey) -> Result<(), AppError> {
    // Only typed, validated source identities reach the OS; the webview cannot
    // choose a program, argument or arbitrary URL.
    let program = if cfg!(target_os = "macos") {
        "/usr/bin/open"
    } else {
        "xdg-open"
    };
    let status = tokio::process::Command::new(program)
        .arg(source_url(key))
        .status()
        .await
        .map_err(|_| AppError::internal("Could not open the source page"))?;
    if status.success() {
        Ok(())
    } else {
        Err(AppError::internal("Could not open the source page"))
    }
}

#[derive(Clone, Serialize)]
struct Connection {
    connected: bool,
    message: Option<String>,
    stamp: Option<RevisionStamp>,
}

#[tauri::command]
pub async fn start_service(paths: State<'_, AppPaths>, restart: bool) -> Result<String, AppError> {
    start_background_service(&paths, restart).await
}

async fn start_background_service(paths: &AppPaths, restart: bool) -> Result<String, AppError> {
    let message =
        tauri::async_runtime::spawn_blocking(move || animesh::service::start_from_desktop(restart))
            .await
            .map_err(|_| AppError::internal("Could not start the background service"))??;
    // Registration returns before launchd/systemd starts the process. Only
    // report success once IPC answers; this is a bounded startup wait.
    tokio::time::timeout(Duration::from_secs(10), async {
        loop {
            if request(paths, Request::Status).await.is_ok() {
                break;
            }
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
    })
    .await
    .map_err(|_| {
        AppError::new(
            animesh::error::ErrorCode::Unavailable,
            "The background service was registered but has not answered yet. Retry the connection.",
        )
    })?;
    Ok(message)
}

pub async fn watch_connection(app: tauri::AppHandle, paths: AppPaths) {
    // A Linux launcher starts the same managed daemon as the CLI. macOS's
    // parent bundle already owns the menu bar and background engine.
    #[cfg(target_os = "linux")]
    if matches!(read_view(&paths, ViewQuery::Health).await, Err(error) if error.code == animesh::error::ErrorCode::Unavailable)
    {
        let _ = start_background_service(&paths, false).await;
    }
    let mut stamp: Option<RevisionStamp> = None;
    let mut connected = false;
    let mut last_error = String::new();
    let mut backoff = 1;
    loop {
        let result = match stamp.clone() {
            Some(stamp) => wait(&paths, stamp).await,
            None => read_view(&paths, ViewQuery::Health)
                .await
                .map(|snapshot| snapshot.stamp),
        };
        match result {
            Ok(next) => {
                let changed = stamp.as_ref().is_some_and(|previous| *previous != next);
                if !connected {
                    let _ = app.emit(
                        "connection",
                        Connection {
                            connected: true,
                            message: None,
                            stamp: Some(next.clone()),
                        },
                    );
                } else if changed {
                    let _ = app.emit("changed", &next);
                }
                stamp = Some(next);
                connected = true;
                backoff = 1;
                last_error.clear();
            }
            Err(error) => {
                if connected || last_error != error.message {
                    let _ = app.emit(
                        "connection",
                        Connection {
                            connected: false,
                            message: Some(error.message.clone()),
                            stamp: None,
                        },
                    );
                }
                connected = false;
                last_error = error.message;
                // Keep the old instance identity to detect a daemon restart.
                tokio::time::sleep(Duration::from_secs(backoff)).await;
                backoff = (backoff * 2).min(30);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use animesh::domain::ids::SourceNumericId;

    #[test]
    fn source_links_keep_colliding_numeric_ids_separate() {
        let id = SourceNumericId::new(21).expect("id");
        assert_eq!(
            source_url(SourceKey {
                source: Source::AniList,
                id
            }),
            "https://anilist.co/anime/21"
        );
        assert_eq!(
            source_url(SourceKey {
                source: Source::TvMaze,
                id
            }),
            "https://www.tvmaze.com/shows/21"
        );
    }
}
