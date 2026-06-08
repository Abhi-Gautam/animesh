//! Top-level App state — owns the model, drives the event loop.
//!
//! All user-invocable verbs flow through `App::dispatch(Command)`. The
//! keymap (`w`, `s`, `d`, `g`, `q`, `?`) and the `:` palette both call
//! it, so they can never drift apart. Async verbs (`:sync`, `:follow`)
//! `block_in_place` on the current tokio runtime — main constructs a
//! multi-thread runtime so this is sound.
//!
//! v0.5: the App holds an `Arc<Facade>` (the durable [`Library`])
//! and no direct `Db`. Every read/write goes through Library; the
//! TUI never touches SQL.

use std::sync::Arc;

use anyhow::Result;
use chrono::Utc;
use tokio::runtime::Handle;

use crate::commands::follow::follow_inner;
use crate::commands::sync::sync_inner;
use crate::library::Library as Facade;
use crate::sources::anilist::AniListClient;
use crate::store::{CanonicalFollowOutcome, EngagementEvent};
use crate::tui::command::Command;
use crate::tui::model::Shelf;
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
    pub facade: Arc<Facade>,
    pub client: AniListClient,
    pub shelf: Shelf,
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
    pub fn new(
        facade: Arc<Facade>,
        client: AniListClient,
        shelf: Shelf,
        windows: Windows,
        now: i64,
    ) -> Self {
        Self {
            facade,
            client,
            shelf,
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
        self.shelf
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
        self.shelf.recompute_panes(self.now, self.windows);
        for i in 0..3 {
            let n = self.items_in(i).len();
            if self.selection[i] >= n {
                self.selection[i] = n.saturating_sub(1);
            }
        }
    }

    /// Shelf is empty → render the onboarding empty state.
    pub fn is_first_run(&self) -> bool {
        self.shelf.shows.is_empty()
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
        let canonical_id = s.canonical_id().clone();
        let title = s.display_title().to_string();
        let total = s.total();
        let prev_seen = s.seen();
        let now = Utc::now().timestamp();
        let new_seen = match total {
            Some(t) if prev_seen + 1 > t => t,
            _ => prev_seen + 1,
        };
        let meta = format!("{{\"seen\":{new_seen}}}");
        match self
            .facade
            .engage(&canonical_id, EngagementEvent::Completed, Some(&meta))
        {
            Ok(()) => {
                self.shelf.set_progress(&canonical_id, new_seen, now);
                self.now = now;
                self.refresh_buckets();
                self.toasts
                    .push(format!("✓ Marked {title} — episode {new_seen} watched"));
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
        let canonical_id = s.canonical_id().clone();
        let title = s.display_title().to_string();
        let source_id = s.source_id().to_string();
        match self.facade.drop_canonical(&canonical_id) {
            Ok(true) => {
                self.shelf
                    .shows
                    .retain(|sh| sh.canonical.id != canonical_id);
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
            .or_else(|| s.user_note().map(str::to_string));
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
            Handle::current().block_on(follow_inner(&self.facade, &self.client, id, now))
        });
        match result {
            Ok(report) => {
                let title = report.media.display_title().to_string();
                let msg = match report.outcome {
                    CanonicalFollowOutcome::NewlyFollowed => format!("✓ Followed {title}"),
                    CanonicalFollowOutcome::RestoredFromDrop => format!("↻ Restored {title}"),
                    CanonicalFollowOutcome::AlreadyFollowing => {
                        format!("already following {title}")
                    }
                    // follow_with_source upserts the canonical first, so
                    // NotFound should be impossible here. Surface it as
                    // a clear error rather than silently swallowing.
                    CanonicalFollowOutcome::NotFound => {
                        format!("follow failed: canonical missing for {title}")
                    }
                };
                self.toasts.push(msg);
                self.reload_shelf(now);
            }
            Err(e) => self.toasts.push(format!("follow failed: {e}")),
        }
    }

    fn do_sync(&mut self) {
        let now = Utc::now().timestamp();
        let result = tokio::task::block_in_place(|| {
            Handle::current().block_on(sync_inner(&self.facade, &self.client, now))
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
                self.reload_shelf(now);
            }
            Err(e) => self.toasts.push(format!("sync failed: {e}")),
        }
    }

    fn do_doctor(&mut self) {
        match self.facade.count_followed() {
            Ok(n) => self.toasts.push(format!("following {n} shows")),
            Err(e) => self.toasts.push(format!("doctor: {e}")),
        }
    }

    /// Reload the shelf from durable state after a write that touched
    /// it (follow/sync). Re-derives panes.
    fn reload_shelf(&mut self, now: i64) {
        if let Ok(shelf) = Shelf::load(&self.facade, now, self.windows) {
            self.shelf = shelf;
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
            self.palette.search_hits = (0..self.shelf.shows.len()).collect();
            self.palette.selected = 0;
            return;
        }
        let mut matcher = Matcher::new(Config::DEFAULT);
        let pattern = Pattern::parse(q, CaseMatching::Ignore, Normalization::Smart);
        let mut scored: Vec<(usize, u32)> = self
            .shelf
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

    /// Jump the focused pane's cursor to the show at `shelf_idx`.
    /// Switches to whichever pane the show lives in.
    pub fn jump_to(&mut self, shelf_idx: usize) -> Result<()> {
        let show = self
            .shelf
            .shows
            .get(shelf_idx)
            .ok_or_else(|| anyhow::anyhow!("show out of range"))?;
        let pane_idx = match show.pane {
            Some(Pane::Today) => PANE_TODAY,
            Some(Pane::Late) => PANE_LATE,
            Some(Pane::Backlog { .. }) => PANE_BACKLOG,
            None => return Err(anyhow::anyhow!("show is hidden (fully watched)")),
        };
        self.focused_pane = pane_idx;
        let items = self.items_in(pane_idx);
        let target = show.canonical_id().clone();
        if let Some(pos) = items.iter().position(|s| *s.canonical_id() == target) {
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
