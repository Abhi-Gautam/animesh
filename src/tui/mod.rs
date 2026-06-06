//! Interactive ratatui-based TUI shell — SP-1.5.
//!
//! Sync event loop: poll crossterm events with a 100ms timeout; render
//! every iteration. A 30s tick re-renders relative-time labels and the
//! wall-clock, and re-derives pane buckets.
//!
//! See `docs/superpowers/specs/2026-06-06-sp1.5-interactive-tui.md`.

pub mod app;
pub mod help;
pub mod model;
pub mod palette;
pub mod pane;
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

use crate::store::{resolve_db_path, Db};
use crate::tui::app::{App, Overlay};
use crate::tui::model::Library;
use crate::tui::pane::Windows;

const POLL_TIMEOUT: Duration = Duration::from_millis(100);
const TICK_INTERVAL: Duration = Duration::from_secs(30);

/// Entry point invoked from `main()` when the user runs `animesh`
/// with no subcommand.
pub fn run() -> Result<()> {
    let path = resolve_db_path()?;
    let db = Db::open(&path)?;

    let windows = Windows::from_env();
    let now = Utc::now().timestamp();
    let library = Library::load(&db, now, windows)?;
    let app = App::new(db, library, windows, now);

    let mut terminal = setup_terminal().context("setup terminal")?;
    install_panic_hook();

    let result = event_loop(&mut terminal, app);

    // Restore even if the loop returned an error.
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

fn handle_key(app: &mut App, key: KeyEvent) {
    // Ctrl-C exits no matter what.
    if key.modifiers.contains(KeyModifiers::CONTROL) && key.code == KeyCode::Char('c') {
        app.quit = true;
        return;
    }
    match app.overlay {
        Overlay::None => handle_key_normal(app, key),
        Overlay::Palette => handle_key_palette(app, key),
        Overlay::Help => handle_key_help(app, key),
    }
}

fn handle_key_normal(app: &mut App, key: KeyEvent) {
    use KeyCode::*;
    match key.code {
        Char('q') => app.quit = true,
        Char('j') | Down => app.move_selection(1),
        Char('k') | Up => app.move_selection(-1),
        Tab => app.switch_pane(1),
        BackTab => app.switch_pane(-1),
        Char('l') | Right => app.switch_pane(1),
        Char('h') | Left => app.switch_pane(-1),
        Char('1') => app.set_pane(0),
        Char('2') => app.set_pane(1),
        Char('3') => app.set_pane(2),
        Char('?') => app.overlay = Overlay::Help,
        Char('a') | Char(':') | Char('/') => {
            app.overlay = Overlay::Palette;
            app.palette.query.clear();
            app.palette.selected = 0;
        }
        Char('w') => action_watched(app),
        Char('s') => action_snooze(app),
        Char('d') => action_drop(app),
        Char('g') => action_stream(app),
        _ => {}
    }
}

fn handle_key_palette(app: &mut App, key: KeyEvent) {
    match key.code {
        KeyCode::Esc => app.overlay = Overlay::None,
        KeyCode::Enter => {
            // T40 wires this; for now just close.
            app.overlay = Overlay::None;
        }
        KeyCode::Backspace => {
            app.palette.query.pop();
        }
        KeyCode::Char(c) => app.palette.query.push(c),
        _ => {}
    }
}

fn handle_key_help(app: &mut App, key: KeyEvent) {
    if matches!(key.code, KeyCode::Esc | KeyCode::Char('?') | KeyCode::Enter) {
        app.overlay = Overlay::None;
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

/// Install a panic hook that restores the terminal before printing
/// the panic. Without this, a panic mid-runloop leaves the user with
/// a hosed terminal.
fn action_watched(app: &mut App) {
    let Some(s) = app.current() else { return };
    let source = s.item.source.clone();
    let source_id = s.item.source_id.clone();
    let title = s.display_title().to_string();
    let total = s.total();
    let now = Utc::now().timestamp();
    match app.db.increment_watch(&source, &source_id, total, now) {
        Ok(seen) => {
            app.library.set_progress(&source, &source_id, seen, now);
            app.now = now;
            app.refresh_buckets();
            app.toasts
                .push(format!("✓ Marked {title} — episode {seen} watched"));
        }
        Err(e) => app.toasts.push(format!("error: {e}")),
    }
}

fn action_snooze(app: &mut App) {
    if let Some(s) = app.current() {
        app.toasts
            .push(format!("▷ Snoozed {} to tomorrow (stub)", s.display_title()));
    }
}

fn action_drop(app: &mut App) {
    let Some(s) = app.current() else { return };
    let source = s.item.source.clone();
    let source_id = s.item.source_id.clone();
    let title = s.display_title().to_string();
    let now = Utc::now().timestamp();
    match app.db.drop_follow(&source, &source_id, now) {
        Ok(true) => {
            app.library.shows.retain(|sh| {
                !(sh.item.source == source && sh.item.source_id == source_id)
            });
            app.refresh_buckets();
            app.toasts
                .push(format!("✗ Dropped {title} — undo with `animesh follow --id`"));
        }
        Ok(false) => app.toasts.push("nothing to drop"),
        Err(e) => app.toasts.push(format!("error: {e}")),
    }
}

fn action_stream(app: &mut App) {
    let Some(s) = app.current() else { return };
    let title = s.display_title().to_string();
    let url = s
        .streaming
        .iter()
        .find_map(|l| l.url.clone())
        .or_else(|| s.item.user_note.clone());
    let Some(url) = url else {
        app.toasts.push(format!(
            "no streaming link cached for {title} — try `animesh sync`"
        ));
        return;
    };
    match open::that(&url) {
        Ok(_) => app
            .toasts
            .push(format!("↗ Opening {title} — {url}")),
        Err(e) => app.toasts.push(format!("open failed: {e}")),
    }
}

fn install_panic_hook() {
    let original = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        let _ = disable_raw_mode();
        let _ = execute!(io::stdout(), LeaveAlternateScreen, DisableMouseCapture);
        original(info);
    }));
}
