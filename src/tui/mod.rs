//! Interactive ratatui-based TUI shell — SP-1.5 + SP-1.6.
//!
//! Sync event loop: poll crossterm events with a 100ms timeout; render
//! every iteration. A 30s tick re-renders relative-time labels and the
//! wall-clock, and re-derives pane buckets.
//!
//! SP-1.6 (this revision):
//! - `:` / `/` / `a` open three distinct overlays (vim/lazygit model).
//! - Pressing `w` and typing `:watched` both go through `App::dispatch`.
//! - Empty library renders an onboarding welcome instead of empty panes.
//!
//! See `docs/superpowers/specs/2026-06-06-sp1.5-interactive-tui.md` for
//! the original substrate; the SP-1.6 onboarding work is doc-less by
//! request — see `docs/QA.md` for the manual verification protocol.

pub mod app;
pub mod ascii_art;
pub mod command;
pub mod help;
pub mod llm_context;
pub mod model;
pub mod palette;
pub mod pane;
pub mod subs;
pub mod toast;
pub mod view;
pub mod view_detail;

use std::io::{self, Stdout};
use std::time::{Duration, Instant};

use anyhow::{Context, Result};
use chrono::Utc;
use crossterm::{
    event::{self, DisableMouseCapture, EnableMouseCapture, Event, KeyCode, KeyEvent, KeyModifiers},
    execute,
    terminal::{disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen},
};
use ratatui::{backend::CrosstermBackend, Terminal};

use std::sync::Arc;

use crate::library::Library as Facade;
use crate::sources::anilist::AniListClient;
use crate::store::resolve_db_path;
use crate::time::SystemClock;
use crate::tui::app::{App, Overlay};
use crate::tui::command::Command;
use crate::tui::model::Shelf;
use crate::tui::palette::{FollowStage, PaletteMode};
use crate::tui::pane::Windows;

const POLL_TIMEOUT: Duration = Duration::from_millis(100);
const TICK_INTERVAL: Duration = Duration::from_secs(30);

/// Entry point invoked from `main()` when the user runs `animesh`
/// with no subcommand.
pub fn run() -> Result<()> {
    let path = resolve_db_path()?;
    let client = AniListClient::new();
    let facade = Arc::new(Facade::open(&path, Arc::new(SystemClock))?);

    // Backfill cover art for any follow with a missing or stale render.
    // Blocks startup briefly (≈300ms × N stale rows) but only fires on
    // the first launch after a renderer change.
    tokio::task::block_in_place(|| {
        tokio::runtime::Handle::current().block_on(
            crate::commands::follow::refresh_stale_covers(&facade, &client),
        )
    });

    let windows = Windows::from_env();
    let now = Utc::now().timestamp();
    let shelf = Shelf::load(&facade, now, windows)?;
    let app = App::new(facade, client, shelf, windows, now);

    let mut terminal = setup_terminal().context("setup terminal")?;
    install_panic_hook();

    let result = event_loop(&mut terminal, app);

    restore_terminal(&mut terminal).ok();
    result
}

fn event_loop(
    terminal: &mut Terminal<CrosstermBackend<Stdout>>,
    mut app: App,
) -> Result<()> {
    let mut last_tick = Instant::now();
    loop {
        terminal.draw(|f| view::render(f, &app))?;
        if event::poll(POLL_TIMEOUT)? {
            if let Event::Key(key) = event::read()? {
                handle_key(&mut app, key);
                if app.quit {
                    break;
                }
            }
        }
        if last_tick.elapsed() >= TICK_INTERVAL {
            app.now = Utc::now().timestamp();
            app.refresh_buckets();
            last_tick = Instant::now();
        }
    }
    Ok(())
}

/// Pure key-dispatch — exposed for integration tests.
pub fn handle_key(app: &mut App, key: KeyEvent) {
    // Ctrl-C exits no matter what.
    if key.modifiers.contains(KeyModifiers::CONTROL) && key.code == KeyCode::Char('c') {
        app.quit = true;
        return;
    }
    match app.overlay {
        Overlay::None => handle_key_normal(app, key),
        Overlay::Command => handle_key_command(app, key),
        Overlay::Search => handle_key_search(app, key),
        Overlay::Follow => handle_key_follow(app, key),
        Overlay::Help => handle_key_help(app, key),
    }
}

fn handle_key_normal(app: &mut App, key: KeyEvent) {
    use KeyCode::*;
    // Onboarding fast path: empty library has its own minimal keymap.
    if app.is_first_run() {
        match key.code {
            Char('a') => app.open_palette(PaletteMode::Follow),
            Char(':') => app.open_palette(PaletteMode::Command),
            Char('?') => app.dispatch(Command::Help),
            Char('q') => app.dispatch(Command::Quit),
            _ => {}
        }
        return;
    }
    match key.code {
        Char('q') => app.dispatch(Command::Quit),
        Char('j') | Down => app.move_selection(1),
        Char('k') | Up => app.move_selection(-1),
        Tab => app.switch_pane(1),
        BackTab => app.switch_pane(-1),
        Char('l') | Right => app.switch_pane(1),
        Char('h') | Left => app.switch_pane(-1),
        Char('1') => app.set_pane(0),
        Char('2') => app.set_pane(1),
        Char('3') => app.set_pane(2),
        Char('?') => app.dispatch(Command::Help),
        Char(':') => app.open_palette(PaletteMode::Command),
        Char('/') => {
            app.open_palette(PaletteMode::Search);
            app.recompute_search_hits();
        }
        Char('a') => app.open_palette(PaletteMode::Follow),
        Char('w') => app.dispatch(Command::Watched),
        Char('s') => app.dispatch(Command::Snooze),
        Char('d') => app.dispatch(Command::Drop),
        Char('g') => app.dispatch(Command::Stream),
        _ => {}
    }
}

