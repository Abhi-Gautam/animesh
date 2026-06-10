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
use crate::tui::theme::Theme;

pub fn render(f: &mut Frame, app: &App) {
    let theme = app.visible_theme();
    let area = f.area();
    f.render_widget(Block::default().style(theme.styles.normal), area);

    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(1), // status line
            Constraint::Min(0),    // body (or empty state)
            Constraint::Length(1), // command line
            Constraint::Length(1), // hint bar
        ])
        .split(area);

    render_status_line(f, app, theme, chunks[0]);
    if app.is_first_run() {
        render_empty_state(f, theme, chunks[1]);
    } else {
        render_body(f, app, chunks[1]);
    }
    render_cmdline(f, app, theme, chunks[2]);
    render_hint_bar(f, app, theme, chunks[3]);

    match app.overlay {
        Overlay::None => {}
        Overlay::Command => render_command_palette(f, app, theme, area),
        Overlay::Search => render_search_palette(f, app, theme, area),
        Overlay::Follow => render_follow_palette(f, app, theme, area),
        Overlay::Theme => render_theme_picker(f, app, theme, area),
        Overlay::Help => render_help(f, theme, area),
    }

    if let Some(text) = app.toasts.visible() {
        render_toast(f, theme, area, text);
    }
}

fn render_status_line(f: &mut Frame, app: &App, theme: &Theme, area: Rect) {
    let active = app.shelf.shows.len();
    let playable = app.items_in(crate::tui::app::PANE_PLAYABLE).len();
    let dropping = app.items_in(crate::tui::app::PANE_DROPPING).len();
    let clock = Local
        .timestamp_opt(app.now, 0)
        .single()
        .map(|t| t.format("%H:%M").to_string())
        .unwrap_or_else(|| "--:--".into());
    let mut left = vec![
        Span::styled("~", theme.styles.key),
        Span::styled("animesh", theme.styles.title),
        Span::raw("   "),
        Span::styled("panes", theme.styles.muted),
        Span::raw(" › "),
        Span::styled(
            PANE_LABELS[app.focused_index()].to_lowercase(),
            theme.styles.dim,
        ),
    ];
    let subs_text = if app.subs.streamers().is_empty() {
        "(none — :subs add netflix)".to_string()
    } else {
        app.subs.streamers().join(" · ").to_lowercase()
    };
    left.extend(vec![
        Span::raw("   "),
        Span::styled("subs", theme.styles.muted),
        Span::raw(" › "),
        Span::styled(subs_text, theme.styles.dim),
    ]);
    let right = vec![
        Span::styled(format!("{active} followed"), theme.styles.dim),
        Span::raw(" · "),
        Span::styled(
            format!("{playable} playable"),
            Style::default().fg(if playable > 0 {
                theme.roles.playable
            } else {
                theme.roles.fg_dim
            }),
        ),
        Span::raw(" · "),
        Span::styled(
            format!("{dropping} dropping"),
            Style::default().fg(theme.roles.upcoming),
        ),
        Span::raw(" · "),
        Span::styled(clock, theme.styles.muted),
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
fn render_empty_state(f: &mut Frame, theme: &Theme, area: Rect) {
    let w = (area.width.saturating_sub(8)).min(72);
    let h = 14.min(area.height.saturating_sub(2));
    let x = area.x + (area.width.saturating_sub(w)) / 2;
    let y = area.y + (area.height.saturating_sub(h)) / 2;
    let rect = Rect::new(x, y, w, h);

    let block = Block::default()
        .borders(Borders::ALL)
        .border_type(BorderType::Rounded)
        .border_style(theme.styles.border_focused)
        .title(Span::styled(
            " welcome to animesh ",
            theme.styles.title_focused,
        ))
        .style(theme.styles.normal);
    let inner = block.inner(rect);
    f.render_widget(block, rect);

    let key = |k: &str| Span::styled(format!("  {k:<6}"), theme.styles.key);
    let lines = vec![
        Line::raw(""),
        Line::from(Span::styled(
            "  Your library is empty. Three keys to get going:",
            theme.styles.normal,
        )),
        Line::raw(""),
        Line::from(vec![
            key("a"),
            Span::styled(
                "Search AniList and follow your first show",
                theme.styles.muted,
            ),
        ]),
        Line::from(vec![
            key(":"),
            Span::styled(
                "Command mode — try :follow 21  (One Piece)",
                theme.styles.muted,
            ),
        ]),
        Line::from(vec![
            key("?"),
            Span::styled("Full keymap. ", theme.styles.muted),
            Span::styled("q quits.", theme.styles.dim),
        ]),
        Line::raw(""),
        Line::from(Span::styled(
            "  Once you've followed shows, panes split into",
            theme.styles.dim,
        )),
        Line::from(Span::styled(
            "  Playable now  ·  Dropping soon  ·  Following. Press c for LLM context.",
            theme.styles.dim,
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
    let theme = app.visible_theme();
    for (pane_idx, &chunk) in chunks.iter().enumerate() {
        render_panel(f, app, theme, pane_idx, chunk);
    }
}

fn render_panel(f: &mut Frame, app: &App, theme: &Theme, pane_idx: usize, area: Rect) {
    let focused = app.focused_index() == pane_idx;
    let items = app.items_in(pane_idx);
    let title_style = if focused {
        theme.styles.title_focused
    } else {
        theme.styles.dim.add_modifier(Modifier::BOLD)
    };
    let title = format!(
        " {}   {}   {} ",
        pane_idx + 1,
        PANE_LABELS[pane_idx],
        items.len()
    );
    let block = Block::default()
        .borders(Borders::LEFT)
        .border_type(BorderType::Plain)
        .border_style(if focused {
            theme.styles.border_focused
        } else {
            Style::default().fg(theme.roles.bg).bg(theme.roles.bg)
        })
        .title(Span::styled(title, title_style))
        .style(theme.styles.normal);
    let inner = block.inner(area);
    f.render_widget(block, area);

    let sel = app.selection[pane_idx];
    let lines: Vec<Line> = items
        .iter()
        .enumerate()
        .map(|(i, s)| render_row(theme, s, i == sel, focused, pane_idx, app.now))
        .collect();
    let para = Paragraph::new(lines).style(theme.styles.normal);
    f.render_widget(para, inner);
}

fn render_row(
    theme: &Theme,
    s: &crate::tui::model::Show,
    selected: bool,
    pane_focused: bool,
    pane_idx: usize,
    now: i64,
) -> Line<'static> {
    let _ = pane_idx;
    let selected_active = selected && pane_focused;
    let mark_color = if selected_active {
        theme.roles.accent
    } else {
        theme.roles.subtle
    };
    let mark = Span::styled("▸ ", Style::default().fg(mark_color));

    // Dim rows where we have a verified link but on no subscribed
    // streamer — they're catalogued but the user can't watch.
    let unreachable = s.last_verified.is_some() && !s.subscribed_match;
    let base = if unreachable {
        theme.roles.fg_dim
    } else {
        theme.roles.fg_muted
    };
    let title_style = if selected_active {
        Style::default()
            .fg(theme.roles.fg)
            .add_modifier(Modifier::BOLD)
    } else {
        Style::default().fg(base)
    };
    let title = Span::styled(s.display_title().to_string(), title_style);

    let middle = kind_descriptor(s);
    let (glyph, glyph_text, glyph_color) = badge_for(theme, s, now);
    let glyph_span = Span::styled(format!(" {glyph} "), Style::default().fg(glyph_color));
    let glyph_label = Span::styled(glyph_text, Style::default().fg(glyph_color));

    Line::from(vec![
        mark,
        title,
        Span::raw("  "),
        Span::styled(middle, theme.styles.dim),
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
fn badge_for(
    scheme: &Theme,
    s: &crate::tui::model::Show,
    now: i64,
) -> (&'static str, String, Color) {
    if let (Some(at), Some(streamer)) = (s.verified_at(), s.verified_streamer()) {
        if s.subscribed_match {
            return ("▶", streamer.to_lowercase(), scheme.roles.playable);
        }
        let _ = at;
    }
    if let Some(drop_at) = s.next_drop_at() {
        if drop_at > now {
            return ("🛈", relative_short(drop_at, now), scheme.roles.upcoming);
        }
        return ("✗", relative_short(drop_at, now), scheme.roles.late);
    }
    ("·", String::new(), scheme.roles.fg_dim)
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

fn render_cmdline(f: &mut Frame, app: &App, theme: &Theme, area: Rect) {
    let line = match app.overlay {
        Overlay::Command => Line::from(vec![
            Span::styled(":", theme.styles.key),
            Span::styled(&app.palette.query, theme.styles.normal),
            Span::styled("▏", theme.styles.normal),
        ]),
        Overlay::Search => Line::from(vec![
            Span::styled("/", theme.styles.key),
            Span::styled(&app.palette.query, theme.styles.normal),
            Span::styled("▏", theme.styles.normal),
        ]),
        Overlay::Follow => Line::from(vec![
            Span::styled("a ", theme.styles.key),
            Span::styled(&app.palette.query, theme.styles.normal),
            Span::styled("▏", theme.styles.normal),
        ]),
        Overlay::Theme => Line::from(vec![
            Span::styled("theme ", theme.styles.key),
            Span::styled("j/k preview · Enter apply · Esc cancel", theme.styles.dim),
        ]),
        _ => Line::from(vec![
            Span::styled(": ", theme.styles.key),
            Span::styled(
                "press : for commands  ·  / to jump  ·  a to follow  ·  t for theme",
                theme.styles.subtle,
            ),
        ]),
    };
    f.render_widget(Paragraph::new(line).style(theme.styles.normal), area);
}

/// Context-aware footer. Keys shown depend on which overlay is open.
fn render_hint_bar(f: &mut Frame, app: &App, theme: &Theme, area: Rect) {
    let (pairs, mode_label): (&[(&str, &str)], &str) = match app.overlay {
        Overlay::None => (
            &[
                ("j/k", "move"),
                ("tab", "pane"),
                ("w", "watched"),
                ("g", "stream"),
                ("c", "context"),
                ("t", "theme"),
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
                &[
                    ("Enter", "follow"),
                    ("↑↓ / j k", "select"),
                    ("Esc", "cancel"),
                ],
                " FOLLOW ",
            ),
            _ => (
                &[
                    ("typing", "local"),
                    ("Enter", "query sources"),
                    ("Esc", "cancel"),
                ],
                " FOLLOW ",
            ),
        },
        Overlay::Theme => (
            &[
                ("Enter", "apply"),
                ("↑↓ / j k", "preview"),
                ("Esc", "cancel"),
            ],
            " THEME ",
        ),
        Overlay::Help => (&[("Esc / ?", "close")], " HELP "),
    };

    let mut spans = Vec::with_capacity(pairs.len() * 4);
    for (key, label) in pairs {
        spans.push(Span::styled(*key, theme.styles.key));
        spans.push(Span::raw(" "));
        spans.push(Span::styled(*label, theme.styles.dim));
        spans.push(Span::raw("  "));
    }
    let chunks = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([Constraint::Min(0), Constraint::Length(10)])
        .split(area);
    f.render_widget(Paragraph::new(Line::from(spans)), chunks[0]);
    let mode = Line::from(vec![Span::styled(
        mode_label.to_string(),
        theme.styles.mode_badge,
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

fn render_command_palette(f: &mut Frame, app: &App, theme: &Theme, area: Rect) {
    let suggestions = command::suggest(&app.palette.query);
    let h = (4 + suggestions.len() as u16).min(18);
    let rect = centered_rect(area, 70, h);
    f.render_widget(Clear, rect);
    let block = Block::default()
        .borders(Borders::ALL)
        .border_type(BorderType::Rounded)
        .border_style(theme.styles.border_focused)
        .title(Span::styled(" :command ", theme.styles.title_focused))
        .style(theme.styles.popup);
    let inner = block.inner(rect);
    f.render_widget(block, rect);

    let mut lines: Vec<Line> = vec![
        Line::from(vec![
            Span::styled(":", theme.styles.key),
            Span::styled(&app.palette.query, theme.styles.popup),
            Span::styled("▏", theme.styles.popup),
        ]),
        Line::raw(""),
    ];
    for (i, s) in suggestions.iter().enumerate() {
        lines.push(render_suggestion(theme, s, i == app.palette.selected));
    }
    f.render_widget(
        Paragraph::new(lines)
            .style(theme.styles.popup)
            .wrap(Wrap { trim: false }),
        inner,
    );
}

fn render_suggestion(theme: &Theme, s: &Suggestion, selected: bool) -> Line<'static> {
    let arrow = Span::styled(
        if selected { "▸ " } else { "  " },
        Style::default().fg(if selected {
            theme.roles.accent
        } else {
            theme.roles.subtle
        }),
    );
    let name_style = if selected {
        Style::default()
            .fg(theme.roles.fg)
            .add_modifier(Modifier::BOLD)
    } else {
        Style::default().fg(theme.roles.fg)
    };
    let name = Span::styled(format!(":{}", s.spec.name), name_style);
    let arg = match s.spec.arg_hint {
        Some(h) => Span::styled(format!(" {h}"), Style::default().fg(theme.roles.fg_muted)),
        None => Span::raw(""),
    };
    let desc = Span::styled(
        format!("    {}", s.spec.description),
        Style::default().fg(theme.roles.fg_dim),
    );
    Line::from(vec![arrow, name, arg, desc])
}

fn render_search_palette(f: &mut Frame, app: &App, theme: &Theme, area: Rect) {
    let hits_n = app.palette.search_hits.len();
    let rect = centered_rect(area, 70, (4 + hits_n as u16).min(18));
    f.render_widget(Clear, rect);
    let block = Block::default()
        .borders(Borders::ALL)
        .border_type(BorderType::Rounded)
        .border_style(theme.styles.border_focused)
        .title(Span::styled(" /jump ", theme.styles.title_focused))
        .style(theme.styles.popup);
    let inner = block.inner(rect);
    f.render_widget(block, rect);

    let mut lines: Vec<Line> = vec![
        Line::from(vec![
            Span::styled("/", theme.styles.key),
            Span::styled(&app.palette.query, theme.styles.popup),
            Span::styled("▏", theme.styles.popup),
        ]),
        Line::raw(""),
    ];
    if hits_n == 0 {
        lines.push(Line::from(Span::styled(
            "  no followed shows match",
            theme.styles.dim,
        )));
    } else {
        for (i, &idx) in app.palette.search_hits.iter().enumerate().take(12) {
            let s = &app.shelf.shows[idx];
            let selected = i == app.palette.selected;
            let arrow = Span::styled(
                if selected { "▸ " } else { "  " },
                Style::default().fg(if selected {
                    theme.roles.accent
                } else {
                    theme.roles.subtle
                }),
            );
            let style = if selected {
                Style::default()
                    .fg(theme.roles.fg)
                    .add_modifier(Modifier::BOLD)
            } else {
                Style::default().fg(theme.roles.fg)
            };
            lines.push(Line::from(vec![
                arrow,
                Span::styled(s.display_title().to_string(), style),
            ]));
        }
    }
    f.render_widget(
        Paragraph::new(lines)
            .style(theme.styles.popup)
            .wrap(Wrap { trim: false }),
        inner,
    );
}

fn render_follow_palette(f: &mut Frame, app: &App, theme: &Theme, area: Rect) {
    let hits_n = app.palette.follow_hits.len();
    let rect = centered_rect(area, 74, (6 + hits_n as u16).min(20));
    f.render_widget(Clear, rect);
    let block = Block::default()
        .borders(Borders::ALL)
        .border_type(BorderType::Rounded)
        .border_style(theme.styles.border_focused)
        .title(Span::styled(
            " a · follow new show ",
            theme.styles.title_focused,
        ))
        .style(theme.styles.popup);
    let inner = block.inner(rect);
    f.render_widget(block, rect);

    let mut lines: Vec<Line> = vec![
        Line::from(vec![
            Span::styled("search › ", theme.styles.key),
            Span::styled(&app.palette.query, theme.styles.popup),
            Span::styled("▏", theme.styles.popup),
        ]),
        Line::raw(""),
    ];
    if let Some(err) = &app.palette.follow_error {
        lines.push(Line::from(Span::styled(
            format!("  {err}"),
            theme.styles.danger,
        )));
    }
    match app.palette.follow_stage {
        FollowStage::AwaitingQuery if hits_n == 0 => {
            lines.push(Line::from(Span::styled(
                "  type to search local candidates; Enter queries sources",
                theme.styles.dim,
            )));
        }
        _ => {
            for (i, m) in app.palette.follow_hits.iter().enumerate().take(10) {
                let selected = i == app.palette.selected;
                let arrow = Span::styled(
                    if selected { "▸ " } else { "  " },
                    Style::default().fg(if selected {
                        theme.roles.accent
                    } else {
                        theme.roles.subtle
                    }),
                );
                let style = if selected {
                    Style::default()
                        .fg(theme.roles.fg)
                        .add_modifier(Modifier::BOLD)
                } else {
                    Style::default().fg(theme.roles.fg)
                };
                let badge = Span::styled(
                    format!("  {}:{}  {:?}", m.source, m.source_id, m.kind),
                    theme.styles.dim,
                );
                lines.push(Line::from(vec![
                    arrow,
                    Span::styled(m.display_title.to_string(), style),
                    badge,
                ]));
            }
        }
    }
    f.render_widget(
        Paragraph::new(lines)
            .style(theme.styles.popup)
            .wrap(Wrap { trim: false }),
        inner,
    );
}

fn render_theme_picker(f: &mut Frame, app: &App, theme: &Theme, area: Rect) {
    let themes = app.theme_registry.all();
    let rect = centered_rect(area, 78, (5 + themes.len() as u16).min(18));
    f.render_widget(Clear, rect);
    let block = Block::default()
        .borders(Borders::ALL)
        .border_type(BorderType::Rounded)
        .border_style(theme.styles.border_focused)
        .title(Span::styled(" theme ", theme.styles.title_focused))
        .style(theme.styles.popup);
    let inner = block.inner(rect);
    f.render_widget(block, rect);

    let mut lines: Vec<Line> = Vec::with_capacity(themes.len() + 3);
    for (i, candidate) in themes.iter().enumerate() {
        let selected = i == app.theme_picker.selected;
        let arrow = Span::styled(
            if selected { "▸ " } else { "  " },
            Style::default().fg(if selected {
                theme.roles.accent
            } else {
                theme.roles.subtle
            }),
        );
        let name_style = if selected {
            Style::default()
                .fg(theme.roles.fg)
                .add_modifier(Modifier::BOLD)
        } else {
            Style::default().fg(theme.roles.fg)
        };
        let active = if candidate.id == app.active_theme_id {
            " current"
        } else {
            ""
        };
        lines.push(Line::from(vec![
            arrow,
            Span::styled(format!("{:<24}", candidate.name), name_style),
            Span::styled(
                format!(" {:<5}", candidate.appearance.label()),
                theme.styles.dim,
            ),
            Span::raw("  "),
            swatch(candidate.roles.accent),
            Span::raw(" "),
            swatch(candidate.roles.success),
            Span::raw(" "),
            swatch(candidate.roles.warning),
            Span::raw(" "),
            swatch(candidate.roles.danger),
            Span::styled(active, theme.styles.dim),
        ]));
    }
    lines.push(Line::raw(""));
    lines.push(Line::from(vec![
        Span::styled("  Enter", theme.styles.key),
        Span::styled(" apply · ", theme.styles.dim),
        Span::styled("Esc", theme.styles.key),
        Span::styled(" cancel · ", theme.styles.dim),
        Span::styled(":theme <id>", theme.styles.key),
        Span::styled(" direct apply", theme.styles.dim),
    ]));

    f.render_widget(
        Paragraph::new(lines)
            .style(theme.styles.popup)
            .wrap(Wrap { trim: false }),
        inner,
    );
}

fn swatch(color: Color) -> Span<'static> {
    Span::styled("  ", Style::default().bg(color))
}

fn render_help(f: &mut Frame, theme: &Theme, area: Rect) {
    let w = area.width.saturating_sub(8).min(72);
    let h = ((HELP_PAIRS.len() + 4) as u16).min(area.height.saturating_sub(4));
    let x = area.x + (area.width.saturating_sub(w)) / 2;
    let y = area.y + (area.height.saturating_sub(h)) / 2;
    let rect = Rect::new(x, y, w, h);
    f.render_widget(Clear, rect);
    let block = Block::default()
        .borders(Borders::ALL)
        .border_type(BorderType::Rounded)
        .border_style(theme.styles.border)
        .title(Span::styled(" keymap ", theme.styles.title))
        .style(theme.styles.popup);
    let inner = block.inner(rect);
    f.render_widget(block, rect);
    let lines: Vec<Line> = HELP_PAIRS
        .iter()
        .map(|(k, label)| {
            Line::from(vec![
                Span::styled(format!("  {k:<16}"), theme.styles.key),
                Span::styled((*label).to_string(), theme.styles.muted),
            ])
        })
        .collect();
    f.render_widget(Paragraph::new(lines).style(theme.styles.popup), inner);
}

fn render_toast(f: &mut Frame, theme: &Theme, area: Rect, text: &str) {
    let w = (text.len() as u16 + 6).min(area.width.saturating_sub(4));
    let x = area.x + (area.width.saturating_sub(w)) / 2;
    let y = area.y + area.height.saturating_sub(5);
    let rect = Rect::new(x, y, w, 3);
    f.render_widget(Clear, rect);
    let block = Block::default()
        .borders(Borders::ALL)
        .border_type(BorderType::Plain)
        .border_style(theme.styles.border_focused)
        .style(theme.styles.popup);
    let inner = block.inner(rect);
    f.render_widget(block, rect);
    f.render_widget(
        Paragraph::new(Line::from(Span::styled(
            text.to_string(),
            theme.styles.popup,
        )))
        .style(theme.styles.popup),
        inner,
    );
}
