//! The Animesh menu-bar app.
//!
//! Owns the database, the AniList client, the refresh scheduler, notification
//! registration, and the IPC server. Bundled as `Contents/MacOS/Animesh`; the
//! cargo bin is `animesh-app` because macOS filesystems are case-insensitive
//! and `Animesh` would collide with the `animesh` CLI in the target directory.
//!
//! There is deliberately no `#[tokio::main]`: the primordial thread belongs to
//! `NSApplication`, and the engine runs on a named worker thread.

fn main() -> std::process::ExitCode {
    // V4B replaces this with the AppKit composition root.
    eprintln!("Animesh: not yet implemented");
    std::process::ExitCode::from(2)
}
