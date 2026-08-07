//! Source-neutral, storage-neutral, platform-neutral types.
//!
//! Nothing here may import an AniList DTO, a SQLite type, an IPC frame, or an
//! AppKit class. That constraint is what lets the reducers in `library` be
//! tested as pure functions and keeps the wire contract from drifting into
//! whatever a source happened to return.

pub mod ids;
pub mod media;
pub mod notification;
pub mod read_models;
pub mod release;
pub mod time;
