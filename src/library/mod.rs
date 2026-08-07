//! Application services: semantic mutations and transaction boundaries.
//!
//! The Library is the only layer that may combine the store and a source. It
//! owns every transaction, and no network call or Apple callback happens inside
//! one.

pub mod reducers;

pub mod service;
