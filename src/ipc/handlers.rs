//! Maps the protocol onto the Library.
//!
//! Contains no business logic and leaks no raw error chains: a handler either
//! forwards a Library call or reports that bootstrap failed.

use std::sync::Arc;

use crate::error::AppError;
use crate::library::service::Library;

use super::protocol::{Request, Response};
use super::server::bootstrap_unavailable;

/// What the server dispatches against.
///
/// `Degraded` still answers `status`, which is what keeps a permanently failed
/// bootstrap diagnosable instead of invisible.
#[derive(Debug, Clone)]
pub enum Dispatcher {
    Ready(Arc<Library>),
    Degraded(Arc<crate::domain::read_models::HealthSnapshot>),
}

impl Dispatcher {
    pub async fn dispatch(&self, request: Request) -> Result<Response, AppError> {
        match self {
            Self::Ready(library) => handle(library, request).await,
            Self::Degraded(snapshot) => {
                if request.served_when_degraded() {
                    Ok(Response::Status(Box::new(snapshot.as_ref().clone())))
                } else {
                    Err(bootstrap_unavailable())
                }
            }
        }
    }
}

async fn handle(library: &Library, request: Request) -> Result<Response, AppError> {
    match request {
        Request::Status => Ok(Response::Status(Box::new(library.health().await?))),

        Request::SearchAnime { query } => Ok(Response::SearchAnime(library.search(&query).await?)),

        Request::FollowAnilist { id } => {
            Ok(Response::FollowAnilist(Box::new(library.follow(id).await?)))
        }

        Request::Drop { media_id } => Ok(Response::Drop(Box::new(
            library.drop_follow(media_id).await?,
        ))),

        Request::ListFollows => Ok(Response::ListFollows(library.list_follows().await?)),

        Request::Upcoming { limit } => Ok(Response::Upcoming(library.upcoming(limit).await?)),

        Request::TriggerRefresh => Ok(Response::TriggerRefresh(library.trigger_refresh().await?)),
    }
}
