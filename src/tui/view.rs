//! Frame composition. The single entry point `render(frame, app)` is
//! called from the run loop on every iteration. v0.4 is text-first;
//! T38 fills in the detail pane and T41 adds the overlays.

use chrono::{Local, TimeZone};
use ratatui::{
    layout::{Alignment, Constraint, Direction, Layout, Rect},
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, BorderType, Borders, Paragraph, Wrap},
    Frame,
};

use crate::tui::app::{App, Overlay, PANE_LABELS};
use crate::tui::help::HELP_PAIRS;
use crate::tui::pane::Pane;

const ACCENT: Color = Color::Rgb(0xff, 0x7a, 0x59);
const ACCENT_DIM: Color = Color::Rgb(0xa8, 0x50, 0x3a);
const INK: Color = Color::Rgb(0xcd, 0xd6, 0xe2);
const INK_2: Color = Color::Rgb(0x9a, 0xa6, 0xb6);
const DIM: Color = Color::Rgb(0x58, 0x60, 0x71);
const DIMMER: Color = Color::Rgb(0x3a, 0x43, 0x50);
const SOON: Color = Color::Rgb(0x61, 0xaf, 0xef);
const LATE: Color = Color::Rgb(0xe0, 0x6c, 0x75);
const TIME_GREEN: Color = Color::Rgb(0x98, 0xc3, 0x79);
const EP_GOLD: Color = Color::Rgb(0xe5, 0xc0, 0x7b);

pub fn render(f: &mut Frame, app: &App) {
    let area = f.area();
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(1), // status line
            Constraint::Min(0),    // body
            Constraint::Length(1), // command line
            Constraint::Length(1), // status bar
        ])
        .split(area);

    render_status_line(f, app, chunks[0]);
    render_body(f, app, chunks[1]);
    render_cmdline(f, app, chunks[2]);
    render_status_bar(f, app, chunks[3]);

    match app.overlay {
        Overlay::None => {}
        Overlay::Palette => render_palette(f, app, area),
        Overlay::Help => render_help(f, area),
    }

    if let Some(text) = app.toasts.visible() {
        render_toast(f, area, text);
    }
}

fn render_status_line(f: &mut Frame, app: &App, area: Rect) {
    let active = app.library.shows.len();
    let late = app.items_in(crate::tui::app::PANE_LATE).len();
    let clock = Local
        .timestamp_opt(app.now, 0)
        .single()
        .map(|t| t.format("%H:%M").to_string())
        .unwrap_or_else(|| "--:--".into());
    let left = vec![
        Span::styled("~", Style::default().fg(ACCENT).add_modifier(Modifier::BOLD)),
        Span::styled("animesh", Style::default().fg(INK).add_modifier(Modifier::BOLD)),
        Span::raw("   "),
        Span::styled("panes", Style::default().fg(INK_2)),
        Span::raw(" › "),
        Span::styled(PANE_LABELS[app.focused_index()].to_lowercase(), Style::default().fg(DIM)),
    ];
    let right = vec![
        Span::styled(format!("{active} followed"), Style::default().fg(DIM)),
        Span::raw(" · "),
        Span::styled(
            format!("{late} late"),
            Style::default().fg(if late > 0 { LATE } else { DIM }),
        ),
        Span::raw(" · "),
        Span::styled(clock, Style::default().fg(INK_2)),
    ];
    let chunks = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([Constraint::Percentage(50), Constraint::Percentage(50)])
        .split(area);
    f.render_widget(Paragraph::new(Line::from(left)), chunks[0]);
    f.render_widget(
        Paragraph::new(Line::from(right)).alignment(Alignment::Right),
        chunks[1],
    );
}

fn render_body(f: &mut Frame, app: &App, area: Rect) {
    let chunks = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([Constraint::Percentage(34), Constraint::Min(0)])
        .split(area);
    render_panels(f, app, chunks[0]);
    crate::tui::view_detail::render(f, app, chunks[1]);
}

fn render_panels(f: &mut Frame, app: &App, area: Rect) {
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Percentage(33),
            Constraint::Percentage(33),
            Constraint::Min(0),
        ])
        .split(area);
    for (pane_idx, &chunk) in chunks.iter().enumerate() {
        render_panel(f, app, pane_idx, chunk);
    }
}

