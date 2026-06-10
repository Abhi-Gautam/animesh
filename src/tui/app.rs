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

use crate::commands::sync::sync_inner_default;
use crate::ingest::follow::FollowIngestService;
use crate::ingest::service::IngestSearchService;
use crate::library::Library as Facade;
use crate::sources::SourceRegistry;
use crate::store::{CanonicalFollowOutcome, EngagementEvent, EngagementMeta};
use crate::tui::command::Command;
use crate::tui::model::Shelf;
use crate::tui::palette::{FollowStage, PaletteMode, PaletteState};
use crate::tui::pane::{Pane, Windows};
use crate::tui::theme::{
    normalize_theme_id, Theme, ThemePickerState, ThemeRegistry, DEFAULT_THEME_ID, KV_UI_THEME,
};
use crate::tui::toast::ToastQueue;

/// Which overlay (if any) is intercepting input.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Overlay {
    None,
    /// `:` — verb palette.
    Command,
    /// `/` — fuzzy jump to a followed show.
    Search,
    /// `a` — AniList picker to follow a new show.
    Follow,
    /// `t` / `:theme` — theme picker with live preview.
    Theme,
    /// `?` — keymap reference.
    Help,
}

/// Which pane is focused. `0/1/2` map to Playable / Dropping / Following
/// so number keys (`1` `2` `3`) trivially map to indices.
pub(crate) const PANE_PLAYABLE: usize = 0;
pub(crate) const PANE_DROPPING: usize = 1;
pub(crate) const PANE_FOLLOWING: usize = 2;
pub(crate) const PANE_LABELS: [&str; 3] = ["PLAYABLE", "DROPPING", "FOLLOWING"];
pub(crate) const PANE_KINDS: [Pane; 3] = [Pane::Playable, Pane::Dropping, Pane::Following];

pub(crate) struct App {
    pub facade: Arc<Facade>,
    pub sources: SourceRegistry,
    pub shelf: Shelf,
    pub focused_pane: usize,
    /// Per-pane cursor; remembered across pane switches.
    pub selection: [usize; 3],
    pub overlay: Overlay,
    pub palette: PaletteState,
    pub theme_registry: ThemeRegistry,
    pub active_theme_id: String,
    pub theme_picker: ThemePickerState,
    pub toasts: ToastQueue,
    pub windows: Windows,
    pub subs: crate::tui::subs::Subs,
    pub now: i64,
    /// Set to true to exit the run loop.
    pub quit: bool,
}

impl App {
    pub(crate) fn new(
        facade: Arc<Facade>,
        sources: SourceRegistry,
        shelf: Shelf,
        windows: Windows,
        subs: crate::tui::subs::Subs,
        now: i64,
    ) -> Self {
        let theme_registry = ThemeRegistry::builtin();
        let stored_theme = facade
            .kv_get(KV_UI_THEME)
            .ok()
            .flatten()
            .unwrap_or_else(|| DEFAULT_THEME_ID.to_string());
        let active_theme_id = theme_registry
            .get(&stored_theme)
            .map(|theme| theme.id.to_string())
            .unwrap_or_else(|| DEFAULT_THEME_ID.to_string());

        Self {
            facade,
            sources,
            shelf,
            focused_pane: PANE_PLAYABLE,
            selection: [0; 3],
            overlay: Overlay::None,
            palette: PaletteState::default(),
            theme_registry,
            active_theme_id,
            theme_picker: ThemePickerState::default(),
            toasts: ToastQueue::default(),
            windows,
            subs,
            now,
            quit: false,
        }
    }

    pub(crate) fn focused_index(&self) -> usize {
        self.focused_pane
    }

    pub(crate) fn items_in(&self, pane: usize) -> Vec<&crate::tui::model::Show> {
        let pane_kind = PANE_KINDS[pane];
        self.shelf
            .shows
            .iter()
            .filter(move |s| {
                matches!(
                    (pane_kind, s.pane),
                    (Pane::Playable, Some(Pane::Playable))
                        | (Pane::Dropping, Some(Pane::Dropping))
                        | (Pane::Following, Some(Pane::Following))
                )
            })
            .collect()
    }

    pub(crate) fn current(&self) -> Option<&crate::tui::model::Show> {
        let pane = self.focused_pane;
        let items = self.items_in(pane);
        let idx = self.selection[pane].min(items.len().saturating_sub(1));
        items.get(idx).copied()
    }

    pub(crate) fn move_selection(&mut self, delta: i32) {
        let n = self.items_in(self.focused_pane).len() as i32;
        if n == 0 {
            return;
        }
        let cur = self.selection[self.focused_pane] as i32;
        let next = (cur + delta).rem_euclid(n);
        self.selection[self.focused_pane] = next as usize;
    }

