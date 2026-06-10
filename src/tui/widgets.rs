//! Shared chrome: header with breadcrumb, key-chip footer, modal overlays,
//! JSON syntax coloring, loading lines.

use ratatui::Frame;
use ratatui::layout::{Constraint, Flex, Layout, Rect};
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, BorderType, Borders, Clear, Padding, Paragraph, Wrap};

use super::theme;
use super::{Modal, theme::spinner};

/// Top bar: app badge + breadcrumb (` › `-separated, last segment bright)
/// and right-aligned meta info.
pub fn draw_header(frame: &mut Frame, area: Rect, breadcrumb: &str, meta: Option<&str>) {
    let mut spans = vec![
        Span::styled(
            " hitpoint ",
            Style::new()
                .fg(theme::BADGE_FG)
                .bg(theme::ACCENT)
                .add_modifier(Modifier::BOLD),
        ),
        Span::raw("  "),
    ];
    let segments: Vec<&str> = breadcrumb.split(" ▸ ").collect();
    for (i, segment) in segments.iter().enumerate() {
        let last = i + 1 == segments.len();
        if i > 0 {
            spans.push(Span::styled(" › ", theme::dim()));
        }
        spans.push(Span::styled(
            segment.to_string(),
            if last {
                theme::bold(theme::text())
            } else {
                theme::soft()
            },
        ));
    }

    frame.render_widget(Paragraph::new(Line::from(spans)), area);

    if let Some(meta) = meta {
        let meta_line = Line::from(Span::styled(format!("{meta} "), theme::dim()));
        frame.render_widget(Paragraph::new(meta_line).right_aligned(), area);
    }
}

/// Bottom bar: status message (left), key chips (right-flowing after it).
pub fn draw_footer(frame: &mut Frame, area: Rect, hints: &[(&str, &str)], status: Option<&str>) {
    let mut spans = Vec::new();
    if let Some(status) = status {
        spans.push(Span::styled("● ", Style::new().fg(theme::YELLOW)));
        spans.push(Span::styled(
            format!("{status}   "),
            Style::new().fg(theme::YELLOW),
        ));
    }
    for (key, label) in hints {
        spans.push(Span::styled(
            format!(" {key} "),
            Style::new()
                .fg(theme::CYAN)
                .bg(theme::SEL_BG)
                .add_modifier(Modifier::BOLD),
        ));
        spans.push(Span::styled(format!(" {label}   "), theme::dim()));
    }
    frame.render_widget(Paragraph::new(Line::from(spans)), area);
}

/// The rounded panel every screen draws inside. Returns the inner area.
pub fn content_panel(frame: &mut Frame, area: Rect) -> Rect {
    let block = Block::default()
        .borders(Borders::ALL)
        .border_type(BorderType::Rounded)
        .border_style(theme::border())
        .padding(Padding::new(1, 1, 0, 0));
    let inner = block.inner(area);
    frame.render_widget(block, area);
    inner
}

pub fn draw_modal(frame: &mut Frame, modal: &Modal) {
    match modal {
        Modal::Info { title, body } => {
            let is_error = title == "error";
            let color = if is_error { theme::RED } else { theme::ACCENT };
            let width = (frame.area().width.saturating_sub(8)).min(64);
            let body_lines = (body.len() as u16 / width.max(1)).saturating_add(3);
            let area = centered_fixed(frame.area(), width, body_lines.clamp(5, 14));
            frame.render_widget(Clear, area);
            let icon = if is_error { "✗" } else { "i" };
            let block = Block::default()
                .borders(Borders::ALL)
                .border_type(BorderType::Rounded)
                .title(Span::styled(
                    format!(" {icon} {title} "),
                    Style::new().fg(color).add_modifier(Modifier::BOLD),
                ))
                .border_style(Style::new().fg(color))
                .padding(Padding::new(1, 1, 0, 0));
            let mut lines = vec![Line::raw("")];
            lines.push(Line::from(Span::styled(body.clone(), theme::text())));
            let paragraph = Paragraph::new(lines)
                .wrap(Wrap { trim: false })
                .block(block);
            frame.render_widget(paragraph, area);
        }
        Modal::Prompt {
            label,
            secret,
            input,
            ..
        } => {
            let area = centered_fixed(frame.area(), 56, 7);
            frame.render_widget(Clear, area);
            let block = Block::default()
                .borders(Borders::ALL)
                .border_type(BorderType::Rounded)
                .title(Span::styled(
                    " ⚿ login ",
                    Style::new().fg(theme::ACCENT).add_modifier(Modifier::BOLD),
                ))
                .border_style(Style::new().fg(theme::ACCENT))
                .padding(Padding::new(1, 1, 0, 0));
            let shown = if *secret {
                "•".repeat(input.chars().count())
            } else {
                input.clone()
            };
            let lines = vec![
                Line::raw(""),
                Line::from(Span::styled(label.clone(), theme::bold(theme::text()))),
                Line::from(vec![
                    Span::styled("❯ ", theme::accent()),
                    Span::styled(
                        shown,
                        Style::new()
                            .fg(theme::FG)
                            .add_modifier(Modifier::UNDERLINED),
                    ),
                    Span::styled("▏", theme::accent()),
                ]),
                Line::raw(""),
                Line::from(vec![
                    Span::styled(" enter ", chip()),
                    Span::styled(" submit   ", theme::dim()),
                    Span::styled(" esc ", chip()),
                    Span::styled(" cancel", theme::dim()),
                ]),
            ];
            frame.render_widget(Paragraph::new(lines).block(block), area);
        }
    }
}

