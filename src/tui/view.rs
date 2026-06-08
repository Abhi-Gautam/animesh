//! Frame composition. The single entry point `render(frame, app)` is
//! called from the run loop on every iteration.

use chrono::{Local, TimeZone};
use ratatui::{
    layout::{Alignment, Constraint, Direction, Layout, Rect},
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, BorderType, Borders, Clear, Paragraph, Wrap},
    Frame,
};

use crate::tui::app::{App, Overlay, PANE_LABELS};
use crate::tui::command::{self, Suggestion};
use crate::tui::help::HELP_PAIRS;
use crate::tui::palette::FollowStage;

const ACCENT: Color = Color::Rgb(0xff, 0x7a, 0x59);
const INK: Color = Color::Rgb(0xcd, 0xd6, 0xe2);
const INK_2: Color = Color::Rgb(0x9a, 0xa6, 0xb6);
const DIM: Color = Color::Rgb(0x58, 0x60, 0x71);
const DIMMER: Color = Color::Rgb(0x3a, 0x43, 0x50);
const SOON: Color = Color::Rgb(0x61, 0xaf, 0xef);
const LATE: Color = Color::Rgb(0xe0, 0x6c, 0x75);
/// Opaque background for overlay cards. Without this, ratatui blocks
/// only paint borders — the panes beneath bleed through.
const PANEL_BG: Color = Color::Rgb(0x14, 0x18, 0x1f);

pub fn render(f: &mut Frame, app: &App) {
    let area = f.area();
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(1), // status line
            Constraint::Min(0),    // body (or empty state)
            Constraint::Length(1), // command line
            Constraint::Length(1), // hint bar
        ])
        .split(area);

    render_status_line(f, app, chunks[0]);
    if app.is_first_run() {
        render_empty_state(f, chunks[1]);
    } else {
        render_body(f, app, chunks[1]);
    }
    render_cmdline(f, app, chunks[2]);
    render_hint_bar(f, app, chunks[3]);

    match app.overlay {
        Overlay::None => {}
        Overlay::Command => render_command_palette(f, app, area),
        Overlay::Search => render_search_palette(f, app, area),
        Overlay::Follow => render_follow_palette(f, app, area),
        Overlay::Help => render_help(f, area),
    }

    if let Some(text) = app.toasts.visible() {
        render_toast(f, area, text);
    }
}

