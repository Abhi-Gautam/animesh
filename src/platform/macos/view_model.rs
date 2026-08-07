//! What the window renders, published by the engine thread.
//!
//! The same contract the menu uses: the engine publishes a snapshot, the
//! surface renders it. The surface never queries, so a slow refresh can never
//! stall a repaint.

use std::sync::{Arc, Mutex};

use serde::Serialize;

/// One followed title, as the window shows it.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
pub struct TitleView {
    pub media_id: i64,
    pub anilist_id: i64,
    pub title: String,
    pub episode: Option<i64>,
    /// Relative phrasing, already resolved against the owner's timezone.
    pub timing: String,
    /// Absolute local time, for the detail pane.
    pub airs_at: String,
    /// `releasing`, `hiatus`, `finished`, `not_yet_released`, `cancelled`, `unknown`.
    pub status: String,
    pub aired: bool,
    pub freshness: String,
    /// Whether macOS is holding a request for this episode.
    pub notified: bool,
    /// Which day column it belongs to, as a local weekday name.
    pub day: String,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
pub struct WindowModel {
    pub following: u32,
    pub tonight: Vec<TitleView>,
    pub upcoming: Vec<TitleView>,
    pub notifications_authorized: bool,
    /// Owner-facing remediation, already phrased.
    pub attention: Option<String>,
}

/// The handle the engine thread writes through.
#[derive(Debug, Clone, Default)]
pub struct WindowState(Arc<Mutex<WindowModel>>);

impl WindowState {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn publish(&self, model: WindowModel) {
        if let Ok(mut slot) = self.0.lock() {
            *slot = model;
        }
    }

    /// Serves the current snapshot to the page.
    ///
    /// Falls back to an empty model rather than failing the request: a poisoned
    /// lock should blank a pane, not break the window.
    pub fn snapshot_json(&self) -> String {
        let model = self.0.lock().map(|m| m.clone()).unwrap_or_default();
        serde_json::to_string(&model).unwrap_or_else(|_| "{}".to_owned())
    }
}
