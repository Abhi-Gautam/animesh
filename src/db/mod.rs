//! Local Turso database infrastructure.
//!
//! Connection lifecycle lives in [`connection`]; the migration engine in
//! [`migrations`]. Table-specific repositories live in submodules
//! (e.g. [`watchlist`], [`notifications`]).

pub(crate) mod connection;
pub(crate) mod migrations;
pub(crate) mod notifications;
pub(crate) mod watchlist;

pub(crate) use connection::open_database;
#[cfg_attr(not(test), allow(unused_imports))]
pub(crate) use connection::open_path;