fn render_status_line(f: &mut Frame, app: &App, area: Rect) {
    let active = app.shelf.shows.len();
    let playable = app
        .items_in(crate::tui::app::PANE_PLAYABLE)
        .len();
    let dropping = app
        .items_in(crate::tui::app::PANE_DROPPING)
        .len();
    let clock = Local
        .timestamp_opt(app.now, 0)
        .single()
        .map(|t| t.format("%H:%M").to_string())
        .unwrap_or_else(|| "--:--".into());
    let mut left = vec![
        Span::styled("~", Style::default().fg(ACCENT).add_modifier(Modifier::BOLD)),
        Span::styled("animesh", Style::default().fg(INK).add_modifier(Modifier::BOLD)),
        Span::raw("   "),
        Span::styled("panes", Style::default().fg(INK_2)),
        Span::raw(" › "),
        Span::styled(PANE_LABELS[app.focused_index()].to_lowercase(), Style::default().fg(DIM)),
    ];
    let subs_text = if app.subs.streamers().is_empty() {
        "(none — :subs add netflix)".to_string()
    } else {
        app.subs.streamers().join(" · ").to_lowercase()
    };
    left.extend(vec![
        Span::raw("   "),
        Span::styled("subs", Style::default().fg(INK_2)),
        Span::raw(" › "),
        Span::styled(subs_text, Style::default().fg(DIM)),
    ]);
    let right = vec![
        Span::styled(format!("{active} followed"), Style::default().fg(DIM)),
        Span::raw(" · "),
        Span::styled(
            format!("{playable} playable"),
            Style::default().fg(if playable > 0 { ACCENT } else { DIM }),
        ),
        Span::raw(" · "),
        Span::styled(format!("{dropping} dropping"), Style::default().fg(SOON)),
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

/// Onboarding takeover: rendered instead of the three panes whenever
/// `library.shows` is empty. Derived state — no flag to remember to set.
fn render_empty_state(f: &mut Frame, area: Rect) {
    let w = (area.width.saturating_sub(8)).min(72);
    let h = 14.min(area.height.saturating_sub(2));
    let x = area.x + (area.width.saturating_sub(w)) / 2;
    let y = area.y + (area.height.saturating_sub(h)) / 2;
    let rect = Rect::new(x, y, w, h);

    let block = Block::default()
        .borders(Borders::ALL)
        .border_type(BorderType::Rounded)
        .border_style(Style::default().fg(ACCENT))
        .title(Span::styled(
            " welcome to animesh ",
            Style::default().fg(ACCENT).add_modifier(Modifier::BOLD),
        ));
    let inner = block.inner(rect);
    f.render_widget(block, rect);

    let key = |k: &str| Span::styled(
        format!("  {k:<6}"),
        Style::default().fg(ACCENT).add_modifier(Modifier::BOLD),
    );
    let lines = vec![
        Line::raw(""),
        Line::from(Span::styled(
            "  Your library is empty. Three keys to get going:",
            Style::default().fg(INK),
        )),
        Line::raw(""),
        Line::from(vec![
            key("a"),
            Span::styled("Search AniList and follow your first show", Style::default().fg(INK_2)),
        ]),
        Line::from(vec![
            key(":"),
            Span::styled("Command mode — try :follow 21  (One Piece)", Style::default().fg(INK_2)),
        ]),
        Line::from(vec![
            key("?"),
            Span::styled("Full keymap. ", Style::default().fg(INK_2)),
            Span::styled("q quits.", Style::default().fg(DIM)),
        ]),
        Line::raw(""),
        Line::from(Span::styled(
            "  Once you've followed shows, panes split into",
            Style::default().fg(DIM),
        )),
        Line::from(Span::styled(
            "  Playable now  ·  Dropping soon  ·  Following. Press c for LLM context.",
            Style::default().fg(DIM),
        )),
    ];
    f.render_widget(Paragraph::new(lines).wrap(Wrap { trim: false }), inner);
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
    let _ = pane_idx;
    let mark_color = if selected && pane_focused { ACCENT } else { DIMMER };
    let mark = Span::styled("▸ ", Style::default().fg(mark_color));

    // Dim rows where we have a verified link but on no subscribed
    // streamer — they're catalogued but the user can't watch.
    let unreachable = s.last_verified.is_some() && !s.subscribed_match;
    let base = if unreachable { DIM } else { INK_2 };
    let title_color = if selected && pane_focused { Color::White } else { base };
    let title_style = if selected && pane_focused {
        Style::default().fg(title_color).add_modifier(Modifier::BOLD)
    } else {
        Style::default().fg(title_color)
    };
    let title = Span::styled(s.display_title().to_string(), title_style);

    let middle = kind_descriptor(s);
    let (glyph, glyph_text, glyph_color) = badge_for(s, now);
    let glyph_span = Span::styled(format!(" {glyph} "), Style::default().fg(glyph_color));
    let glyph_label = Span::styled(glyph_text, Style::default().fg(glyph_color));

    Line::from(vec![
        mark,
        title,
        Span::raw("  "),
        Span::styled(middle, Style::default().fg(DIM)),
        glyph_span,
        glyph_label,
    ])
}

/// Middle-column descriptor for a row. Kind-aware so anime/TV show
/// episode numbers, film shows "theatrical", music shows the cached
/// release format (album/single/EP).
fn kind_descriptor(s: &crate::tui::model::Show) -> String {
    use crate::ids::ReleaseKind::*;
    match s.canonical_id().kind() {
        Anime | Tv => {
            let ep = s.next_episode().unwrap_or(0);
            if ep == 0 {
                String::new()
            } else {
                format!("E{ep}")
            }
        }
        Film => "theatrical".to_string(),
        MusicArtist => s
            .format()
            .map(|f| f.to_lowercase())
            .unwrap_or_else(|| "release".to_string()),
    }
}

/// Trailing badge glyph + label + color.
///
/// Precedence:
/// 1. Verified + subscribed streamer → ▶ <streamer> in ACCENT.
/// 2. Future scheduled drop → 🛈 in SOON.
/// 3. Past scheduled drop (no verification) → ✗ in LATE.
/// 4. Otherwise → · (idle) in DIM.
fn badge_for(s: &crate::tui::model::Show, now: i64) -> (&'static str, String, Color) {
    if let (Some(at), Some(streamer)) = (s.verified_at(), s.verified_streamer()) {
        if s.subscribed_match {
            return ("▶", streamer.to_lowercase(), ACCENT);
        }
        let _ = at;
    }
    if let Some(drop_at) = s.next_drop_at() {
        if drop_at > now {
            return ("🛈", relative_short(drop_at, now), SOON);
        }
        return ("✗", relative_short(drop_at, now), LATE);
    }
    ("·", String::new(), DIM)
}

pub fn relative_short(at: i64, now: i64) -> String {
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
        Overlay::Command => Line::from(vec![
            Span::styled(":", Style::default().fg(ACCENT).add_modifier(Modifier::BOLD)),
            Span::styled(&app.palette.query, Style::default().fg(INK)),
            Span::styled("▏", Style::default().fg(INK)),
        ]),
        Overlay::Search => Line::from(vec![
            Span::styled("/", Style::default().fg(ACCENT).add_modifier(Modifier::BOLD)),
            Span::styled(&app.palette.query, Style::default().fg(INK)),
            Span::styled("▏", Style::default().fg(INK)),
        ]),
        Overlay::Follow => Line::from(vec![
            Span::styled(
                "a ",
                Style::default().fg(ACCENT).add_modifier(Modifier::BOLD),
            ),
            Span::styled(&app.palette.query, Style::default().fg(INK)),
            Span::styled("▏", Style::default().fg(INK)),
        ]),
        _ => Line::from(vec![
            Span::styled(": ", Style::default().fg(ACCENT).add_modifier(Modifier::BOLD)),
            Span::styled(
                "press : for commands  ·  / to jump  ·  a to follow a new show",
                Style::default().fg(DIMMER),
            ),
        ]),
    };
    f.render_widget(Paragraph::new(line), area);
}

/// Context-aware footer. Keys shown depend on which overlay is open.
fn render_hint_bar(f: &mut Frame, app: &App, area: Rect) {
    let (pairs, mode_label): (&[(&str, &str)], &str) = match app.overlay {
        Overlay::None => (
            &[
                ("j/k", "move"),
                ("tab", "pane"),
                ("w", "watched"),
                ("g", "stream"),
                ("c", "context"),
                (":", "cmd"),
                ("/", "jump"),
                ("a", "add"),
                ("?", "help"),
            ],
            " NORMAL ",
        ),
        Overlay::Command => (
            &[
                ("Enter", "run"),
                ("Tab", "complete"),
                ("↑↓", "select"),
                ("Esc", "cancel"),
            ],
            " COMMAND ",
        ),
        Overlay::Search => (
            &[("Enter", "jump"), ("↑↓", "select"), ("Esc", "cancel")],
            " SEARCH ",
        ),
        Overlay::Follow => match app.palette.follow_stage {
            FollowStage::Picking => (
                &[("Enter", "follow"), ("↑↓ / j k", "select"), ("Esc", "cancel")],
                " FOLLOW ",
            ),
            _ => (
                &[("Enter", "search AniList"), ("Esc", "cancel")],
                " FOLLOW ",
            ),
        },
        Overlay::Help => (&[("Esc / ?", "close")], " HELP "),
    };

    let mut spans = Vec::with_capacity(pairs.len() * 4);
    for (key, label) in pairs {
        spans.push(Span::styled(*key, Style::default().fg(ACCENT).add_modifier(Modifier::BOLD)));
        spans.push(Span::raw(" "));
        spans.push(Span::styled(*label, Style::default().fg(DIM)));
        spans.push(Span::raw("  "));
    }
    let chunks = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([Constraint::Min(0), Constraint::Length(10)])
        .split(area);
    f.render_widget(Paragraph::new(Line::from(spans)), chunks[0]);
    let mode = Line::from(vec![Span::styled(
        mode_label.to_string(),
        Style::default()
            .bg(ACCENT)
            .fg(Color::Rgb(0x0a, 0x0a, 0x0a))
            .add_modifier(Modifier::BOLD),
    )]);
    f.render_widget(Paragraph::new(mode).alignment(Alignment::Right), chunks[1]);
}

fn centered_rect(area: Rect, w: u16, h: u16) -> Rect {
    let w = w.min(area.width.saturating_sub(4));
    let h = h.min(area.height.saturating_sub(4));
    let x = area.x + (area.width.saturating_sub(w)) / 2;
    let y = area.y + (area.height.saturating_sub(h)) / 3;
    Rect::new(x, y, w, h)
}

fn render_command_palette(f: &mut Frame, app: &App, area: Rect) {
    let suggestions = command::suggest(&app.palette.query);
    let h = (4 + suggestions.len() as u16).min(18);
    let rect = centered_rect(area, 70, h);
    f.render_widget(Clear, rect);
    let block = Block::default()
        .borders(Borders::ALL)
        .border_type(BorderType::Rounded)
        .border_style(Style::default().fg(ACCENT))
        .title(Span::styled(
            " :command ",
            Style::default().fg(ACCENT).add_modifier(Modifier::BOLD),
        ))
        .style(Style::default().bg(PANEL_BG));
    let inner = block.inner(rect);
    f.render_widget(block, rect);

    let mut lines: Vec<Line> = vec![
        Line::from(vec![
            Span::styled(":", Style::default().fg(ACCENT).add_modifier(Modifier::BOLD)),
            Span::styled(&app.palette.query, Style::default().fg(INK)),
            Span::styled("▏", Style::default().fg(INK)),
        ]),
        Line::raw(""),
    ];
    for (i, s) in suggestions.iter().enumerate() {
        lines.push(render_suggestion(s, i == app.palette.selected));
    }
    f.render_widget(
        Paragraph::new(lines)
            .style(Style::default().bg(PANEL_BG))
            .wrap(Wrap { trim: false }),
        inner,
    );
}

fn render_suggestion(s: &Suggestion, selected: bool) -> Line<'static> {
    let arrow = Span::styled(
        if selected { "▸ " } else { "  " },
        Style::default().fg(if selected { ACCENT } else { DIMMER }),
    );
    let name_style = if selected {
        Style::default().fg(Color::White).add_modifier(Modifier::BOLD)
    } else {
        Style::default().fg(INK)
    };
    let name = Span::styled(format!(":{}", s.spec.name), name_style);
    let arg = match s.spec.arg_hint {
        Some(h) => Span::styled(format!(" {h}"), Style::default().fg(INK_2)),
        None => Span::raw(""),
    };
    let desc = Span::styled(
        format!("    {}", s.spec.description),
        Style::default().fg(DIM),
    );
    Line::from(vec![arrow, name, arg, desc])
}