fn render_panel(f: &mut Frame, app: &App, pane_idx: usize, area: Rect) {
    let focused = app.focused_index() == pane_idx;
    let items = app.items_in(pane_idx);
    let header_color = if focused { ACCENT } else { DIM };
    let title = format!(
        " {}   {}   {} ",
        pane_idx + 1,
        PANE_LABELS[pane_idx],
        items.len()
    );
    let block = Block::default()
        .borders(Borders::LEFT)
        .border_type(BorderType::Plain)
        .border_style(Style::default().fg(if focused { ACCENT } else { Color::Reset }))
        .title(Span::styled(
            title,
            Style::default().fg(header_color).add_modifier(Modifier::BOLD),
        ));
    let inner = block.inner(area);
    f.render_widget(block, area);

    let sel = app.selection[pane_idx];
    let lines: Vec<Line> = items
        .iter()
        .enumerate()
        .map(|(i, s)| render_row(s, i == sel, focused, pane_idx, app.now))
        .collect();
    let para = Paragraph::new(lines);
    f.render_widget(para, inner);
}

fn render_row(
    s: &crate::tui::model::Show,
    selected: bool,
    pane_focused: bool,
    pane_idx: usize,
    now: i64,
) -> Line<'static> {
    let mark_color = if selected && pane_focused { ACCENT } else { DIMMER };
    let mark = Span::styled("▸ ", Style::default().fg(mark_color));
    let title_color = if selected && pane_focused {
        Color::White
    } else if selected {
        INK_2
    } else {
        INK_2
    };
    let title_style = if selected && pane_focused {
        Style::default().fg(title_color).add_modifier(Modifier::BOLD)
    } else {
        Style::default().fg(title_color)
    };
    let title = Span::styled(s.display_title().to_string(), title_style);

    let (badge, badge_color) = match s.pane {
        Some(Pane::Today) => {
            let rel = relative_short(s.airs_at().unwrap_or(now), now);
            let ep = s.next_episode().unwrap_or(0);
            (format!("E{ep} · {rel}"), SOON)
        }
        Some(Pane::Late) => {
            let ep = s.next_episode().unwrap_or(0);
            (format!("E{ep} ✓?"), LATE)
        }
        Some(Pane::Backlog { behind }) => {
            if behind > 0 {
                (format!("+{behind}"), LATE)
            } else {
                ("1 left".to_string(), SOON)
            }
        }
        None => (String::new(), DIM),
    };
    let _ = pane_idx;

    Line::from(vec![
        mark,
        title,
        Span::raw("  "),
        Span::styled(badge, Style::default().fg(badge_color)),
    ])
}

fn relative_short(at: i64, now: i64) -> String {
    let diff = at - now;
    let a = diff.abs();
    let mins = (a / 60).max(1);
    let hours = a / 3600;
    let days = a / 86400;
    let body = if days >= 1 {
        format!("{days}d")
    } else if hours >= 1 {
        format!("{hours}h")
    } else {
        format!("{mins}m")
    };
    if diff < 0 {
        format!("{body} ago")
    } else {
        format!("in {body}")
    }
}

fn render_cmdline(f: &mut Frame, app: &App, area: Rect) {
    let line = match app.overlay {
        Overlay::Palette => Line::from(vec![
            Span::styled("❯ ", Style::default().fg(ACCENT).add_modifier(Modifier::BOLD)),
            Span::styled(&app.palette.query, Style::default().fg(INK)),
            Span::styled("▏", Style::default().fg(INK)),
        ]),
        _ => Line::from(vec![
            Span::styled(": ", Style::default().fg(ACCENT).add_modifier(Modifier::BOLD)),
            Span::styled(
                "press a / : for the command palette — add, watched, snooze, drop, where-to-watch…",
                Style::default().fg(DIMMER),
            ),
        ]),
    };
    f.render_widget(Paragraph::new(line), area);
}