    pub(crate) fn switch_pane(&mut self, delta: i32) {
        let next = (self.focused_pane as i32 + delta).rem_euclid(3) as usize;
        self.focused_pane = next;
    }

    pub(crate) fn set_pane(&mut self, index: usize) {
        if index < 3 {
            self.focused_pane = index;
        }
    }

    /// Called on the 30s tick (and after any state-changing action).
    pub(crate) fn refresh_buckets(&mut self) {
        self.shelf
            .recompute_panes(self.now, self.windows, &self.subs);
        for i in 0..3 {
            let n = self.items_in(i).len();
            if self.selection[i] >= n {
                self.selection[i] = n.saturating_sub(1);
            }
        }
    }

    /// Shelf is empty → render the onboarding empty state.
    pub(crate) fn is_first_run(&self) -> bool {
        self.shelf.shows.is_empty()
    }

    /// Single dispatch entry point. Pressing `w` calls
    /// `dispatch(Command::Watched)`; typing `:watched` and pressing
    /// Enter calls the same. Tests drive `dispatch` directly without
    /// touching the terminal.
    pub(crate) fn dispatch(&mut self, cmd: Command) {
        match cmd {
            Command::Watched => self.do_watched(),
            Command::Drop => self.do_drop(),
            Command::Stream => self.do_stream(),
            Command::CopyContext => self.do_copy_context(),
            Command::SubsAdd(name) => self.do_subs_add(&name),
            Command::SubsRemove(name) => self.do_subs_remove(&name),
            Command::SubsList => self.do_subs_list(),
            Command::Theme(theme_id) => self.do_theme(theme_id.as_deref()),
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
        match self.facade.engage(
            &canonical_id,
            EngagementEvent::Completed,
            Some(EngagementMeta::Completed { seen: new_seen }),
        ) {
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
                self.toasts.push(format!(
                    "✗ Dropped {title} — undo with `:follow {source_id}`"
                ));
            }
            Ok(false) => self.toasts.push("nothing to drop"),
            Err(e) => self.toasts.push(format!("error: {e}")),
        }
    }

    fn do_stream(&mut self) {
        // Extract owned data so the &Show borrow ends before we call
        // open_url (which needs &mut self).
        let (title, verified_url, verified_streamer, streaming) = {
            let Some(s) = self.current() else {
                self.toasts.push("nothing selected");
                return;
            };
            (
                s.display_title().to_string(),
                s.verified_url().map(str::to_string),
                s.verified_streamer().map(str::to_string),
                s.streaming.clone(),
            )
        };

        // Prefer verified URL on a subscribed streamer.
        if let (Some(url), Some(streamer)) = (verified_url, verified_streamer) {
            if self.subs.matches(&streamer) {
                return self.open_url(&title, &url, None);
            }
        }
        // Fall back: any cached streaming link whose site is subscribed.
        let preferred = streaming.iter().find_map(|l| {
            let site = l.site.as_deref()?;
            let url = l.url.as_deref()?;
            if self.subs.matches(site) {
                Some(url.to_string())
            } else {
                None
            }
        });
        if let Some(url) = preferred {
            return self.open_url(&title, &url, None);
        }
        // Last resort: first link with a URL, but warn it isn't subscribed.
        if let Some((url, site)) = streaming.iter().find_map(|l| {
            let url = l.url.clone()?;
            Some((url, l.site.clone().unwrap_or_else(|| "unknown".into())))
        }) {
            return self.open_url(
                &title,
                &url,
                Some(format!("opens on {site} — not in your subs")),
            );
        }
        self.toasts.push(format!(
            "no streaming link cached for {title} — try `:sync`"
        ));
    }

    fn open_url(&mut self, title: &str, url: &str, warn: Option<String>) {
        match open::that(url) {
            Ok(_) => {
                let msg = match warn {
                    Some(w) => format!("⚠ {w} · ↗ {title}"),
                    None => format!("↗ Opening {title} — {url}"),
                };
                self.toasts.push(msg);
            }
            Err(e) => self.toasts.push(format!("open failed: {e}")),
        }
    }

    fn do_copy_context(&mut self) {
        let Some(s) = self.current() else {
            self.toasts.push("nothing selected");
            return;
        };
        let title = s.display_title().to_string();
        let value = match crate::tui::llm_context::build(&self.facade, s) {
            Ok(v) => v,
            Err(e) => {
                self.toasts.push(format!("context build failed: {e}"));
                return;
            }
        };
        let pretty = serde_json::to_string_pretty(&value).unwrap_or_default();
        let bytes = pretty.len();
        match arboard::Clipboard::new().and_then(|mut c| c.set_text(pretty)) {
            Ok(_) => self.toasts.push(format!(
                "⧉ context for \"{title}\" copied ({:.1} KB)",
                bytes as f64 / 1024.0
            )),
            Err(e) => self.toasts.push(format!("clipboard error: {e}")),
        }
    }

