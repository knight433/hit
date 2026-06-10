//! Response viewer: status, latency, headers (toggle), pretty body, and
//! FastAPI 422 detail rendering. `r` pops back to the form (state intact).

use std::sync::Arc;

use crossterm::event::{KeyCode, KeyEvent};
use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::Paragraph;

use super::{Action, Screen};
use crate::http::ApiResponse;
use crate::spec::adapter::adapter_for;
use crate::tui::{AppCtx, AppMsg, SpecBundle, theme, widgets};

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
        format!(
            "projects ▸ {} ▸ {} {} ▸ response",
            self.bundle.project, self.method, self.path
        )
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
            State::Loading => vec![Line::raw(""), widgets::loading_line("response", ctx.frame)],
            State::Failed(message) => vec![
                Line::raw(""),
                Line::from(Span::styled(
                    " ✗ request failed",
                    Style::new().fg(theme::RED).add_modifier(Modifier::BOLD),
                )),
                Line::raw(""),
                Line::from(Span::styled(format!("   {message}"), theme::soft())),
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
            theme::status_badge(response.status),
            Span::raw(" "),
            theme::method_badge(&response.method),
            Span::styled(format!(" {}", response.url), theme::bold(theme::text())),
            Span::styled(format!("  ⏱ {} ms", response.latency_ms), theme::dim()),
        ]));

        // Framework-aware error rendering (FastAPI 422 detail).
        if response.status >= 400
            && let Some(project) = ctx.services.config.projects.get(&self.bundle.project)
            && let Some(error_lines) =
                adapter_for(project.framework).render_error_lines(response.status, &response.body)
        {
            lines.push(Line::raw(""));
            for error_line in error_lines {
                lines.push(Line::from(vec![
                    Span::styled("  ✗ ", Style::new().fg(theme::RED)),
                    Span::styled(error_line, Style::new().fg(theme::YELLOW)),
                ]));
            }
        }

        if self.show_headers {
            lines.push(Line::raw(""));
            lines.push(Line::from(Span::styled("  ── headers ──", theme::dim())));
            for (name, value) in &response.headers {
                lines.push(Line::from(vec![
                    Span::styled(format!("  {name}"), Style::new().fg(theme::CYAN)),
                    Span::styled(": ", theme::dim()),
                    Span::styled(value.clone(), theme::soft()),
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
            if response.body_is_json {
                lines.push(widgets::colorize_json_line(raw_line));
            } else {
                lines.push(Line::from(Span::styled(
                    raw_line.to_string(),
                    theme::soft(),
                )));
            }
        }
        lines
    }
}
