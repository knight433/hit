//! Small shared rendering helpers: header, key-hint footer, modal overlay,
//! method coloring.

use ratatui::Frame;
use ratatui::layout::{Constraint, Flex, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Clear, Paragraph, Wrap};

use super::Modal;

pub fn draw_header(frame: &mut Frame, area: Rect, title: &str) {
    let line = Line::from(vec![
        Span::styled(" hitpoint ", Style::new().fg(Color::Black).bg(Color::Cyan)),
        Span::raw(" "),
        Span::styled(title.to_string(), Style::new().add_modifier(Modifier::BOLD)),
    ]);
    frame.render_widget(Paragraph::new(line), area);
}

pub fn draw_footer(frame: &mut Frame, area: Rect, hints: &[(&str, &str)], status: Option<&str>) {
    let mut spans = Vec::new();
    if let Some(status) = status {
        spans.push(Span::styled(
            format!(" {status} "),
            Style::new().fg(Color::Yellow),
        ));
        spans.push(Span::raw(" "));
    }
    for (key, label) in hints {
        spans.push(Span::styled(
            format!(" {key} "),
            Style::new().fg(Color::Black).bg(Color::DarkGray),
        ));
        spans.push(Span::raw(format!(" {label}  ")));
    }
    frame.render_widget(Paragraph::new(Line::from(spans)), area);
}

pub fn draw_modal(frame: &mut Frame, modal: &Modal) {
    match modal {
        Modal::Info { title, body } => {
            let area = centered(frame.area(), 60, 30);
            frame.render_widget(Clear, area);
            let block = Block::default()
                .borders(Borders::ALL)
                .title(format!(" {title} "))
                .border_style(Style::new().fg(Color::Red));
            let paragraph = Paragraph::new(body.as_str())
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
            let area = centered(frame.area(), 60, 22);
            frame.render_widget(Clear, area);
            let block = Block::default()
                .borders(Borders::ALL)
                .title(" login ")
                .border_style(Style::new().fg(Color::Cyan));
            let shown = if *secret {
                "•".repeat(input.chars().count())
            } else {
                input.clone()
            };
            let lines = vec![
                Line::raw(""),
                Line::from(Span::styled(
                    format!(" {label}:"),
                    Style::new().add_modifier(Modifier::BOLD),
                )),
                Line::from(vec![
                    Span::raw(" > "),
                    Span::styled(shown, Style::new().fg(Color::Yellow)),
                    Span::styled("▏", Style::new().fg(Color::Yellow)),
                ]),
                Line::raw(""),
                Line::from(Span::styled(" enter: submit   esc: cancel", dim())),
            ];
            frame.render_widget(Paragraph::new(lines).block(block), area);
        }
    }
}

/// A centered rect occupying the given percentages of the parent.
pub fn centered(parent: Rect, percent_x: u16, percent_y: u16) -> Rect {
    let [area] = Layout::horizontal([Constraint::Percentage(percent_x)])
        .flex(Flex::Center)
        .areas(parent);
    let [area] = Layout::vertical([Constraint::Percentage(percent_y)])
        .flex(Flex::Center)
        .areas(area);
    area
}

pub fn method_style(method: &str) -> Style {
    let color = match method {
        "GET" => Color::Green,
        "POST" => Color::Blue,
        "PUT" => Color::Yellow,
        "PATCH" => Color::Magenta,
        "DELETE" => Color::Red,
        _ => Color::Gray,
    };
    Style::new().fg(color).add_modifier(Modifier::BOLD)
}

pub fn status_style(status: u16) -> Style {
    let color = match status {
        200..=299 => Color::Green,
        300..=399 => Color::Cyan,
        400..=499 => Color::Yellow,
        _ => Color::Red,
    };
    Style::new().fg(color).add_modifier(Modifier::BOLD)
}

/// Dim style for excluded rows, etc.
pub fn dim() -> Style {
    Style::new().fg(Color::DarkGray)
}

pub fn loading_line(label: &str) -> Line<'static> {
    Line::from(Span::styled(
        format!("  loading {label}…"),
        Style::new().fg(Color::Yellow).italic(),
    ))
}