fn chip() -> Style {
    Style::new().fg(theme::CYAN).bg(theme::SEL_BG)
}

/// A centered rect with a fixed size, clamped to the parent.
pub fn centered_fixed(parent: Rect, width: u16, height: u16) -> Rect {
    let [area] = Layout::horizontal([Constraint::Length(width.min(parent.width))])
        .flex(Flex::Center)
        .areas(parent);
    let [area] = Layout::vertical([Constraint::Length(height.min(parent.height))])
        .flex(Flex::Center)
        .areas(area);
    area
}

pub fn loading_line(label: &str, frame: u64) -> Line<'static> {
    Line::from(vec![
        Span::raw(" "),
        Span::styled(
            format!("{} ", spinner(frame)),
            Style::new().fg(theme::ACCENT),
        ),
        Span::styled(
            format!("loading {label}…"),
            Style::new()
                .fg(theme::FG_SOFT)
                .add_modifier(Modifier::ITALIC),
        ),
    ])
}

/// Per-line JSON syntax coloring for pretty-printed bodies: keys cyan,
/// strings green, numbers orange, booleans/null magenta, punctuation dim.
pub fn colorize_json_line(line: &str) -> Line<'static> {
    let indent_len = line.len() - line.trim_start().len();
    let (indent, rest) = line.split_at(indent_len);
    let mut spans = vec![Span::raw(indent.to_string())];

    let mut value_part = rest;
    // `"key": value` — split off the key when present.
    if rest.starts_with('"')
        && let Some(colon) = rest.find("\": ")
    {
        spans.push(Span::styled(
            rest[..colon + 1].to_string(),
            Style::new().fg(theme::CYAN),
        ));
        spans.push(Span::styled(": ".to_string(), theme::dim()));
        value_part = &rest[colon + 3..];
    }

    let trailing_comma = value_part.ends_with(',');
    let value = value_part.trim_end_matches(',');
    let style = match value.chars().next() {
        Some('"') => Style::new().fg(theme::GREEN),
        Some(c) if c.is_ascii_digit() || c == '-' => Style::new().fg(theme::ORANGE),
        Some('t') | Some('f') | Some('n') => Style::new().fg(theme::MAGENTA),
        Some('{') | Some('}') | Some('[') | Some(']') => theme::dim(),
        _ => theme::text(),
    };
    spans.push(Span::styled(value.to_string(), style));
    if trailing_comma {
        spans.push(Span::styled(",".to_string(), theme::dim()));
    }
    Line::from(spans)
}

/// Centered empty-state panel.
pub fn empty_state(frame: &mut Frame, area: Rect, headline: &str, hint: &str) {
    let region = centered_fixed(area, (hint.len() as u16 + 6).max(40), 5);
    let lines = vec![
        Line::from(Span::styled(
            headline.to_string(),
            theme::bold(theme::soft()),
        ))
        .centered(),
        Line::raw(""),
        Line::from(Span::styled(hint.to_string(), Style::new().fg(theme::CYAN))).centered(),
    ];
    frame.render_widget(Paragraph::new(lines), region);
}
