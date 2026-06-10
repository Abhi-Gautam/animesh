//! Detail pane composition. Mirrors the handoff's right column.

use ratatui::{
    layout::{Constraint, Direction, Layout, Rect},
    style::{Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, Paragraph, Wrap},
    Frame,
};

use crate::tui::app::App;
use crate::tui::pane::Pane;
use crate::tui::theme::Theme;

pub(crate) fn render(f: &mut Frame, app: &App, area: Rect) {
    let theme = app.visible_theme();
    let block = Block::default()
        .borders(Borders::NONE)
        .style(theme.styles.normal);
    let inner = block.inner(area);
    f.render_widget(block, area);

    let s = match app.current() {
        Some(s) => s,
        None => {
            let lines = vec![
                Line::raw(""),
                Line::from(Span::styled("  No selection.", theme.styles.dim)),
                Line::raw(""),
                Line::from(Span::styled(
                    "  Follow a show with `animesh follow --id <N>` outside the TUI,",
                    theme.styles.dim,
                )),
                Line::from(Span::styled(
                    "  then relaunch. Interactive add lands when the palette wires AniList in T40.",
                    theme.styles.dim,
                )),
            ];
            f.render_widget(Paragraph::new(lines).wrap(Wrap { trim: false }), inner);
            return;
        }
    };

    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .margin(1)
        .constraints([
            Constraint::Length(5), // hero (title + sub + stats)
            Constraint::Length(2), // progress
            Constraint::Length(3), // watch-on
            Constraint::Length(5), // refs (header + up to 4 source rows)
            Constraint::Length(3), // verification provenance
            Constraint::Length(4), // episodes
            Constraint::Min(0),    // synopsis
        ])
        .split(inner);

    render_hero(f, theme, s, app.now, chunks[0]);
    render_progress(f, theme, s, chunks[1]);
    render_watch_on(f, theme, s, chunks[2]);
    render_refs(f, theme, app, s, chunks[3]);
    render_verification(f, theme, s, app.now, chunks[4]);
    render_episodes(f, theme, s, chunks[5]);
    render_synopsis(f, theme, s, chunks[6]);
}

