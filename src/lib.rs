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

pub mod domain;
pub mod error;
pub mod paths;
pub mod sources;

/// Version reported over IPC and in the health snapshot.
pub const PROCESS_VERSION: &str = env!("CARGO_PKG_VERSION");
