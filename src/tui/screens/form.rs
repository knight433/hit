//! The request form screen: navigate flattened rows, edit values inline,
//! Shift+X to null/exclude, `e` for $EDITOR, Ctrl+S to send.

use std::sync::Arc;

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use ratatui::Frame;
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::Paragraph;
use serde_json::Value;

use super::{Action, Screen, response::ResponseView};
use crate::auth::AuthManager;
use crate::config;
use crate::http::{RequestArgs, RequestExecutor};
use crate::model::Endpoint;
use crate::tui::form::{FormState, RowKind, RowState};
use crate::tui::{AppCtx, AppMsg, SpecBundle, theme};

pub struct RequestForm {
    bundle: Arc<SpecBundle>,
    endpoint: Endpoint,
    form: FormState,
    /// Inline editor state: (row, text). Some = editing.
    editor: Option<(usize, String)>,
    scroll: usize,
}

impl RequestForm {
    pub fn new(bundle: Arc<SpecBundle>, endpoint: Endpoint) -> Self {
        let form = FormState::new(&endpoint);
        Self {
            bundle,
            endpoint,
            form,
            editor: None,
            scroll: 0,
        }
    }

    fn submit(&mut self, ctx: &mut AppCtx) -> Action {
        let serialized = match self.form.serialize() {
            Err(e) => {
                self.form.cursor = e.row;
                ctx.set_status(e.message);
                return Action::None;
            }
            Ok(s) => s,
        };

        ctx.request_seq += 1;
        let seq = ctx.request_seq;
        let services = ctx.services.clone();
        let tx = ctx.tx.clone();
        let interactor = ctx.interactor();
        let project_name = self.bundle.project.clone();
        let endpoint = self.endpoint.clone();
        let args = RequestArgs {
            path_params: serialized.path_params,
            query_params: serialized.query_params,
            headers: serialized.headers,
            body: serialized.body,
            no_auth: false,
        };

        tokio::spawn(async move {
            let result = async {
                let project =
                    config::project(&services.config, &project_name).map_err(|e| e.to_string())?;
                let auth = AuthManager::for_project(
                    &project_name,
                    project,
                    services.settings(),
                    &services.paths,
                    services.client.clone(),
                    interactor,
                    false,
                )
                .map_err(|e| e.to_string())?;
                let executor = RequestExecutor {
                    client: &services.client,
                    project,
                    auth: auth.as_ref(),
                };
                executor
                    .execute(&endpoint, &args)
                    .await
                    .map_err(|e| e.to_string())
            }
            .await;
            let _ = tx.send(AppMsg::Response {
                request_seq: seq,
                result,
            });
        });

        Action::Push(Box::new(ResponseView::loading(
            seq,
            self.bundle.clone(),
            self.endpoint.method.clone(),
            self.endpoint.path.clone(),
        )))
    }

    fn handle_editor_key(&mut self, key: KeyEvent, ctx: &mut AppCtx) -> Action {
        let (row, mut text) = self.editor.take().expect("editor checked by caller");
        match key.code {
            KeyCode::Esc => {}
            KeyCode::Enter => {
                if let Err(message) = self.form.commit_text(row, &text) {
                    ctx.set_status(message);
                    self.editor = Some((row, text));
                }
            }
            KeyCode::Backspace => {
                text.pop();
                self.editor = Some((row, text));
            }
            KeyCode::Char(c) => {
                text.push(c);
                self.editor = Some((row, text));
            }
            _ => self.editor = Some((row, text)),
        }
        Action::None
    }

    /// Seed JSON for external editing: current body, leniently serialized.
    fn editor_seed(&self) -> String {
        let body = self.form.body_for_editing();
        serde_json::to_string_pretty(&body).unwrap_or_else(|_| "{}".to_string())
    }
}

impl Screen for RequestForm {
    fn title(&self) -> String {
        format!(
            "projects ▸ {} ▸ {} {}",
            self.bundle.project, self.endpoint.method, self.endpoint.path
        )
    }

    fn meta(&self) -> Option<String> {
        self.endpoint.summary.clone()
    }

    fn key_hints(&self) -> Vec<(&'static str, &'static str)> {
        if self.editor.is_some() {
            vec![("enter", "commit"), ("esc", "cancel")]
        } else {
            vec![
                ("enter", "edit/toggle"),
                ("X", "null/exclude"),
                ("a/d", "array +/-"),
                ("e", "$EDITOR"),
                ("ctrl+s", "send"),
                ("esc", "back"),
            ]
        }
    }

