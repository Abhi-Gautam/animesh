//! The `animesh` CLI.
//!
//! A thin IPC client. It never opens the database and never constructs an
//! AniList client; every product command goes through the app process so there
//! is exactly one source client and one policy.

fn main() -> std::process::ExitCode {
    // V1C replaces this with the clap surface from plan section 16.
    eprintln!("animesh: not yet implemented");
    std::process::ExitCode::from(2)
}
