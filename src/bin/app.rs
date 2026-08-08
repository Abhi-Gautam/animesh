//! The Animesh daemon.
//!
//! Owns the database, the AniList client, the refresh scheduler, the IPC
//! server, and — where the platform has one — the status item and the
//! notification centre. The CLI is a client of this process and never opens the
//! database itself.
//!
//! There are two composition roots because the platforms disagree about who
//! owns the main thread. On macOS `NSApplication` requires the primordial
//! thread and will not accept a substitute, so the async runtime lives on one
//! named worker and nothing crosses that boundary except two channels. Nothing
//! else claims the main thread, so everywhere else the engine simply runs on
//! it.
//!
//! Bundled on macOS as `Contents/MacOS/Animesh`; the cargo bin is `animesh-app`
//! because macOS filesystems are case-insensitive and `Animesh` would collide
//! with the `animesh` CLI in the target directory.

use std::process::ExitCode;
#[cfg(target_os = "linux")]
use std::sync::Arc;

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

    run(paths)
}

// ---------------------------------------------------------------------------
// macOS: AppKit owns the main thread, the engine gets a worker
// ---------------------------------------------------------------------------

#[cfg(target_os = "macos")]
fn run(paths: AppPaths) -> ExitCode {
    use animesh::platform::macos::menu_bar::{self, MenuBar, MenuState};
    use objc2_foundation::MainThreadMarker;
    use tokio::sync::mpsc;

    let state = MenuState::new();
    let (commands_tx, commands_rx) = mpsc::unbounded_channel();

    let engine_state = state.clone();
    let engine = std::thread::Builder::new()
        .name("animesh-engine".to_owned())
        .spawn(move || {
            let status = run_engine(paths, move |shutdown| {
                let hook: bootstrap::ReadyHook = Box::new(move |library, wake| {
                    tokio::spawn(animesh::platform::macos::app::run_surface(
                        library,
                        wake,
                        engine_state,
                        commands_rx,
                        shutdown,
                    ));
                });
                Some(hook)
            });

            // The socket is unlinked and the database is closed by this point,
            // so nothing is left for an orderly unwind to protect. Exiting from
            // here is what lets the AppKit run loop stay untouched on the main
            // thread: it never has to be woken to be told the engine is done.
            if MainThreadMarker::new().is_none() {
                std::process::exit(i32::from(status));
            }
            ExitCode::from(status)
        });

    let engine = match engine {
        Ok(handle) => handle,
        Err(error) => {
            eprintln!("Animesh: cannot start the engine thread: {error}");
            return ExitCode::from(2);
        }
    };

    // Headless is a supported mode, not a failure: it is what every `cargo run`
    // and every CI smoke does. Without a main-thread marker there is no AppKit,
    // so the engine simply owns the process instead.
    let Some(mtm) = MainThreadMarker::new() else {
        return join(engine);
    };

    let app = menu_bar::become_accessory(mtm);
    let _menu = MenuBar::install(mtm, commands_tx, state);
    tracing::info!("status item installed");

    // Never returns under normal operation.
    app.run();
    join(engine)
}

#[cfg(target_os = "macos")]
fn join(engine: std::thread::JoinHandle<ExitCode>) -> ExitCode {
    engine.join().unwrap_or_else(|_| {
        eprintln!("Animesh: the engine thread panicked");
        ExitCode::from(2)
    })
}

// ---------------------------------------------------------------------------
// Everywhere else: the engine owns the process
// ---------------------------------------------------------------------------

/// No status item, so nothing needs the main thread.
///
/// The CLI is the whole visible surface. Notifications attach through the same
/// [`bootstrap::ReadyHook`] the menu bar uses, but the desktop here holds no
/// schedule, so the daemon runs the notifier loop and fires them itself.
#[cfg(target_os = "linux")]
fn run(paths: AppPaths) -> ExitCode {
    use animesh::engine::{notifier, reconciler::Reconciler};
    use animesh::platform::linux::notifications::DesktopNotifier;

    let status = run_engine(paths, |shutdown| {
        let hook: bootstrap::ReadyHook = Box::new(move |library, _wake| {
            tokio::spawn(async move {
                // A missing session bus is an ordinary way to run the daemon —
                // a server, a container, an SSH session — not a failure. The
                // schedule and the CLI work either way; only banners are lost.
                let notifier_surface = match DesktopNotifier::connect().await {
                    Ok(surface) => surface,
                    Err(reason) => {
                        tracing::warn!(
                            reason = %reason,
                            "no desktop notifications; the CLI is unaffected"
                        );
                        return;
                    }
                };
                let reconciler = Arc::new(Reconciler::new(
                    Arc::clone(&library),
                    notifier_surface as Arc<dyn animesh::engine::reconciler::NotificationSurface>,
                ));
                notifier::run(library, reconciler, shutdown.subscribe()).await;
            });
        });
        Some(hook)
    });
    ExitCode::from(status)
}

/// Every other platform: the daemon and the CLI, with no notification adapter.
#[cfg(not(any(target_os = "macos", target_os = "linux")))]
fn run(paths: AppPaths) -> ExitCode {
    ExitCode::from(run_engine(paths, |_shutdown| None))
}

// ---------------------------------------------------------------------------
// Shared engine runtime
// ---------------------------------------------------------------------------

/// Runs the engine on this thread until shutdown, returning the exit status.
///
/// `on_ready` is built inside the runtime rather than passed in ready-made,
/// because a platform surface needs to spawn onto that runtime and needs the
/// shutdown sender to stop the process when the user quits.
fn run_engine<F>(paths: AppPaths, on_ready: F) -> u8
where
    F: FnOnce(watch::Sender<bool>) -> Option<bootstrap::ReadyHook>,
{
    // Current-thread on purpose: this is one thread doing one thing, and a work
    // stealing pool would only add threads for an idle process to keep parked.
    let runtime = match tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
    {
        Ok(runtime) => runtime,
        Err(error) => {
            eprintln!("Animesh: could not start the async runtime: {error}");
            return 2;
        }
    };

    runtime.block_on(async move {
        let (shutdown_tx, shutdown_rx) = watch::channel(false);
        tokio::spawn(await_signals(shutdown_tx.clone()));

        let hook = on_ready(shutdown_tx);

        match bootstrap::run(&paths, shutdown_rx, hook).await {
            Ok(()) => {
                tracing::info!("shut down cleanly");
                0
            }
            // A lost singleton race is the expected way a second launch ends,
            // so it is not an error worth a stack trace.
            Err(animesh::ipc::endpoint::IpcError::AlreadyRunning) => {
                eprintln!("Animesh is already running.");
                3
            }
            Err(error) => {
                eprintln!("Animesh: {error}");
                2
            }
        }
    })
}

/// Flips the shutdown signal on `SIGTERM` or `SIGINT`.
///
/// launchd and systemd both send `SIGTERM` to stop a unit, so this is the
/// normal exit path rather than an exceptional one.
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