fn section_label(theme: &Theme, label: &'static str) -> Line<'static> {
    Line::from(Span::styled(
        label,
        Style::default()
            .fg(theme.roles.subtle)
            .add_modifier(Modifier::BOLD),
    ))
}

fn render_hero(f: &mut Frame, theme: &Theme, s: &crate::tui::model::Show, now: i64, area: Rect) {
    let title = s.display_title();
    let title_line = Line::from(Span::styled(title.to_string(), theme.styles.title));

    // Plain dimmed underline. Clip to pane width so we never overflow
    // on narrow terminals.
    let title_cells = title.chars().count().min(area.width as usize);
    let underline_line = Line::from(Span::styled(
        "━".repeat(title_cells),
        Style::default().fg(theme.roles.subtle),
    ));

    // Metadata sub-line (romaji removed — was a duplicate of the title
    // for shows whose english + romaji match).
    let mut sub: Vec<Span<'static>> = Vec::new();
    if let Some(total) = s.total() {
        sub.push(Span::styled(format!("{total} eps"), theme.styles.dim));
    }
    if let Some(f) = s.format() {
        if !sub.is_empty() {
            sub.push(Span::styled(" · ", theme.styles.subtle));
        }
        sub.push(Span::styled(f.to_string(), theme.styles.dim));
    }
    if let Some(stu) = s.studios() {
        if !sub.is_empty() {
            sub.push(Span::styled(" · ", theme.styles.subtle));
        }
        sub.push(Span::styled(stu.to_string(), theme.styles.dim));
    }
    let sub_line = if sub.is_empty() {
        Line::from(Span::styled(
            "(no metadata cached — run `animesh sync`)",
            theme.styles.dim,
        ))
    } else {
        Line::from(sub)
    };

    let mut stat: Vec<Span<'static>> = Vec::new();
    match s.pane {
        Some(Pane::Playable) => {
            stat.push(Span::styled("PLAYABLE NOW  ", theme.styles.subtle));
            match s.verified_streamer() {
                Some(streamer) => stat.push(Span::styled(
                    streamer.to_lowercase(),
                    Style::default()
                        .fg(theme.roles.playable)
                        .add_modifier(Modifier::BOLD),
                )),
                None => stat.push(Span::styled(
                    "verified",
                    Style::default()
                        .fg(theme.roles.playable)
                        .add_modifier(Modifier::BOLD),
                )),
            }
        }
        Some(Pane::Dropping) => {
            stat.push(Span::styled("DROPS  ", theme.styles.subtle));
            match s.next_drop_at() {
                Some(at) => stat.push(Span::styled(
                    crate::tui::view::relative_short(at, now),
                    Style::default()
                        .fg(theme.roles.upcoming)
                        .add_modifier(Modifier::BOLD),
                )),
                None => stat.push(Span::styled(
                    "soon",
                    Style::default()
                        .fg(theme.roles.upcoming)
                        .add_modifier(Modifier::BOLD),
                )),
            }
        }
        Some(Pane::Following) => {
            let ep = s.next_episode().unwrap_or(0);
            if ep > 0 {
                stat.push(Span::styled("RESUME  ", theme.styles.subtle));
                stat.push(Span::styled(
                    format!("E{ep}"),
                    Style::default()
                        .fg(theme.roles.episode)
                        .add_modifier(Modifier::BOLD),
                ));
            } else {
                stat.push(Span::styled(
                    "FOLLOWING",
                    theme.styles.subtle.add_modifier(Modifier::BOLD),
                ));
            }
        }
        None => {}
    }
    stat.push(Span::styled("        ", Style::default()));
    stat.push(Span::styled("SCORE  ", theme.styles.subtle));
    match s.score() {
        Some(n) => stat.push(Span::styled(
            format!("★ {:.1}", n / 10.0),
            Style::default()
                .fg(theme.roles.fg)
                .add_modifier(Modifier::BOLD),
        )),
        None => stat.push(Span::styled("—", theme.styles.dim)),
    }
    let stat_line = Line::from(stat);

    let lines = vec![
        title_line,
        underline_line,
        sub_line,
        Line::raw(""),
        stat_line,
    ];
    f.render_widget(Paragraph::new(lines).wrap(Wrap { trim: false }), area);
}

fn render_progress(f: &mut Frame, theme: &Theme, s: &crate::tui::model::Show, area: Rect) {
    let seen = s.seen();
    let total = s.total().unwrap_or(0);
    let line = Line::from(vec![
        Span::styled("watched  ", theme.styles.dim),
        Span::styled(
            seen.to_string(),
            Style::default()
                .fg(theme.roles.fg_muted)
                .add_modifier(Modifier::BOLD),
        ),
        Span::styled(format!(" / {total}"), theme.styles.dim),
    ]);
    f.render_widget(Paragraph::new(line), area);
}

fn render_watch_on(f: &mut Frame, theme: &Theme, s: &crate::tui::model::Show, area: Rect) {
    let lines = if s.streaming.is_empty() {
        vec![
            section_label(theme, "WATCH ON"),
            Line::from(Span::styled(
                "  — (run `animesh sync` to refresh streaming links)",
                theme.styles.dim,
            )),
        ]
    } else {
        let mut spans = vec![Span::raw("  ")];
        for (i, link) in s.streaming.iter().enumerate() {
            let name = link.site.as_deref().unwrap_or("?");
            let style = if i == 0 {
                Style::default()
                    .fg(theme.roles.playable)
                    .add_modifier(Modifier::BOLD)
            } else {
                Style::default().fg(theme.roles.fg_muted)
            };
            spans.push(Span::styled(format!("● {name}"), style));
            spans.push(Span::raw("   "));
        }
        spans.push(Span::styled("press ", theme.styles.dim));
        spans.push(Span::styled("g", theme.styles.key));
        spans.push(Span::styled(" to open", theme.styles.dim));
        vec![section_label(theme, "WATCH ON"), Line::from(spans)]
    };
    f.render_widget(Paragraph::new(lines).wrap(Wrap { trim: false }), area);
}

/// REFS — every source the canonicalizer attached. Shows the
/// substrate, not just the row. Limited to four lines to keep the
/// fixed-height pane from clipping; if a canonical ever has more,
/// `+N more` could land here.
fn render_refs(f: &mut Frame, theme: &Theme, app: &App, s: &crate::tui::model::Show, area: Rect) {
    let refs = app
        .facade
        .source_refs_for(s.canonical_id())
        .unwrap_or_default();
    let mut lines: Vec<Line> = vec![section_label(theme, "REFS")];
    if refs.is_empty() {
        lines.push(Line::from(Span::styled("  (none)", theme.styles.dim)));
    } else {
        for r in refs.iter().take(4) {
            lines.push(Line::from(vec![
                Span::styled(format!("  {:<8}", r.source), theme.styles.muted),
                Span::styled(format!("#{}", r.source_id), theme.styles.dim),
            ]));
        }
    }
    f.render_widget(Paragraph::new(lines).wrap(Wrap { trim: false }), area);
}

/// VERIFICATION — when we last saw the source confirm playability on
/// a streamer. Glyph + color shift with subscribed_match: ▶/playable
/// means hit `g` and it works; ·/dim means the link exists but on a
/// service the user doesn't subscribe to.
fn render_verification(
    f: &mut Frame,
    theme: &Theme,
    s: &crate::tui::model::Show,
    now: i64,
    area: Rect,
) {
    let mut lines: Vec<Line> = vec![section_label(theme, "VERIFICATION")];
    if let (Some(streamer), Some(at)) = (s.verified_streamer(), s.verified_at()) {
        let glyph = if s.subscribed_match { "▶" } else { "·" };
        let color = if s.subscribed_match {
            theme.roles.playable
        } else {
            theme.roles.fg_dim
        };
        lines.push(Line::from(vec![
            Span::styled(format!("  {glyph} "), Style::default().fg(color)),
            Span::styled(streamer.to_lowercase(), Style::default().fg(color)),
            Span::styled(
                format!("  · verified {}", crate::tui::view::relative_short(at, now)),
                theme.styles.muted,
            ),
        ]));
    } else if let Some(drop_at) = s.next_drop_at() {
        lines.push(Line::from(vec![
            Span::styled("  🛈 scheduled  ", theme.styles.info),
            Span::styled(
                crate::tui::view::relative_short(drop_at, now),
                theme.styles.muted,
            ),
        ]));
    } else {
        lines.push(Line::from(Span::styled(
            "  no drops scheduled",
            theme.styles.dim,
        )));
    }
    f.render_widget(Paragraph::new(lines).wrap(Wrap { trim: false }), area);
}

fn render_episodes(f: &mut Frame, theme: &Theme, s: &crate::tui::model::Show, area: Rect) {
    let seen = s.seen();
    let next = s.next_episode().unwrap_or(seen + 1);
    let total = s.total().unwrap_or(next.max(seen + 1));
    let from = (next - 5).max(1);
    let to = (from + 9).min(total.max(1));
    let mut spans: Vec<Span<'static>> = Vec::with_capacity((to - from + 1) as usize * 2);
    spans.push(Span::raw("  "));
    for e in from..=to {
        let style = if e <= seen {
            Style::default()
                .fg(theme.roles.watched)
                .add_modifier(Modifier::BOLD)
        } else if e == next {
            Style::default()
                .fg(theme.roles.accent)
                .add_modifier(Modifier::BOLD)
        } else if Some(e) == s.next_episode()
            && s.airs_at()
                .is_some_and(|a| a < chrono::Utc::now().timestamp())
        {
            theme.styles.danger
        } else {
            theme.styles.dim
        };
        spans.push(Span::styled(format!("{e:>3}"), style));
        spans.push(Span::raw(" "));
    }
    let lines = vec![
        section_label(theme, "EPISODES"),
        Line::raw(""),
        Line::from(spans),
    ];
    f.render_widget(Paragraph::new(lines).wrap(Wrap { trim: false }), area);
}

fn render_synopsis(f: &mut Frame, theme: &Theme, s: &crate::tui::model::Show, area: Rect) {
    let body = s
        .description()
        .map(strip_html)
        .unwrap_or_else(|| "— (no synopsis cached — run `animesh sync` to fetch)".into());
    let lines = vec![
        section_label(theme, "SYNOPSIS"),
        Line::raw(""),
        Line::from(Span::styled(body, theme.styles.muted)),
    ];
    f.render_widget(Paragraph::new(lines).wrap(Wrap { trim: true }), area);
}

fn strip_html(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut in_tag = false;
    for c in s.chars() {
        match c {
            '<' => in_tag = true,
            '>' => in_tag = false,
            c if !in_tag => out.push(c),
            _ => {}
        }
    }
    // Collapse <br>-derived double newlines into spaces for one-paragraph display.
    out.replace('\n', " ").replace("  ", " ")
}