fn handle_key_command(app: &mut App, key: KeyEvent) {
    let suggestions = crate::tui::command::suggest(&app.palette.query);
    match key.code {
        KeyCode::Esc => app.close_overlay(),
        KeyCode::Enter => {
            // If there's a clean suggestion list with a selection, run
            // that. Otherwise parse the raw query so users who type the
            // full verb don't need to press Tab first.
            let chosen = suggestions.get(app.palette.selected).map(|s| s.spec.name);
            let to_parse = match chosen {
                Some(name) if app.palette.query.trim().is_empty() => name.to_string(),
                _ => app.palette.query.clone(),
            };
            match crate::tui::command::parse(&to_parse) {
                Ok(cmd) => {
                    app.close_overlay();
                    app.dispatch(cmd);
                }
                Err(e) => app.toasts.push(format!("{e}")),
            }
        }
        KeyCode::Tab => {
            if let Some(top) = suggestions.first() {
                app.palette.query = top.spec.name.to_string();
                app.palette.selected = 0;
            }
        }
        KeyCode::Down => app.palette.move_selection(1, suggestions.len()),
        KeyCode::Up => app.palette.move_selection(-1, suggestions.len()),
        KeyCode::Backspace => {
            app.palette.query.pop();
            app.palette.selected = 0;
        }
        KeyCode::Char(c) => {
            app.palette.query.push(c);
            app.palette.selected = 0;
        }
        _ => {}
    }
}

fn handle_key_search(app: &mut App, key: KeyEvent) {
    match key.code {
        KeyCode::Esc => app.close_overlay(),
        KeyCode::Enter => {
            if let Some(&library_idx) = app.palette.search_hits.get(app.palette.selected) {
                let _ = app.jump_to(library_idx);
            }
            app.close_overlay();
        }
        KeyCode::Down => {
            let len = app.palette.search_hits.len();
            app.palette.move_selection(1, len);
        }
        KeyCode::Up => {
            let len = app.palette.search_hits.len();
            app.palette.move_selection(-1, len);
        }
        KeyCode::Backspace => {
            app.palette.query.pop();
            app.recompute_search_hits();
        }
        KeyCode::Char(c) => {
            app.palette.query.push(c);
            app.recompute_search_hits();
        }
        _ => {}
    }
}

fn handle_key_follow(app: &mut App, key: KeyEvent) {
    match app.palette.follow_stage {
        FollowStage::AwaitingQuery | FollowStage::Searching => match key.code {
            KeyCode::Esc => app.close_overlay(),
            KeyCode::Enter => app.run_follow_search(),
            KeyCode::Backspace => {
                app.palette.query.pop();
                app.palette.follow_error = None;
            }
            KeyCode::Char(c) => {
                app.palette.query.push(c);
                app.palette.follow_error = None;
            }
            _ => {}
        },
        FollowStage::Picking => match key.code {
            KeyCode::Esc => app.close_overlay(),
            KeyCode::Enter => app.confirm_follow(),
            KeyCode::Down | KeyCode::Char('j') => {
                let len = app.palette.follow_hits.len();
                app.palette.move_selection(1, len);
            }
            KeyCode::Up | KeyCode::Char('k') => {
                let len = app.palette.follow_hits.len();
                app.palette.move_selection(-1, len);
            }
            // Any other char resets to a new query (escape-hatch UX).
            KeyCode::Char(_) => {
                app.palette.follow_stage = FollowStage::AwaitingQuery;
                app.palette.follow_hits.clear();
            }
            _ => {}
        },
    }
}

fn handle_key_help(app: &mut App, key: KeyEvent) {
    if matches!(key.code, KeyCode::Esc | KeyCode::Char('?') | KeyCode::Enter | KeyCode::Char('q')) {
        app.close_overlay();
    }
}

fn setup_terminal() -> Result<Terminal<CrosstermBackend<Stdout>>> {
    enable_raw_mode().context("enable raw mode")?;
    let mut stdout = io::stdout();
    execute!(stdout, EnterAlternateScreen, EnableMouseCapture)?;
    let backend = CrosstermBackend::new(stdout);
    Terminal::new(backend).context("create terminal")
}

fn restore_terminal(terminal: &mut Terminal<CrosstermBackend<Stdout>>) -> Result<()> {
    disable_raw_mode().ok();
    execute!(
        terminal.backend_mut(),
        LeaveAlternateScreen,
        DisableMouseCapture
    )
    .ok();
    terminal.show_cursor().ok();
    Ok(())
}

#[cfg(test)]
mod integration_tests;

/// Install a panic hook that restores the terminal before printing
/// the panic. Without this, a panic mid-runloop leaves the user with
/// a hosed terminal.
fn install_panic_hook() {
    let original = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        let _ = disable_raw_mode();
        let _ = execute!(io::stdout(), LeaveAlternateScreen, DisableMouseCapture);
        original(info);
    }));
}