    fn do_subs_add(&mut self, name: &str) {
        let lib = self.facade.clone();
        match self.subs.add(&lib, name) {
            Ok(true) => {
                self.toasts.push(format!("✓ subscribed to {name}"));
                self.refresh_buckets();
            }
            Ok(false) => self.toasts.push(format!("already subscribed to {name}")),
            Err(e) => self.toasts.push(format!("subs: {e}")),
        }
    }

    fn do_subs_remove(&mut self, name: &str) {
        let lib = self.facade.clone();
        match self.subs.remove(&lib, name) {
            Ok(true) => {
                self.toasts.push(format!("✗ removed {name}"));
                self.refresh_buckets();
            }
            Ok(false) => self.toasts.push(format!("not subscribed to {name}")),
            Err(e) => self.toasts.push(format!("subs: {e}")),
        }
    }

    fn do_subs_list(&mut self) {
        let s = self.subs.streamers();
        if s.is_empty() {
            self.toasts.push("no subs — `:subs add netflix` to start");
        } else {
            self.toasts.push(format!("subs › {}", s.join(" · ")));
        }
    }

    fn do_theme(&mut self, theme_id: Option<&str>) {
        match theme_id {
            None => self.open_theme_picker(),
            Some(id) => self.apply_theme_id(id, true),
        }
    }

    fn do_follow(&mut self, _id: i64) {
        self.toasts
            .push("direct :follow id is retired — press `a`, search candidates, then Enter");
    }