fn render_search_palette(f: &mut Frame, app: &App, area: Rect) {
    let hits_n = app.palette.search_hits.len();
    let rect = centered_rect(area, 70, (4 + hits_n as u16).min(18));
    f.render_widget(Clear, rect);
    let block = Block::default()
        .borders(Borders::ALL)
        .border_type(BorderType::Rounded)
        .border_style(Style::default().fg(ACCENT))
        .title(Span::styled(
            " /jump ",
            Style::default().fg(ACCENT).add_modifier(Modifier::BOLD),
        ))
        .style(Style::default().bg(PANEL_BG));
    let inner = block.inner(rect);
    f.render_widget(block, rect);

    let mut lines: Vec<Line> = vec![
        Line::from(vec![
            Span::styled("/", Style::default().fg(ACCENT).add_modifier(Modifier::BOLD)),
            Span::styled(&app.palette.query, Style::default().fg(INK)),
            Span::styled("▏", Style::default().fg(INK)),
        ]),
        Line::raw(""),
    ];
    if hits_n == 0 {
        lines.push(Line::from(Span::styled(
            "  no followed shows match",
            Style::default().fg(DIM),
        )));
    } else {
        for (i, &idx) in app.palette.search_hits.iter().enumerate().take(12) {
            let s = &app.shelf.shows[idx];
            let selected = i == app.palette.selected;
            let arrow = Span::styled(
                if selected { "▸ " } else { "  " },
                Style::default().fg(if selected { ACCENT } else { DIMMER }),
            );
            let style = if selected {
                Style::default().fg(Color::White).add_modifier(Modifier::BOLD)
            } else {
                Style::default().fg(INK)
            };
            lines.push(Line::from(vec![
                arrow,
                Span::styled(s.display_title().to_string(), style),
            ]));
        }
    }
    f.render_widget(
        Paragraph::new(lines)
            .style(Style::default().bg(PANEL_BG))
            .wrap(Wrap { trim: false }),
        inner,
    );
}