    fn handle_key(&mut self, key: KeyEvent, ctx: &mut AppCtx) -> Action {
        if self.editor.is_some() {
            return self.handle_editor_key(key, ctx);
        }

        let cursor = self.form.cursor;
        let has_rows = !self.form.rows.is_empty();

        if key.modifiers.contains(KeyModifiers::CONTROL) && key.code == KeyCode::Char('s') {
            return self.submit(ctx);
        }

        match key.code {
            KeyCode::Up => self.form.move_cursor(-1),
            KeyCode::Down => self.form.move_cursor(1),
            KeyCode::Char('k') => self.form.move_cursor(-1),
            KeyCode::Char('j') => self.form.move_cursor(1),
            KeyCode::Enter if has_rows => {
                let kind = self.form.rows[cursor].kind.clone();
                match kind {
                    RowKind::Scalar | RowKind::RawJson => {
                        self.editor = Some((cursor, self.form.text_of(cursor)));
                    }
                    RowKind::Bool
                    | RowKind::Enum(_)
                    | RowKind::ObjectHeader
                    | RowKind::ArrayHeader => self.form.toggle(cursor),
                    _ => {}
                }
            }
            KeyCode::Char(' ') if has_rows => self.form.toggle(cursor),
            KeyCode::Left | KeyCode::Right if has_rows => {
                if matches!(self.form.rows[cursor].kind, RowKind::Enum(_)) {
                    self.form.toggle(cursor);
                }
            }
            // Shift+X arrives as 'X' (modifier flags vary by terminal).
            KeyCode::Char('X') if has_rows => {
                if let Some(hint) = self.form.cycle_exclusion(cursor) {
                    ctx.set_status(hint);
                }
            }
            KeyCode::Char('x') if has_rows => self.form.reinclude(cursor),
            KeyCode::Char('a') if has_rows => {
                // Append to the array at/above the cursor.
                if self.form.rows[cursor].kind == RowKind::ArrayHeader {
                    self.form.array_append(cursor);
                }
            }
            KeyCode::Char('d') if has_rows => self.form.array_delete(cursor),
            KeyCode::Tab if has_rows => self.form.toggle(cursor),
            KeyCode::Char('e') if self.endpoint.body.is_some() => {
                return Action::RunEditor {
                    seed: self.editor_seed(),
                };
            }
            KeyCode::Esc => return Action::Pop,
            _ => {}
        }
        Action::None
    }

    fn handle_editor_result(&mut self, text: Option<String>, ctx: &mut AppCtx) -> Action {
        if let Some(text) = text {
            match serde_json::from_str::<Value>(&text) {
                Ok(value) => {
                    self.form.hydrate_body(&self.endpoint, &value);
                    ctx.set_status("body updated from editor");
                }
                Err(e) => ctx.show_error(format!("editor result is not valid JSON: {e}")),
            }
        }
        Action::None
    }

    fn draw(&mut self, frame: &mut Frame, area: Rect, _ctx: &AppCtx) {
        let [list_area, info_area] =
            Layout::vertical([Constraint::Min(1), Constraint::Length(2)]).areas(area);

        let hidden = self.form.hidden_mask();
        let visible: Vec<usize> = (0..self.form.rows.len()).filter(|&i| !hidden[i]).collect();

        // Keep cursor in view.
        let cursor_pos = visible
            .iter()
            .position(|&i| i == self.form.cursor)
            .unwrap_or(0);
        let height = list_area.height as usize;
        if cursor_pos < self.scroll {
            self.scroll = cursor_pos;
        } else if height > 0 && cursor_pos >= self.scroll + height {
            self.scroll = cursor_pos + 1 - height;
        }

        // Label column width: longest visible label (incl. indent), clamped.
        let label_width = visible
            .iter()
            .map(|&i| {
                let row = &self.form.rows[i];
                row.depth as usize * 2 + row.label.len() + 1
            })
            .max()
            .unwrap_or(16)
            .clamp(16, 34);

        let lines: Vec<Line> = visible
            .iter()
            .skip(self.scroll)
            .take(height)
            .map(|&i| self.render_row(i, label_width, list_area.width))
            .collect();
        frame.render_widget(Paragraph::new(lines), list_area);

        // Info strip: cursor row name · type · flags — description.
        if let Some(row) = self.form.rows.get(self.form.cursor) {
            let mut spans = vec![
                Span::styled(format!(" {} ", row.label), theme::bold(theme::text())),
                Span::styled(format!("· {} ", row.kind_label), theme::dim()),
            ];
            if row.required {
                spans.push(Span::styled("· required ", Style::new().fg(theme::RED)));
            }
            if row.nullable {
                spans.push(Span::styled("· nullable ", Style::new().fg(theme::MAGENTA)));
            }
            if let Some(description) = &row.description {
                spans.push(Span::styled(format!("— {description}"), theme::soft()));
            }
            let rule = Line::from(Span::styled(
                "─".repeat(info_area.width as usize),
                Style::new().fg(theme::SEL_BG),
            ));
            frame.render_widget(Paragraph::new(vec![rule, Line::from(spans)]), info_area);
        }
    }
}

