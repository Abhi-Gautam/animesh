//! Linux adapters for the engine's ports.
//!
//! Only notifications. There is no tray: the freedesktop StatusNotifierItem
//! spec is unevenly supported — GNOME dropped it and needs an extension — so
//! the CLI is the whole visible surface here by choice, not by omission.

pub mod notifications;
