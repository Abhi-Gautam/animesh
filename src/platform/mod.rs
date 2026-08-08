//! Platform adapters. The only place Apple frameworks are linked.

#[cfg(target_os = "linux")]
pub mod linux;

#[cfg(target_os = "macos")]
pub mod macos;
