//! The Animesh app process.
//!
//! Owns the database, the AniList client, the refresh scheduler, and the IPC
//! server. Bundled as `Contents/MacOS/Animesh`; the cargo bin is `animesh-app`
//! because macOS filesystems are case-insensitive and `Animesh` would collide
//! with the `animesh` CLI in the target directory.
//!
//! There is deliberately no `#[tokio::main]`. When the menu bar lands, the
//! primordial thread belongs to `NSApplication` and this runtime moves to a
//! named worker thread; [`engine::bootstrap::run`] is already the entry point
//! that will be called from there, so that change does not touch the engine.

use std::process::ExitCode;

use animesh::engine::bootstrap;
use animesh::paths::AppPaths;
use tokio::sync::watch;

fn main() -> ExitCode {
    init_tracing();

    let paths = match AppPaths::production() {
        Ok(paths) => paths,
        Err(error) => {
            eprintln!("Animesh: {error}");
            return ExitCode::from(2);
        }
    };

    // Current-thread on purpose: when the menu bar lands this runtime
    // moves onto one named worker thread and the primordial thread goes to
    // NSApplication. A multi-thread pool here would have to be undone then.
    let runtime = match tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
    {
        Ok(runtime) => runtime,
        Err(error) => {
            eprintln!("Animesh: could not start the async runtime: {error}");
            return ExitCode::from(2);
        }
    };

    runtime.block_on(async move {
        let (shutdown_tx, shutdown_rx) = watch::channel(false);
        tokio::spawn(await_signals(shutdown_tx));

        match bootstrap::run(&paths, shutdown_rx).await {
            Ok(()) => {
                tracing::info!("shut down cleanly");
                ExitCode::SUCCESS
            }
            // A lost singleton race is the expected way a second launch ends,
            // so it is not an error worth a stack trace.
            Err(animesh::ipc::endpoint::IpcError::AlreadyRunning) => {
                eprintln!("Animesh is already running.");
                ExitCode::from(3)
            }
            Err(error) => {
                eprintln!("Animesh: {error}");
                ExitCode::from(2)
            }
        }
    })
}

/// Flips the shutdown signal on `SIGTERM` or `SIGINT`.
///
/// launchd sends `SIGTERM` when it stops the agent, so this is the normal exit
/// path rather than an exceptional one.
async fn await_signals(shutdown: watch::Sender<bool>) {
    use tokio::signal::unix::{signal, SignalKind};

    let mut term = match signal(SignalKind::terminate()) {
        Ok(stream) => stream,
        Err(error) => {
            tracing::error!(error = %error, "cannot listen for SIGTERM");
            return;
        }
    };
    let mut interrupt = match signal(SignalKind::interrupt()) {
        Ok(stream) => stream,
        Err(error) => {
            tracing::error!(error = %error, "cannot listen for SIGINT");
            return;
        }
    };

    tokio::select! {
        _ = term.recv() => tracing::info!("SIGTERM"),
        _ = interrupt.recv() => tracing::info!("SIGINT"),
    }
    let _ = shutdown.send(true);
}

fn init_tracing() {
    use tracing_subscriber::EnvFilter;

    // Default logs omit queries, titles, and payloads. What the owner watches
    // is not something to leave lying in a log file.
    let filter =
        EnvFilter::try_from_env("ANIMESH_LOG").unwrap_or_else(|_| EnvFilter::new("animesh=info"));
    tracing_subscriber::fmt()
        .with_env_filter(filter)
        .with_target(false)
        .init();
}
