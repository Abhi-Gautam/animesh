//! animesh — a personal release radar.
//!
//! The dependency direction is enforced by module layering and asserted by the
//! architecture check in CI:
//!
//! ```text
//! binaries          -> composition only
//! platform::macos   -> engine + Apple frameworks
//! engine            -> domain + library + ports
//! library           -> domain + store + source coordinator
//! store             -> domain + rusqlite
//! sources           -> source DTOs + domain observations
//! domain            -> std/serde/chrono only
//! ```

pub mod cli;
pub mod domain;
pub mod engine;
pub mod error;
pub mod ipc;
pub mod library;
pub mod paths;
pub mod platform;
pub mod sources;
pub mod store;

/// Version reported over IPC and in the health snapshot.
pub const PROCESS_VERSION: &str = env!("CARGO_PKG_VERSION");
