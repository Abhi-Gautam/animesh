//! External sources.
//!
//! Source code performs transport and parsing only. It never opens the database
//! and never mutates storage: the Library owns every semantic mutation and
//! every transaction boundary.

pub mod anilist;
pub mod fetch;
pub mod tvmaze;
