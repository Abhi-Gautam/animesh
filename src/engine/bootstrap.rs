//! The composition root.
//!
//! Paths, then the singleton lock, then the socket, then the database. The lock
//! comes before the socket and before the database so a second instance can
//! never unlink a live endpoint or migrate a database another process owns.
//!
//! A permanent database failure does not exit. Under launchd's `KeepAlive` that
//! would be a crash loop, and a crash loop is indistinguishable from a hang. The
//! process stays up, answers `status` with the reason, and refuses everything
//! else.

use std::sync::Arc;

use tokio::sync::watch;

use crate::domain::read_models::{
    AuthorizationState, BootstrapState, DegradedReason, HealthSnapshot, NotificationCounts,
    RefreshCounts,
};
use crate::domain::time::{KeyedJitter, SystemClock, WallClock};
use crate::error::ErrorCode;
use crate::ipc::endpoint::{Endpoint, IpcError};
use crate::ipc::handlers::Dispatcher;
use crate::ipc::protocol::PROTOCOL_VERSION;
use crate::ipc::server;
use crate::library::service::Library;
use crate::paths::AppPaths;
use crate::sources::anilist::client::AniListClient;
use crate::store::connection::{Store, StoreError};
use crate::store::{graph, migrations};

/// Runs the engine until `shutdown` reports true.
///
/// Returns `Err` only for failures that leave nothing to serve — a lost
/// singleton race, or a socket that cannot be bound. Everything else degrades
/// into a queryable state rather than terminating.
pub async fn run(paths: &AppPaths, shutdown: watch::Receiver<bool>) -> Result<(), IpcError> {
    let endpoint = Endpoint::bind(paths)?;
    tracing::info!(socket = %endpoint.socket_path().display(), "endpoint bound");

    let clock = SystemClock;
    let started_at = clock.now();

    match open_library(paths).await {
        Ok(library) => {
            let library = Arc::new(library);
            let instance_id = library.instance_id();

            let (_wake, wake_rx) = super::wake_channel();
            let scheduler = tokio::spawn(super::run_scheduler(
                Arc::clone(&library),
                wake_rx,
                shutdown.clone(),
            ));

            tracing::info!("engine ready");
            let dispatcher = Dispatcher::Ready(Arc::clone(&library));
            serve(&endpoint, instance_id, dispatcher, shutdown).await;

            scheduler.abort();
            Ok(())
        }

        Err(error) => {
            // Permanent or not, there is nothing to serve from. Stay up so the
            // reason is reachable instead of vanishing into a restart loop.
            tracing::error!(error = %error, "bootstrap failed; serving status only");
            let snapshot = degraded_snapshot(&error, started_at);
            let dispatcher = Dispatcher::Degraded(Arc::new(snapshot));
            serve(&endpoint, Arc::from("degraded"), dispatcher, shutdown).await;
            Ok(())
        }
    }
}

async fn serve(
    endpoint: &Endpoint,
    instance_id: Arc<str>,
    dispatcher: Dispatcher,
    shutdown: watch::Receiver<bool>,
) {
    server::serve(
        endpoint,
        instance_id,
        move |request| {
            let dispatcher = dispatcher.clone();
            async move { dispatcher.dispatch(request).await }
        },
        shutdown,
    )
    .await;
}

async fn open_library(paths: &AppPaths) -> Result<Library, StoreError> {
    let store = Store::open(&paths.database())?;
    let now = SystemClock.now();

    // Migrations run on the writer connection, so nothing else can be mid
    // transaction while the schema moves.
    let migrated = store
        .writer()
        .call(move |conn| migrations::apply(conn, now.get()))
        .await?;

    if migrated > 0 {
        tracing::info!(count = migrated, "applied migrations");
        store
            .writer()
            .call(|conn| crate::store::connection::verify_integrity(conn))
            .await?;
    }

    let installation = store
        .write(move |tx| graph::ensure_installation(tx, now))
        .await?;

    let source = AniListClient::production()
        .map_err(|e| StoreError::Integrity(format!("building the AniList client: {e}")))?;

    Ok(Library::new(
        store,
        source,
        Arc::new(SystemClock),
        Arc::new(KeyedJitter::default()),
        installation,
        migrations::supported_version(),
    ))
}

fn degraded_snapshot(
    error: &StoreError,
    started_at: crate::domain::ids::UnixTimestamp,
) -> HealthSnapshot {
    let reason = match error.code() {
        ErrorCode::SchemaTooNew => DegradedReason::SchemaTooNew,
        ErrorCode::DatabaseCorrupt => DegradedReason::DatabaseCorrupt,
        ErrorCode::DatabaseReadOnly => DegradedReason::DatabaseReadOnly,
        _ => DegradedReason::MigrationFailed,
    };

    HealthSnapshot {
        process_version: crate::PROCESS_VERSION.to_owned(),
        schema_version: 0,
        protocol_version: PROTOCOL_VERSION,
        app_instance_id: "degraded".to_owned(),
        started_at,
        bootstrap: BootstrapState::Degraded,
        database_ready: false,
        active_follows: 0,
        earliest_upcoming: None,
        last_success_at: None,
        refresh: RefreshCounts::default(),
        source_blocked_until: None,
        notifications: NotificationCounts::default(),
        last_reconciled_at: None,
        authorization: AuthorizationState::Unknown,
        authorization_observed_at: None,
        degraded: vec![reason],
    }
}