impl RequestForm {
    fn render_row(&self, i: usize, label_width: usize, width: u16) -> Line<'_> {
        let row = &self.form.rows[i];
        let is_cursor = i == self.form.cursor;

        if row.kind == RowKind::SectionHeader {
            let label = format!("╴{}╶", row.label);
            let fill = (width as usize).saturating_sub(label.len() + 3);
            return Line::from(vec![
                Span::styled("──", Style::new().fg(theme::SEL_BG)),
                Span::styled(label, theme::bold(theme::accent())),
                Span::styled("─".repeat(fill), Style::new().fg(theme::SEL_BG)),
            ]);
        }

        let mut spans = vec![if is_cursor {
            Span::styled("▌ ", theme::accent())
        } else {
            Span::raw("  ")
        }];

        // Label cell, padded to the shared column width.
        let indent = "  ".repeat(row.depth as usize);
        let marker = if row.required { "*" } else { " " };
        let label_text = format!("{indent}{}{marker}", row.label);
        let padded = format!("{label_text:<label_width$}  ");
        let mut label_spans = vec![
            Span::styled(
                format!("{indent}{}", row.label),
                if is_cursor {
                    theme::bold(theme::text())
                } else {
                    theme::soft()
                },
            ),
            Span::styled(
                if row.required { "*" } else { " " },
                Style::new().fg(theme::RED),
            ),
        ];
        let pad = padded.len().saturating_sub(label_text.len());
        label_spans.push(Span::raw(" ".repeat(pad)));
        spans.extend(label_spans);

        // Inline editor takes over the value cell.
        if let Some((edit_row, text)) = &self.editor
            && *edit_row == i
        {
            spans.push(Span::styled(
                format!("{text}▏"),
                Style::new().fg(theme::YELLOW),
            ));
            return Line::from(spans);
        }

        let value_span = match (&row.state, &row.kind) {
            (RowState::Excluded, _) => Span::styled(
                "⊘ excluded",
                theme::dim().add_modifier(Modifier::CROSSED_OUT),
            ),
            (RowState::Null, _) => Span::styled(
                "∅ null",
                Style::new()
                    .fg(theme::MAGENTA)
                    .add_modifier(Modifier::ITALIC),
            ),
            (RowState::Empty, _) => {
                Span::styled("‹empty›", theme::dim().add_modifier(Modifier::ITALIC))
            }
            (RowState::Filled(_), RowKind::ObjectHeader) => Span::styled(
                if row.collapsed {
                    "{ … } collapsed"
                } else {
                    "{"
                },
                Style::new().fg(theme::CYAN),
            ),
            (RowState::Filled(_), RowKind::ArrayHeader) => Span::styled(
                format!("[ {} ]", self.array_len(i)),
                Style::new().fg(theme::CYAN),
            ),
            (RowState::Filled(v), RowKind::Enum(_)) => Span::styled(
                format!("◂ {} ▸", value_text(v)),
                Style::new().fg(theme::CYAN),
            ),
            (RowState::Filled(v), RowKind::Const) => {
                Span::styled(format!("{} ⚷ fixed", value_text(v)), theme::dim())
            }
            (RowState::Filled(v), _) => Span::styled(value_text(v), value_style(v)),
        };
        spans.push(value_span);

        spans.push(Span::styled(format!("  {}", row.kind_label), theme::dim()));
        Line::from(spans)
    }

    fn array_len(&self, header: usize) -> String {
        let depth = self.form.rows[header].depth + 1;
        let count = (header + 1..self.form.span_end(header))
            .filter(|&j| self.form.rows[j].depth == depth)
            .count();
        format!("{count} item{}", if count == 1 { "" } else { "s" })
    }
}

fn value_text(value: &Value) -> String {
    match value {
        Value::String(s) => s.clone(),
        other => other.to_string(),
    }
}

/// Color scalar values by JSON type (mirrors the response-body coloring).
fn value_style(value: &Value) -> Style {
    match value {
        Value::String(_) => Style::new().fg(theme::GREEN),
        Value::Number(_) => Style::new().fg(theme::ORANGE),
        Value::Bool(_) | Value::Null => Style::new().fg(theme::MAGENTA),
        _ => theme::text(),
    }
}
