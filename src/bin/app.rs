//! The Animesh app process.
//!
//! Owns the database, the AniList client, the refresh scheduler, the IPC
//! server, the status item, and the notification centre. Bundled as
//! `Contents/MacOS/Animesh`; the cargo bin is `animesh-app` because macOS
//! filesystems are case-insensitive and `Animesh` would collide with the
//! `animesh` CLI in the target directory.
//!
//! The primordial thread belongs to `NSApplication` — AppKit requires it and
//! will not accept a substitute — so the async runtime lives on one named
//! worker thread. Nothing crosses that boundary except two channels.

use std::process::ExitCode;

use animesh::engine::bootstrap;
use animesh::paths::AppPaths;
use animesh::platform::macos::menu_bar::{self, MenuBar, MenuState};
use animesh::platform::macos::view_model::WindowState;
use animesh::platform::macos::window::MainWindow;
use objc2_foundation::MainThreadMarker;
use tokio::sync::{mpsc, watch};

fn main() -> ExitCode {
    init_tracing();

    let paths = match AppPaths::production() {
        Ok(paths) => paths,
        Err(error) => {
            eprintln!("Animesh: {error}");
            return ExitCode::from(2);
        }
    };

    let state = MenuState::new();
    let window_state = WindowState::new();
    let (commands_tx, commands_rx) = mpsc::unbounded_channel();

    let engine_state = state.clone();
    let engine_window = window_state.clone();
    let engine = std::thread::Builder::new()
        .name("animesh-engine".to_owned())
        .spawn(move || engine_thread(paths, engine_state, engine_window, commands_rx));

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

    // Opened at launch for now. Once the menu owns it, this becomes an action.
    match MainWindow::open(mtm, window_state) {
        Ok(_window) => tracing::info!("window opened"),
        Err(error) => tracing::error!(error = %error, "cannot open the window"),
    }

    // Never returns under normal operation. The engine thread exits the process
    // once it has released the socket and closed the database, which keeps
    // every AppKit call on this thread and needs no cross-thread wake-up.
    app.run();
    join(engine)
}

fn join(engine: std::thread::JoinHandle<ExitCode>) -> ExitCode {
    engine.join().unwrap_or_else(|_| {
        eprintln!("Animesh: the engine thread panicked");
        ExitCode::from(2)
    })
}

fn engine_thread(
    paths: AppPaths,
    state: MenuState,
    window: WindowState,
    commands: mpsc::UnboundedReceiver<menu_bar::MenuCommand>,
) -> ExitCode {
    // Current-thread on purpose: this is one thread doing one thing, and a work
    // stealing pool would only add threads for an idle process to keep parked.
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

    let status: u8 = runtime.block_on(async move {
        let (shutdown_tx, shutdown_rx) = watch::channel(false);
        tokio::spawn(await_signals(shutdown_tx.clone()));

        let surface_state = state.clone();
        let hook: bootstrap::ReadyHook = Box::new(move |library, wake| {
            tokio::spawn(animesh::platform::macos::app::run_surface(
                library,
                wake,
                surface_state,
                window,
                commands,
                shutdown_tx,
            ));
        });

        match bootstrap::run(&paths, shutdown_rx, Some(hook)).await {
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
    });

    // The socket is unlinked and the database is closed by this point, so there
    // is nothing left for an orderly unwind to protect. Exiting from here is
    // what lets the AppKit run loop stay untouched on the main thread: it never
    // has to be woken to be told the engine is done.
    if MainThreadMarker::new().is_none() {
        std::process::exit(i32::from(status));
    }
    ExitCode::from(status)
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
