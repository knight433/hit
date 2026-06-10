//! Response viewer: status, latency, headers (toggle), pretty body, and
//! FastAPI 422 detail rendering. `r` pops back to the form (state intact).

use std::sync::Arc;

use crossterm::event::{KeyCode, KeyEvent};
use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::Paragraph;

use super::{Action, Screen};
use crate::http::ApiResponse;
use crate::spec::adapter::adapter_for;
use crate::tui::{AppCtx, AppMsg, SpecBundle, widgets};

enum State {
    Loading,
    Done(ApiResponse),
    Failed(String),
}

pub struct ResponseView {
    seq: u64,
    bundle: Arc<SpecBundle>,
    method: String,
    path: String,
    state: State,
    scroll: u16,
    show_headers: bool,
}

impl ResponseView {
    pub fn loading(seq: u64, bundle: Arc<SpecBundle>, method: String, path: String) -> Self {
        Self {
            seq,
            bundle,
            method,
            path,
            state: State::Loading,
            scroll: 0,
            show_headers: false,
        }
    }
}

impl Screen for ResponseView {
    fn title(&self) -> String {
        format!("{} / {} {}", self.bundle.project, self.method, self.path)
    }

    fn key_hints(&self) -> Vec<(&'static str, &'static str)> {
        match self.state {
            State::Loading => vec![("esc", "cancel")],
            _ => vec![
                ("↑↓", "scroll"),
                ("h", "headers"),
                ("r", "edit & resend"),
                ("esc", "back"),
            ],
        }
    }

    fn handle_key(&mut self, key: KeyEvent, _ctx: &mut AppCtx) -> Action {
        match key.code {
            KeyCode::Esc | KeyCode::Char('q') => Action::Pop,
            KeyCode::Char('r') => Action::Pop, // form below holds its state
            KeyCode::Char('h') => {
                self.show_headers = !self.show_headers;
                Action::None
            }
            KeyCode::Up | KeyCode::Char('k') => {
                self.scroll = self.scroll.saturating_sub(1);
                Action::None
            }
            KeyCode::Down | KeyCode::Char('j') => {
                self.scroll = self.scroll.saturating_add(1);
                Action::None
            }
            KeyCode::PageUp => {
                self.scroll = self.scroll.saturating_sub(20);
                Action::None
            }
            KeyCode::PageDown => {
                self.scroll = self.scroll.saturating_add(20);
                Action::None
            }
            _ => Action::None,
        }
    }

    fn handle_msg(&mut self, msg: &AppMsg, _ctx: &mut AppCtx) -> Action {
        if let AppMsg::Response {
            request_seq,
            result,
        } = msg
            && *request_seq == self.seq
        {
            self.state = match result {
                Ok(response) => State::Done(response.clone()),
                Err(message) => State::Failed(message.clone()),
            };
        }
        Action::None
    }

    fn draw(&mut self, frame: &mut Frame, area: Rect, ctx: &AppCtx) {
        let lines = match &self.state {
            State::Loading => vec![widgets::loading_line("response")],
            State::Failed(message) => vec![
                Line::from(Span::styled(
                    " request failed",
                    Style::new().fg(Color::Red).add_modifier(Modifier::BOLD),
                )),
                Line::raw(""),
                Line::from(Span::raw(format!(" {message}"))),
            ],
            State::Done(response) => self.render_response(response, ctx),
        };
        let paragraph = Paragraph::new(lines).scroll((self.scroll, 0));
        frame.render_widget(paragraph, area);
    }
}

impl ResponseView {
    fn render_response(&self, response: &ApiResponse, ctx: &AppCtx) -> Vec<Line<'static>> {
        let mut lines = Vec::new();
        lines.push(Line::from(vec![
            Span::styled(
                format!(" {} ", response.status),
                widgets::status_style(response.status),
            ),
            Span::raw(format!(" {} {}  ", response.method, response.url)),
            Span::styled(
                format!("{} ms", response.latency_ms),
                Style::new().fg(Color::Cyan),
            ),
        ]));

        // Framework-aware error rendering (FastAPI 422 detail).
        if response.status >= 400
            && let Some(project) = ctx.services.config.projects.get(&self.bundle.project)
            && let Some(error_lines) =
                adapter_for(project.framework).render_error_lines(response.status, &response.body)
        {
            lines.push(Line::raw(""));
            for error_line in error_lines {
                lines.push(Line::from(Span::styled(
                    format!("  ! {error_line}"),
                    Style::new().fg(Color::Yellow),
                )));
            }
        }

        if self.show_headers {
            lines.push(Line::raw(""));
            for (name, value) in &response.headers {
                lines.push(Line::from(vec![
                    Span::styled(format!("  {name}: "), Style::new().fg(Color::Cyan)),
                    Span::raw(value.clone()),
                ]));
            }
        }

        lines.push(Line::raw(""));
        let body_text = if response.body_is_json {
            serde_json::to_string_pretty(&response.body).unwrap_or_default()
        } else {
            response.body.as_str().unwrap_or("").to_string()
        };
        for raw_line in body_text.lines() {
            lines.push(render_json_line(raw_line));
        }
        lines
    }
}

/// Cheap depth-tinted JSON rendering: keys cyan, scalars by type.
fn render_json_line(line: &str) -> Line<'static> {
    if let Some((key_part, rest)) = line.split_once(':')
        && key_part.trim_start().starts_with('"')
    {
        return Line::from(vec![
            Span::styled(key_part.to_string(), Style::new().fg(Color::Cyan)),
            Span::raw(":"),
            Span::raw(rest.to_string()),
        ]);
    }
    Line::raw(line.to_string())
}
