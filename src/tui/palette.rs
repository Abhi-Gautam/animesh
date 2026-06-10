//! Overlay state for the three palette modes.
//!
//! Three modes, three Enter behaviors (nvim-style):
//! - `Command` (`:`)  → parse query as a verb, dispatch through `App::dispatch`
//! - `Search`  (`/`)  → fuzzy over followed titles, jump cursor on Enter
//! - `Follow`  (`a`)  → query AniList, Enter follows the highlighted result
//!
//! The `mode` discriminator lets one render path handle all three with
//! different headers, candidates, and Enter wiring.

use crate::search::source_candidate::SourceCandidateResult;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PaletteMode {
    Command,
    Search,
    Follow,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FollowStage {
    /// User types a query; Enter triggers a network search.
    AwaitingQuery,
    /// Network search returned at least one result.
    Picking,
    /// Network search is mid-flight (transient — we run blocking, so
    /// the loop won't actually render this. Reserved for future async).
    Searching,
}

#[derive(Debug, Clone)]
pub struct PaletteState {
    pub mode: PaletteMode,
    pub query: String,
    pub selected: usize,
    /// Search mode: indices into `library.shows` matching the query,
    /// ranked by nucleo. Recomputed on every keystroke.
    pub search_hits: Vec<usize>,
    /// Follow mode: local source candidates ranked by search.
    pub follow_hits: Vec<SourceCandidateResult>,
    pub follow_stage: FollowStage,
    /// Follow mode: last error message, surfaced inline so the user
    /// doesn't have to close the overlay to see why search failed.
    pub follow_error: Option<String>,
}

impl Default for PaletteState {
    fn default() -> Self {
        Self {
            mode: PaletteMode::Command,
            query: String::new(),
            selected: 0,
            search_hits: Vec::new(),
            follow_hits: Vec::new(),
            follow_stage: FollowStage::AwaitingQuery,
            follow_error: None,
        }
    }
}

impl PaletteState {
    /// Reset to a fresh overlay in the given mode.
    pub fn open(&mut self, mode: PaletteMode) {
        self.mode = mode;
        self.query.clear();
        self.selected = 0;
        self.search_hits.clear();
        self.follow_hits.clear();
        self.follow_stage = FollowStage::AwaitingQuery;
        self.follow_error = None;
    }

    /// Move selection by delta within `len` candidates, wrapping.
    pub fn move_selection(&mut self, delta: i32, len: usize) {
        if len == 0 {
            self.selected = 0;
            return;
        }
        let n = len as i32;
        let cur = self.selected as i32;
        let next = (cur + delta).rem_euclid(n);
        self.selected = next as usize;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn open_resets_all_fields() {
        let mut p = PaletteState {
            query: "stale".into(),
            selected: 5,
            search_hits: vec![1, 2, 3],
            follow_error: Some("boom".into()),
            ..PaletteState::default()
        };
        p.open(PaletteMode::Search);
        assert_eq!(p.mode, PaletteMode::Search);
        assert!(p.query.is_empty());
        assert_eq!(p.selected, 0);
        assert!(p.search_hits.is_empty());
        assert!(p.follow_error.is_none());
    }

    #[test]
    fn move_selection_wraps() {
        let mut p = PaletteState::default();
        p.move_selection(1, 3);
        assert_eq!(p.selected, 1);
        p.move_selection(5, 3); // wrap to (1+5)%3 = 0
        assert_eq!(p.selected, 0);
        p.move_selection(-1, 3); // wrap to 2
        assert_eq!(p.selected, 2);
    }

    #[test]
    fn move_selection_with_zero_candidates_is_safe() {
        let mut p = PaletteState {
            selected: 4,
            ..PaletteState::default()
        };
        p.move_selection(1, 0);
        assert_eq!(p.selected, 0);
    }
}
