//! TVmaze source: transport, wire shapes, parsing.

pub mod client;
pub mod dto;
pub mod parser;

pub use client::TvMazeClient;
pub use parser::{parse_detail, parse_simulcast, DetailResult};
