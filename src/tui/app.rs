//! Top-level App state — owns the model, drives the event loop.

use crate::store::Db;
use crate::tui::model::Library;
use crate::tui::palette::PaletteState;
use crate::tui::pane::{Pane, Windows};
use crate::tui::toast::ToastQueue;

/// Which overlay (if any) is intercepting input.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Overlay {
    None,
    Palette,
    Help,
}

/// Which pane is focused. `0/1/2` map to Today / Late / Backlog so
/// number keys (`1` `2` `3`) trivially map to indices.
pub const PANE_TODAY: usize = 0;
pub const PANE_LATE: usize = 1;
pub const PANE_BACKLOG: usize = 2;
pub const PANE_LABELS: [&str; 3] = ["TODAY", "LATE · UNWATCHED", "BACKLOG"];
pub const PANE_KINDS: [Pane; 3] = [Pane::Today, Pane::Late, Pane::Backlog { behind: 0 }];

pub struct App {
    pub db: Db,
    pub library: Library,
    pub focused_pane: usize,
    /// Per-pane cursor; remembered across pane switches.
    pub selection: [usize; 3],
    pub overlay: Overlay,
    pub palette: PaletteState,
    pub toasts: ToastQueue,
    pub windows: Windows,
    pub now: i64,
    /// Set to true to exit the run loop.
    pub quit: bool,
}

impl App {
    pub fn new(db: Db, library: Library, windows: Windows, now: i64) -> Self {
        Self {
            db,
            library,
            focused_pane: PANE_TODAY,
            selection: [0; 3],
            overlay: Overlay::None,
            palette: PaletteState::default(),
            toasts: ToastQueue::default(),
            windows,
            now,
            quit: false,
        }
    }

    /// Index of the currently focused pane.
    pub fn focused_index(&self) -> usize {
        self.focused_pane
    }

    /// Items in the focused pane.
    pub fn focused_items(&self) -> Vec<&crate::tui::model::Show> {
        self.items_in(self.focused_pane)
    }

    pub fn items_in(&self, pane: usize) -> Vec<&crate::tui::model::Show> {
        let pane_kind = PANE_KINDS[pane];
        self.library
            .shows
            .iter()
            .filter(move |s| match (pane_kind, s.pane) {
                (Pane::Today, Some(Pane::Today)) => true,
                (Pane::Late, Some(Pane::Late)) => true,
                (Pane::Backlog { .. }, Some(Pane::Backlog { .. })) => true,
                _ => false,
            })
            .collect()
    }

    pub fn current(&self) -> Option<&crate::tui::model::Show> {
        let pane = self.focused_pane;
        let items = self.items_in(pane);
        let idx = self.selection[pane].min(items.len().saturating_sub(1));
        items.get(idx).copied()
    }

    pub fn move_selection(&mut self, delta: i32) {
        let n = self.items_in(self.focused_pane).len() as i32;
        if n == 0 {
            return;
        }
        let cur = self.selection[self.focused_pane] as i32;
        let next = (cur + delta).rem_euclid(n);
        self.selection[self.focused_pane] = next as usize;
    }

    pub fn switch_pane(&mut self, delta: i32) {
        let next = (self.focused_pane as i32 + delta).rem_euclid(3) as usize;
        self.focused_pane = next;
    }

    pub fn set_pane(&mut self, index: usize) {
        if index < 3 {
            self.focused_pane = index;
        }
    }

    /// Called on the 30s tick (and after any state-changing action).
    pub fn refresh_buckets(&mut self) {
        self.library.recompute_panes(self.now, self.windows);
        // Clamp selections to current pane sizes.
        for i in 0..3 {
            let n = self.items_in(i).len();
            if self.selection[i] >= n {
                self.selection[i] = n.saturating_sub(1);
            }
        }
    }
}
