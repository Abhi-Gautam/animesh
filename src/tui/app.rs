//! Top-level App state — owns the model, drives the event loop.
//!
//! All user-invocable verbs flow through `App::dispatch(Command)`. The
//! keymap (`w`, `s`, `d`, `g`, `q`, `?`) and the `:` palette both call
//! it, so they can never drift apart. Async verbs (`:sync`, `:follow`)
//! `block_in_place` on the current tokio runtime — main constructs a
//! multi-thread runtime so this is sound.

use anyhow::Result;
use chrono::Utc;
use tokio::runtime::Handle;

use crate::anilist::AniListClient;
use crate::commands::follow::follow_inner;
use crate::commands::sync::sync_inner;
use crate::store::{Db, FollowOutcome};
use crate::tui::command::Command;
use crate::tui::model::Library;
use crate::tui::palette::{FollowStage, PaletteMode, PaletteState};
use crate::tui::pane::{Pane, Windows};
use crate::tui::toast::ToastQueue;

/// Which overlay (if any) is intercepting input.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Overlay {
    None,
    /// `:` — verb palette.
    Command,
    /// `/` — fuzzy jump to a followed show.
    Search,
    /// `a` — AniList picker to follow a new show.
    Follow,
    /// `?` — keymap reference.
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
    pub client: AniListClient,
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
    pub fn new(db: Db, client: AniListClient, library: Library, windows: Windows, now: i64) -> Self {
        Self {
            db,
            client,
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

    pub fn focused_index(&self) -> usize {
        self.focused_pane
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
        for i in 0..3 {
            let n = self.items_in(i).len();
            if self.selection[i] >= n {
                self.selection[i] = n.saturating_sub(1);
            }
        }
    }

    /// Library is empty → render the onboarding empty state.
    pub fn is_first_run(&self) -> bool {
        self.library.shows.is_empty()
    }

    /// Single dispatch entry point. Pressing `w` calls
    /// `dispatch(Command::Watched)`; typing `:watched` and pressing
    /// Enter calls the same. Tests drive `dispatch` directly without
    /// touching the terminal.
    pub fn dispatch(&mut self, cmd: Command) {
        match cmd {
            Command::Watched => self.do_watched(),
            Command::Snooze => self.do_snooze(),
            Command::Drop => self.do_drop(),
            Command::Stream => self.do_stream(),
            Command::Help => {
                self.overlay = Overlay::Help;
            }
            Command::Quit => {
                self.quit = true;
            }
            Command::Follow(id) => self.do_follow(id),
            Command::Sync => self.do_sync(),
            Command::Doctor => self.do_doctor(),
        }
    }

    fn do_watched(&mut self) {
        let Some(s) = self.current() else {
            self.toasts.push("nothing selected");
            return;
        };
        let source = s.item.source.clone();
        let source_id = s.item.source_id.clone();
        let title = s.display_title().to_string();
        let total = s.total();
        let now = Utc::now().timestamp();
        match self.db.increment_watch(&source, &source_id, total, now) {
            Ok(seen) => {
                self.library.set_progress(&source, &source_id, seen, now);
                self.now = now;
                self.refresh_buckets();
                self.toasts
                    .push(format!("✓ Marked {title} — episode {seen} watched"));
            }
            Err(e) => self.toasts.push(format!("error: {e}")),
        }
    }

    fn do_snooze(&mut self) {
        if let Some(s) = self.current() {
            self.toasts
                .push(format!("▷ Snoozed {} to tomorrow (stub)", s.display_title()));
        } else {
            self.toasts.push("nothing selected");
        }
    }

    fn do_drop(&mut self) {
        let Some(s) = self.current() else {
            self.toasts.push("nothing selected");
            return;
        };
        let source = s.item.source.clone();
        let source_id = s.item.source_id.clone();
        let title = s.display_title().to_string();
        let now = Utc::now().timestamp();
        match self.db.drop_follow(&source, &source_id, now) {
            Ok(true) => {
                self.library
                    .shows
                    .retain(|sh| !(sh.item.source == source && sh.item.source_id == source_id));
                self.refresh_buckets();
                self.toasts
                    .push(format!("✗ Dropped {title} — undo with `:follow {source_id}`"));
            }
            Ok(false) => self.toasts.push("nothing to drop"),
            Err(e) => self.toasts.push(format!("error: {e}")),
        }
    }

    fn do_stream(&mut self) {
        let Some(s) = self.current() else {
            self.toasts.push("nothing selected");
            return;
        };
        let title = s.display_title().to_string();
        let url = s
            .streaming
            .iter()
            .find_map(|l| l.url.clone())
            .or_else(|| s.item.user_note.clone());
        let Some(url) = url else {
            self.toasts.push(format!(
                "no streaming link cached for {title} — try `:sync`"
            ));
            return;
        };
        match open::that(&url) {
            Ok(_) => self.toasts.push(format!("↗ Opening {title} — {url}")),
            Err(e) => self.toasts.push(format!("open failed: {e}")),
        }
    }

    fn do_follow(&mut self, id: i64) {
        let now = Utc::now().timestamp();
        let result = tokio::task::block_in_place(|| {
            Handle::current().block_on(follow_inner(&mut self.db, &self.client, id, now))
        });
        match result {
            Ok(report) => {
                let title = report.media.display_title().to_string();
                let msg = match report.outcome {
                    FollowOutcome::NewlyFollowed => format!("✓ Followed {title}"),
                    FollowOutcome::RestoredFromDrop => format!("↻ Restored {title}"),
                    FollowOutcome::AlreadyFollowing => format!("already following {title}"),
                };
                self.toasts.push(msg);
                self.reload_library(now);
            }
            Err(e) => self.toasts.push(format!("follow failed: {e}")),
        }
    }

    fn do_sync(&mut self) {
        let now = Utc::now().timestamp();
        let result = tokio::task::block_in_place(|| {
            Handle::current().block_on(sync_inner(&mut self.db, &self.client, now))
        });
        match result {
            Ok(report) => {
                let msg = if report.failures.is_empty() {
                    format!("✓ Synced {}/{}", report.succeeded, report.total)
                } else {
                    format!(
                        "synced {}/{}, {} failed",
                        report.succeeded,
                        report.total,
                        report.failures.len()
                    )
                };
                self.toasts.push(msg);
                self.reload_library(now);
            }
            Err(e) => self.toasts.push(format!("sync failed: {e}")),
        }
    }

    fn do_doctor(&mut self) {
        match self.db.count_active() {
            Ok(n) => self.toasts.push(format!("following {n} shows")),
            Err(e) => self.toasts.push(format!("doctor: {e}")),
        }
    }

    /// Reload the library from disk after a write that touched it
    /// (follow/sync). Re-derives panes.
    fn reload_library(&mut self, now: i64) {
        if let Ok(lib) = Library::load(&self.db, now, self.windows) {
            self.library = lib;
            self.now = now;
            self.refresh_buckets();
        }
    }

    // ---------- Palette helpers ----------

    /// Open a palette overlay in the given mode.
    pub fn open_palette(&mut self, mode: PaletteMode) {
        self.palette.open(mode);
        self.overlay = match mode {
            PaletteMode::Command => Overlay::Command,
            PaletteMode::Search => Overlay::Search,
            PaletteMode::Follow => Overlay::Follow,
        };
    }

    pub fn close_overlay(&mut self) {
        self.overlay = Overlay::None;
    }

    /// Recompute `palette.search_hits` from the current query. Called
    /// on every keystroke in Search mode.
    pub fn recompute_search_hits(&mut self) {
        use nucleo::{pattern::{CaseMatching, Normalization, Pattern}, Config, Matcher};
        let q = self.palette.query.trim();
        self.palette.search_hits.clear();
        if q.is_empty() {
            self.palette.search_hits = (0..self.library.shows.len()).collect();
            self.palette.selected = 0;
            return;
        }
        let mut matcher = Matcher::new(Config::DEFAULT);
        let pattern = Pattern::parse(q, CaseMatching::Ignore, Normalization::Smart);
        let mut scored: Vec<(usize, u32)> = self
            .library
            .shows
            .iter()
            .enumerate()
            .filter_map(|(i, s)| {
                let title = s.display_title().to_string();
                let mut buf = Vec::new();
                let haystack = nucleo::Utf32Str::new(&title, &mut buf);
                pattern.score(haystack, &mut matcher).map(|sc| (i, sc))
            })
            .collect();
        scored.sort_by(|a, b| b.1.cmp(&a.1));
        self.palette.search_hits = scored.into_iter().map(|(i, _)| i).collect();
        if self.palette.selected >= self.palette.search_hits.len() {
            self.palette.selected = 0;
        }
    }

    /// Jump the focused pane's cursor to the show at `library_idx`.
    /// Switches to whichever pane the show lives in.
    pub fn jump_to(&mut self, library_idx: usize) -> Result<()> {
        let show = self
            .library
            .shows
            .get(library_idx)
            .ok_or_else(|| anyhow::anyhow!("show out of range"))?;
        let pane_idx = match show.pane {
            Some(Pane::Today) => PANE_TODAY,
            Some(Pane::Late) => PANE_LATE,
            Some(Pane::Backlog { .. }) => PANE_BACKLOG,
            None => return Err(anyhow::anyhow!("show is hidden (fully watched)")),
        };
        self.focused_pane = pane_idx;
        let items = self.items_in(pane_idx);
        let target = (show.item.source.as_str(), show.item.source_id.as_str());
        if let Some(pos) = items
            .iter()
            .position(|s| (s.item.source.as_str(), s.item.source_id.as_str()) == target)
        {
            self.selection[pane_idx] = pos;
        }
        Ok(())
    }

    /// Run AniList search for the current Follow-mode query. Blocking.
    pub fn run_follow_search(&mut self) {
        let q = self.palette.query.trim().to_string();
        if q.is_empty() {
            self.palette.follow_error = Some("type a query first".into());
            return;
        }
        self.palette.follow_error = None;
        self.palette.follow_stage = FollowStage::Searching;
        let result = tokio::task::block_in_place(|| {
            Handle::current().block_on(self.client.search(&q, 10))
        });
        match result {
            Ok(hits) if hits.is_empty() => {
                self.palette.follow_error = Some("no matches on AniList".into());
                self.palette.follow_stage = FollowStage::AwaitingQuery;
            }
            Ok(hits) => {
                self.palette.follow_hits = hits;
                self.palette.selected = 0;
                self.palette.follow_stage = FollowStage::Picking;
            }
            Err(e) => {
                self.palette.follow_error = Some(format!("AniList: {e}"));
                self.palette.follow_stage = FollowStage::AwaitingQuery;
            }
        }
    }

    /// Follow the AniList id from the currently selected Follow-mode hit.
    pub fn confirm_follow(&mut self) {
        let Some(media) = self.palette.follow_hits.get(self.palette.selected).cloned() else {
            return;
        };
        let id = media.id;
        self.close_overlay();
        self.dispatch(Command::Follow(id));
    }

}
