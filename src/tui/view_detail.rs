//! Detail pane composition. Mirrors the handoff's right column.

use ratatui::{
    layout::{Constraint, Direction, Layout, Rect},
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, Paragraph, Wrap},
    Frame,
};

use crate::tui::app::App;
use crate::tui::pane::Pane;

const ACCENT: Color = Color::Rgb(0xff, 0x7a, 0x59);
const INK: Color = Color::Rgb(0xcd, 0xd6, 0xe2);
const INK_2: Color = Color::Rgb(0x9a, 0xa6, 0xb6);
const DIM: Color = Color::Rgb(0x58, 0x60, 0x71);
const DIMMER: Color = Color::Rgb(0x3a, 0x43, 0x50);
const SOON: Color = Color::Rgb(0x61, 0xaf, 0xef);
const LATE: Color = Color::Rgb(0xe0, 0x6c, 0x75);
const TIME_GREEN: Color = Color::Rgb(0x98, 0xc3, 0x79);
const EP_GOLD: Color = Color::Rgb(0xe5, 0xc0, 0x7b);

pub fn render(f: &mut Frame, app: &App, area: Rect) {
    let block = Block::default()
        .borders(Borders::NONE)
        .style(Style::default().bg(Color::Reset));
    let inner = block.inner(area);
    f.render_widget(block, area);

    let s = match app.current() {
        Some(s) => s,
        None => {
            let lines = vec![
                Line::raw(""),
                Line::from(Span::styled(
                    "  No selection.",
                    Style::default().fg(DIM),
                )),
                Line::raw(""),
                Line::from(Span::styled(
                    "  Follow a show with `animesh follow --id <N>` outside the TUI,",
                    Style::default().fg(DIM),
                )),
                Line::from(Span::styled(
                    "  then relaunch. Interactive add lands when the palette wires AniList in T40.",
                    Style::default().fg(DIM),
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
            Constraint::Length(4), // episodes
            Constraint::Min(0),    // synopsis
        ])
        .split(inner);

    render_hero(f, s, app.now, chunks[0]);
    render_progress(f, s, chunks[1]);
    render_watch_on(f, s, chunks[2]);
    render_episodes(f, s, chunks[3]);
    render_synopsis(f, s, chunks[4]);
}

/// Paint the title gradient-colored across the show's palette. Each
/// character gets a smoothly-interpolated RGB along the palette so the
/// title itself encodes the cover's identity. Plain ASCII glyphs —
/// the colored underline strip below carries the heavy chromatic load.
fn palette_painted_title(title: &str, palette: Option<&[(u8, u8, u8)]>) -> Vec<Span<'static>> {
    let chars: Vec<char> = title.chars().collect();
    let n = chars.len().max(1);
    let Some(pal) = palette else {
        return vec![Span::styled(
            title.to_string(),
            Style::default().fg(Color::White).add_modifier(Modifier::BOLD),
        )];
    };
    chars
        .iter()
        .enumerate()
        .map(|(i, ch)| {
            let t = if n == 1 { 0.0 } else { i as f32 / (n - 1) as f32 };
            let (r, g, b) = crate::tui::ascii_art::lerp(pal, t);
            Span::styled(
                ch.to_string(),
                Style::default()
                    .fg(Color::Rgb(r, g, b))
                    .add_modifier(Modifier::BOLD),
            )
        })
        .collect()
}

/// A `cells`-wide colored strip of `━` painted along the same palette
/// gradient as the title — shows the palette as a pure color band,
/// which is more legible than the per-character title tinting alone.
fn palette_painted_strip(cells: usize, palette: Option<&[(u8, u8, u8)]>) -> Vec<Span<'static>> {
    let Some(pal) = palette else {
        return vec![Span::styled(
            "━".repeat(cells),
            Style::default().fg(DIMMER),
        )];
    };
    let n = cells.max(1);
    (0..cells)
        .map(|i| {
            let t = if n == 1 { 0.0 } else { i as f32 / (n - 1) as f32 };
            let (r, g, b) = crate::tui::ascii_art::lerp(pal, t);
            Span::styled("━", Style::default().fg(Color::Rgb(r, g, b)))
        })
        .collect()
}

fn render_hero(f: &mut Frame, s: &crate::tui::model::Show, now: i64, area: Rect) {
    let title = s.display_title();
    let palette = s.cover_ascii().and_then(crate::tui::ascii_art::decode);
    let title_line = Line::from(Span::styled(
        title.to_string(),
        Style::default().fg(Color::White).add_modifier(Modifier::BOLD),
    ));

    // Underline strip — same visual width as the fullwidth title
    // (each source char → 2 cells). Clip to the pane width so we
    // never overflow on narrow terminals.
    let title_cells = title.chars().count().min(area.width as usize);
    let underline_line = Line::from(palette_painted_strip(title_cells, palette.as_deref()));

    // Metadata sub-line (romaji removed — was a duplicate of the title
    // for shows whose english + romaji match).
    let mut sub: Vec<Span<'static>> = Vec::new();
    if let Some(total) = s.total() {
        sub.push(Span::styled(
            format!("{total} eps"),
            Style::default().fg(DIM),
        ));
    }
    if let Some(f) = s.format() {
        if !sub.is_empty() {
            sub.push(Span::styled(" · ", Style::default().fg(DIMMER)));
        }
        sub.push(Span::styled(f.to_string(), Style::default().fg(DIM)));
    }
    if let Some(stu) = s.studios() {
        if !sub.is_empty() {
            sub.push(Span::styled(" · ", Style::default().fg(DIMMER)));
        }
        sub.push(Span::styled(stu.to_string(), Style::default().fg(DIM)));
    }
    let sub_line = if sub.is_empty() {
        Line::from(Span::styled("(no metadata cached — run `animesh sync`)", Style::default().fg(DIM)))
    } else {
        Line::from(sub)
    };

    let mut stat: Vec<Span<'static>> = Vec::new();
    match s.pane {
        Some(Pane::Today) => {
            let ep = s.next_episode().unwrap_or(0);
            let rel = relative(s.airs_at().unwrap_or(now), now);
            stat.push(Span::styled("NEXT EPISODE  ", Style::default().fg(DIMMER)));
            stat.push(Span::styled(format!("E{ep}"), Style::default().fg(EP_GOLD).add_modifier(Modifier::BOLD)));
            stat.push(Span::styled(" · ", Style::default().fg(DIM)));
            stat.push(Span::styled(rel, Style::default().fg(SOON)));
        }
        Some(Pane::Late) => {
            let ep = s.next_episode().unwrap_or(0);
            let rel = relative(s.airs_at().unwrap_or(now), now);
            stat.push(Span::styled("AIRED  ", Style::default().fg(DIMMER)));
            stat.push(Span::styled(format!("E{ep}"), Style::default().fg(EP_GOLD).add_modifier(Modifier::BOLD)));
            stat.push(Span::styled(" · ", Style::default().fg(DIM)));
            stat.push(Span::styled(rel, Style::default().fg(LATE)));
        }
        Some(Pane::Backlog { behind }) => {
            let ep = s.next_episode().unwrap_or(s.seen() + 1);
            stat.push(Span::styled("RESUME  ", Style::default().fg(DIMMER)));
            stat.push(Span::styled(format!("E{ep}"), Style::default().fg(EP_GOLD).add_modifier(Modifier::BOLD)));
            stat.push(Span::styled(" · ", Style::default().fg(DIM)));
            let tail = if behind > 0 {
                Span::styled(format!("{behind} behind"), Style::default().fg(LATE))
            } else {
                Span::styled("finale", Style::default().fg(SOON))
            };
            stat.push(tail);
        }
        None => {}
    }
    stat.push(Span::styled("        ", Style::default()));
    stat.push(Span::styled("SCORE  ", Style::default().fg(DIMMER)));
    match s.score() {
        Some(n) => stat.push(Span::styled(
            format!("★ {:.1}", n / 10.0),
            Style::default().fg(INK).add_modifier(Modifier::BOLD),
        )),
        None => stat.push(Span::styled("—", Style::default().fg(DIM))),
    }
    let stat_line = Line::from(stat);

    let lines = vec![title_line, underline_line, sub_line, Line::raw(""), stat_line];
    f.render_widget(Paragraph::new(lines).wrap(Wrap { trim: false }), area);
}

fn render_progress(f: &mut Frame, s: &crate::tui::model::Show, area: Rect) {
    let seen = s.seen();
    let total = s.total().unwrap_or(0);
    let line = Line::from(vec![
        Span::styled("watched  ", Style::default().fg(DIM)),
        Span::styled(
            seen.to_string(),
            Style::default().fg(INK_2).add_modifier(Modifier::BOLD),
        ),
        Span::styled(format!(" / {total}"), Style::default().fg(DIM)),
    ]);
    f.render_widget(Paragraph::new(line), area);
}

fn render_watch_on(f: &mut Frame, s: &crate::tui::model::Show, area: Rect) {
    let lines = if s.streaming.is_empty() {
        vec![
            Line::from(Span::styled("WATCH ON", Style::default().fg(DIMMER).add_modifier(Modifier::BOLD))),
            Line::from(Span::styled(
                "  — (run `animesh sync` to refresh streaming links)",
                Style::default().fg(DIM),
            )),
        ]
    } else {
        let mut spans = vec![Span::raw("  ")];
        for (i, link) in s.streaming.iter().enumerate() {
            let name = link.site.as_deref().unwrap_or("?");
            let style = if i == 0 {
                Style::default().fg(ACCENT).add_modifier(Modifier::BOLD)
            } else {
                Style::default().fg(INK_2)
            };
            spans.push(Span::styled(format!("● {name}"), style));
            spans.push(Span::raw("   "));
        }
        spans.push(Span::styled("press ", Style::default().fg(DIM)));
        spans.push(Span::styled("g", Style::default().fg(ACCENT).add_modifier(Modifier::BOLD)));
        spans.push(Span::styled(" to open", Style::default().fg(DIM)));
        vec![
            Line::from(Span::styled("WATCH ON", Style::default().fg(DIMMER).add_modifier(Modifier::BOLD))),
            Line::from(spans),
        ]
    };
    f.render_widget(Paragraph::new(lines).wrap(Wrap { trim: false }), area);
}

fn render_episodes(f: &mut Frame, s: &crate::tui::model::Show, area: Rect) {
    let seen = s.seen();
    let next = s.next_episode().unwrap_or(seen + 1);
    let total = s.total().unwrap_or(next.max(seen + 1));
    let from = (next - 5).max(1);
    let to = (from + 9).min(total.max(1));
    let mut spans: Vec<Span<'static>> = Vec::with_capacity((to - from + 1) as usize * 2);
    spans.push(Span::raw("  "));
    for e in from..=to {
        let style = if e <= seen {
            Style::default().fg(TIME_GREEN).add_modifier(Modifier::BOLD)
        } else if e == next {
            Style::default().fg(ACCENT).add_modifier(Modifier::BOLD)
        } else if Some(e) == s.next_episode() && s.airs_at().map_or(false, |a| a < chrono::Utc::now().timestamp()) {
            Style::default().fg(LATE)
        } else {
            Style::default().fg(DIM)
        };
        spans.push(Span::styled(format!("{e:>3}"), style));
        spans.push(Span::raw(" "));
    }
    let lines = vec![
        Line::from(Span::styled(
            "EPISODES",
            Style::default().fg(DIMMER).add_modifier(Modifier::BOLD),
        )),
        Line::raw(""),
        Line::from(spans),
    ];
    f.render_widget(Paragraph::new(lines).wrap(Wrap { trim: false }), area);
}

fn render_synopsis(f: &mut Frame, s: &crate::tui::model::Show, area: Rect) {
    let body = s
        .description()
        .map(|d| strip_html(d))
        .unwrap_or_else(|| "— (no synopsis cached — run `animesh sync` to fetch)".into());
    let lines = vec![
        Line::from(Span::styled(
            "SYNOPSIS",
            Style::default().fg(DIMMER).add_modifier(Modifier::BOLD),
        )),
        Line::raw(""),
        Line::from(Span::styled(body, Style::default().fg(INK_2))),
    ];
    f.render_widget(Paragraph::new(lines).wrap(Wrap { trim: true }), area);
}

fn relative(at: i64, now: i64) -> String {
    let diff = at - now;
    let a = diff.abs();
    let body = if a >= 86400 {
        format!("{}d", a / 86400)
    } else if a >= 3600 {
        format!("{}h", a / 3600)
    } else {
        format!("{}m", (a / 60).max(1))
    };
    if diff < 0 { format!("{body} ago") } else { format!("in {body}") }
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