fn render_follow_palette(f: &mut Frame, app: &App, area: Rect) {
    let hits_n = app.palette.follow_hits.len();
    let rect = centered_rect(area, 74, (6 + hits_n as u16).min(20));
    f.render_widget(Clear, rect);
    let block = Block::default()
        .borders(Borders::ALL)
        .border_type(BorderType::Rounded)
        .border_style(Style::default().fg(ACCENT))
        .title(Span::styled(
            " a · follow new show ",
            Style::default().fg(ACCENT).add_modifier(Modifier::BOLD),
        ))
        .style(Style::default().bg(PANEL_BG));
    let inner = block.inner(rect);
    f.render_widget(block, rect);

    let mut lines: Vec<Line> = vec![
        Line::from(vec![
            Span::styled(
                "search › ",
                Style::default().fg(ACCENT).add_modifier(Modifier::BOLD),
            ),
            Span::styled(&app.palette.query, Style::default().fg(INK)),
            Span::styled("▏", Style::default().fg(INK)),
        ]),
        Line::raw(""),
    ];
    if let Some(err) = &app.palette.follow_error {
        lines.push(Line::from(Span::styled(
            format!("  {err}"),
            Style::default().fg(LATE),
        )));
    }
    match app.palette.follow_stage {
        FollowStage::AwaitingQuery if hits_n == 0 => {
            lines.push(Line::from(Span::styled(
                "  type a title, press Enter to search AniList",
                Style::default().fg(DIM),
            )));
        }
        _ => {
            for (i, m) in app.palette.follow_hits.iter().enumerate().take(10) {
                let selected = i == app.palette.selected;
                let arrow = Span::styled(
                    if selected { "▸ " } else { "  " },
                    Style::default().fg(if selected { ACCENT } else { DIMMER }),
                );
                let style = if selected {
                    Style::default().fg(Color::White).add_modifier(Modifier::BOLD)
                } else {
                    Style::default().fg(INK)
                };
                let badge = Span::styled(
                    format!(
                        "  #{}  {}",
                        m.id,
                        m.status.as_deref().unwrap_or("")
                    ),
                    Style::default().fg(DIM),
                );
                lines.push(Line::from(vec![
                    arrow,
                    Span::styled(m.display_title().to_string(), style),
                    badge,
                ]));
            }
        }
    }
    f.render_widget(
        Paragraph::new(lines)
            .style(Style::default().bg(PANEL_BG))
            .wrap(Wrap { trim: false }),
        inner,
    );
}