    fn do_sync(&mut self) {
        let now = Utc::now().timestamp();
        let result = tokio::task::block_in_place(|| {
            Handle::current().block_on(sync_inner_default(&self.facade, now))
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
        if let Ok(shelf) = Shelf::load(&self.facade, now, self.windows, &self.subs) {
            self.shelf = shelf;
            self.now = now;
            self.refresh_buckets();
        }
    }

    // ---------- Palette helpers ----------

    /// Open a palette overlay in the given mode.
    pub(crate) fn open_palette(&mut self, mode: PaletteMode) {
        self.palette.open(mode);
        self.overlay = match mode {
            PaletteMode::Command => Overlay::Command,
            PaletteMode::Search => Overlay::Search,
            PaletteMode::Follow => Overlay::Follow,
        };
    }

    pub(crate) fn theme(&self) -> &Theme {
        self.theme_registry
            .get(&self.active_theme_id)
            .unwrap_or_else(|| self.theme_registry.default_theme())
    }

    pub(crate) fn visible_theme(&self) -> &Theme {
        if self.overlay == Overlay::Theme {
            if let Some(id) = &self.theme_picker.preview_theme_id {
                if let Some(theme) = self.theme_registry.get(id) {
                    return theme;
                }
            }
        }
        self.theme()
    }

    pub(crate) fn open_theme_picker(&mut self) {
        let selected = self.theme_registry.index_of(&self.active_theme_id);
        self.theme_picker.open(&self.active_theme_id, selected);
        self.sync_theme_preview();
        self.overlay = Overlay::Theme;
    }

    pub(crate) fn move_theme_selection(&mut self, delta: i32) {
        let len = self.theme_registry.all().len();
        self.theme_picker.move_selection(delta, len);
        self.sync_theme_preview();
    }

    pub(crate) fn confirm_theme_picker(&mut self) {
        let Some(id) = self.theme_picker.preview_theme_id.clone() else {
            self.close_overlay();
            return;
        };
        self.apply_theme_id(&id, true);
        self.theme_picker.close();
        self.overlay = Overlay::None;
    }

    pub(crate) fn cancel_theme_picker(&mut self) {
        self.active_theme_id = self.theme_picker.original_theme_id.clone();
        self.theme_picker.close();
        self.overlay = Overlay::None;
    }

    fn sync_theme_preview(&mut self) {
        self.theme_picker.preview_theme_id = self
            .theme_registry
            .all()
            .get(self.theme_picker.selected)
            .map(|theme| theme.id.to_string());
    }

    fn apply_theme_id(&mut self, id: &str, persist: bool) {
        let normalized = normalize_theme_id(id);
        let Some(theme) = self.theme_registry.get(normalized) else {
            self.toasts.push(format!("unknown theme: {id}"));
            return;
        };
        let theme_id = theme.id.to_string();
        let theme_name = theme.name;
        self.active_theme_id = theme_id.clone();
        self.theme_picker.preview_theme_id = Some(theme_id.clone());
        if persist {
            match self.facade.kv_set(KV_UI_THEME, &theme_id) {
                Ok(()) => self.toasts.push(format!("theme › {theme_name}")),
                Err(e) => self
                    .toasts
                    .push(format!("theme saved for session only: {e}")),
            }
        }
    }

    pub(crate) fn close_overlay(&mut self) {
        self.overlay = Overlay::None;
    }

    /// Recompute `palette.search_hits` from the current query. Called
    /// on every keystroke in Search mode.
    pub(crate) fn recompute_search_hits(&mut self) {
        use nucleo::{
            pattern::{CaseMatching, Normalization, Pattern},
            Config, Matcher,
        };
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
    pub(crate) fn jump_to(&mut self, shelf_idx: usize) -> Result<()> {
        let show = self
            .shelf
            .shows
            .get(shelf_idx)
            .ok_or_else(|| anyhow::anyhow!("show out of range"))?;
        let pane_idx = match show.pane {
            Some(Pane::Playable) => PANE_PLAYABLE,
            Some(Pane::Dropping) => PANE_DROPPING,
            Some(Pane::Following) => PANE_FOLLOWING,
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

    /// Recompute follow candidates from local `source_candidate_fts` only.
    /// Called on every keystroke in Follow mode; no network happens here.
    pub(crate) fn recompute_follow_hits(&mut self) {
        let q = self.palette.query.trim();
        self.palette.follow_hits.clear();
        if q.is_empty() {
            self.palette.selected = 0;
            self.palette.follow_stage = FollowStage::AwaitingQuery;
            return;
        }
        match self.facade.search_source_candidates(q, 10) {
            Ok(hits) => {
                self.palette.follow_hits = hits;
                self.palette.selected = 0;
                self.palette.follow_stage = if self.palette.follow_hits.is_empty() {
                    FollowStage::AwaitingQuery
                } else {
                    FollowStage::Picking
                };
            }
            Err(e) => {
                self.palette.follow_error = Some(format!("local candidate search: {e}"));
                self.palette.follow_stage = FollowStage::AwaitingQuery;
            }
        }
    }

    /// Go online for the current Follow-mode query, ingest fresh source
    /// candidates, then show local candidate search results.
    pub(crate) fn run_follow_search(&mut self) {
        let q = self.palette.query.trim().to_string();
        if q.is_empty() {
            self.palette.follow_error = Some("type a query first".into());
            return;
        }
        self.palette.follow_error = None;
        self.palette.follow_stage = FollowStage::Searching;
        let now = Utc::now().timestamp();
        let result = tokio::task::block_in_place(|| {
            let service = IngestSearchService::new(&self.facade, &self.sources);
            Handle::current().block_on(service.refresh_candidates(&q, 10, now))
        });
        match result {
            Ok(hits) if hits.is_empty() => {
                self.palette.follow_hits.clear();
                self.palette.follow_error = Some("no source candidates found".into());
                self.palette.follow_stage = FollowStage::AwaitingQuery;
            }
            Ok(hits) => {
                self.palette.follow_hits = hits;
                self.palette.selected = 0;
                self.palette.follow_stage = FollowStage::Picking;
            }
            Err(e) => {
                self.palette.follow_error = Some(format!("source search: {e}"));
                self.palette.follow_stage = FollowStage::AwaitingQuery;
            }
        }
    }

    /// Follow the currently selected source candidate.
    pub(crate) fn confirm_follow(&mut self) {
        let Some(candidate) = self.palette.follow_hits.get(self.palette.selected).cloned() else {
            return;
        };
        let now = Utc::now().timestamp();
        self.close_overlay();
        let result = match Handle::try_current() {
            Ok(handle) => tokio::task::block_in_place(|| {
                let service = FollowIngestService::new(&self.facade, &self.sources);
                handle.block_on(service.follow_and_ingest(&candidate, now))
            }),
            Err(_) => {
                let rt = tokio::runtime::Runtime::new().expect("create follow ingest runtime");
                let service = FollowIngestService::new(&self.facade, &self.sources);
                rt.block_on(service.follow_and_ingest(&candidate, now))
            }
        };
        match result {
            Ok(report) => {
                let title = report.candidate.display_title;
                let mut msg = match report.outcome {
                    CanonicalFollowOutcome::NewlyFollowed => format!("✓ Followed {title}"),
                    CanonicalFollowOutcome::RestoredFromDrop => format!("↻ Restored {title}"),
                    CanonicalFollowOutcome::AlreadyFollowing => {
                        format!("already following {title}")
                    }
                    CanonicalFollowOutcome::NotFound => {
                        format!("follow failed: canonical missing for {title}")
                    }
                };
                if report.detail_ingested {
                    if report.projected_events > 0 {
                        msg.push_str(&format!(" · ingested {} events", report.projected_events));
                    } else {
                        msg.push_str(" · no schedule events found");
                    }
                } else if let Some(warning) = report.warning {
                    msg.push_str(&format!(" · detail ingest failed, will retry ({warning})"));
                }
                self.toasts.push(msg);
                self.reload_shelf(now);
            }
            Err(e) => self.toasts.push(format!("follow failed: {e}")),
        }
    }
}
