//! Domain operations that mutate the durable library.
//!
//! Originally the CLI command layer (each verb was its own
//! `Command` trait impl wrapping an inner). After the TUI became
//! the only surface, the wrappers were deleted; the inner functions
//! remain because the TUI's `App::dispatch` calls them directly.

pub mod follow;
pub mod sync;