fn render_help(f: &mut Frame, area: Rect) {
    let w = (area.width - 8).min(72);
    let h = ((HELP_PAIRS.len() + 4) as u16).min(area.height.saturating_sub(4));
    let x = (area.width - w) / 2;
    let y = (area.height.saturating_sub(h)) / 2;
    let rect = Rect::new(x, y, w, h);
    f.render_widget(Clear, rect);
    let block = Block::default()
        .borders(Borders::ALL)
        .border_type(BorderType::Rounded)
        .border_style(Style::default().fg(DIM))
        .title(Span::styled(
            " keymap ",
            Style::default().fg(INK).add_modifier(Modifier::BOLD),
        ))
        .style(Style::default().bg(PANEL_BG));
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
    f.render_widget(
        Paragraph::new(lines).style(Style::default().bg(PANEL_BG)),
        inner,
    );
}

fn render_toast(f: &mut Frame, area: Rect, text: &str) {
    let w = (text.len() as u16 + 6).min(area.width.saturating_sub(4));
    let x = (area.width.saturating_sub(w)) / 2;
    let y = area.height.saturating_sub(5);
    let rect = Rect::new(x, y, w, 3);
    f.render_widget(Clear, rect);
    let block = Block::default()
        .borders(Borders::ALL)
        .border_type(BorderType::Plain)
        .border_style(Style::default().fg(ACCENT))
        .style(Style::default().bg(PANEL_BG));
    let inner = block.inner(rect);
    f.render_widget(block, rect);
    f.render_widget(
        Paragraph::new(Line::from(Span::styled(
            text.to_string(),
            Style::default().fg(INK),
        )))
        .style(Style::default().bg(PANEL_BG)),
        inner,
    );
}