fn render_status_bar(f: &mut Frame, _app: &App, area: Rect) {
    let pairs: [(&str, &str); 6] = [
        ("j/k", "move"),
        ("tab", "pane"),
        ("w", "watched"),
        ("g", "stream"),
        (":", "cmd"),
        ("?", "help"),
    ];
    let mut spans = Vec::with_capacity(pairs.len() * 4);
    for (key, label) in pairs {
        spans.push(Span::styled(key, Style::default().fg(ACCENT).add_modifier(Modifier::BOLD)));
        spans.push(Span::raw(" "));
        spans.push(Span::styled(label, Style::default().fg(DIM)));
        spans.push(Span::raw("  "));
    }
    let line = Line::from(spans);
    let chunks = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([Constraint::Min(0), Constraint::Length(10)])
        .split(area);
    f.render_widget(Paragraph::new(line), chunks[0]);
    let mode = Line::from(vec![Span::styled(
        " NORMAL ",
        Style::default()
            .bg(ACCENT)
            .fg(Color::Rgb(0x0a, 0x0a, 0x0a))
            .add_modifier(Modifier::BOLD),
    )]);
    f.render_widget(
        Paragraph::new(mode).alignment(Alignment::Right),
        chunks[1],
    );
}

fn render_palette(f: &mut Frame, app: &App, area: Rect) {
    let w = area.width.saturating_sub(20).min(70);
    let h = 16.min(area.height.saturating_sub(4));
    let x = (area.width - w) / 2;
    let y = area.height.saturating_sub(area.height.saturating_sub(2)).max(4);
    let rect = Rect::new(x, y, w, h);
    let block = Block::default()
        .borders(Borders::ALL)
        .border_type(BorderType::Rounded)
        .border_style(Style::default().fg(ACCENT))
        .title(Span::styled(" command palette ", Style::default().fg(ACCENT)));
    let inner = block.inner(rect);
    f.render_widget(block, rect);
    let lines = vec![
        Line::from(vec![
            Span::styled("❯ ", Style::default().fg(ACCENT).add_modifier(Modifier::BOLD)),
            Span::styled(&app.palette.query, Style::default().fg(INK)),
        ]),
        Line::raw(""),
        Line::from(Span::styled(
            "type to fuzzy-search verbs and your followed shows · Enter to run · Esc to close",
            Style::default().fg(DIM),
        )),
    ];
    f.render_widget(Paragraph::new(lines).wrap(Wrap { trim: false }), inner);
}

fn render_help(f: &mut Frame, area: Rect) {
    let w = (area.width - 8).min(72);
    let h = ((HELP_PAIRS.len() + 4) as u16).min(area.height.saturating_sub(4));
    let x = (area.width - w) / 2;
    let y = (area.height.saturating_sub(h)) / 2;
    let rect = Rect::new(x, y, w, h);
    let block = Block::default()
        .borders(Borders::ALL)
        .border_type(BorderType::Rounded)
        .border_style(Style::default().fg(DIM))
        .title(Span::styled(
            " keymap ",
            Style::default().fg(INK).add_modifier(Modifier::BOLD),
        ));
    let inner = block.inner(rect);
    f.render_widget(block, rect);
    let lines: Vec<Line> = HELP_PAIRS
        .iter()
        .map(|(k, label)| {
            Line::from(vec![
                Span::styled(
                    format!("  {k:<16}"),
                    Style::default().fg(ACCENT).add_modifier(Modifier::BOLD),
                ),
                Span::styled((*label).to_string(), Style::default().fg(INK_2)),
            ])
        })
        .collect();
    f.render_widget(Paragraph::new(lines), inner);
}

fn render_toast(f: &mut Frame, area: Rect, text: &str) {
    let w = (text.len() as u16 + 6).min(area.width.saturating_sub(4));
    let x = (area.width.saturating_sub(w)) / 2;
    let y = area.height.saturating_sub(5);
    let rect = Rect::new(x, y, w, 3);
    let block = Block::default()
        .borders(Borders::ALL)
        .border_type(BorderType::Plain)
        .border_style(Style::default().fg(ACCENT));
    let inner = block.inner(rect);
    f.render_widget(block, rect);
    f.render_widget(
        Paragraph::new(Line::from(Span::styled(
            text.to_string(),
            Style::default().fg(INK),
        ))),
        inner,
    );
}

// Re-export for tests that want to peek into colors.
pub(crate) const _DESIGN_TOKENS: &[Color] = &[
    ACCENT, ACCENT_DIM, INK, DIM, SOON, LATE, TIME_GREEN, EP_GOLD,
];
