//! Endpoint list for a tag (or the whole project), with `/` filtering.

use std::sync::Arc;

use crossterm::event::{KeyCode, KeyEvent};
use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::layout::{Constraint, Layout};
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::Paragraph;
use ratatui::widgets::{List, ListItem, ListState};

use super::{Action, Screen, form::RequestForm, move_selection};
use crate::tui::{AppCtx, SpecBundle, theme, widgets};

pub struct EndpointList {
    bundle: Arc<SpecBundle>,
    tag: Option<String>,
    selected: usize,
    filter: String,
    filtering: bool,
}

impl EndpointList {
    pub fn new(bundle: Arc<SpecBundle>, tag: Option<String>) -> Self {
        Self {
            bundle,
            tag,
            selected: 0,
            filter: String::new(),
            filtering: false,
        }
    }

    /// Indices into `bundle.spec.endpoints` after tag + filter narrowing.
    fn visible(&self) -> Vec<usize> {
        let needle = self.filter.to_ascii_lowercase();
        self.bundle
            .spec
            .endpoints
            .iter()
            .enumerate()
            .filter(|(_, e)| match &self.tag {
                Some(tag) => {
                    e.tags.iter().any(|t| t == tag) || (e.tags.is_empty() && tag == "untagged")
                }
                None => true,
            })
            .filter(|(_, e)| {
                needle.is_empty()
                    || e.id.to_ascii_lowercase().contains(&needle)
                    || e.path.to_ascii_lowercase().contains(&needle)
                    || e.summary
                        .as_deref()
                        .unwrap_or("")
                        .to_ascii_lowercase()
                        .contains(&needle)
            })
            .map(|(i, _)| i)
            .collect()
    }
}

impl Screen for EndpointList {
    fn title(&self) -> String {
        let scope = self.tag.as_deref().unwrap_or("all endpoints");
        format!("projects ▸ {} ▸ {scope}", self.bundle.project)
    }

    fn meta(&self) -> Option<String> {
        Some(format!("{} endpoints", self.visible().len()))
    }

    fn key_hints(&self) -> Vec<(&'static str, &'static str)> {
        if self.filtering {
            vec![("type", "filter"), ("enter", "apply"), ("esc", "clear")]
        } else {
            vec![
                ("↑↓", "select"),
                ("enter", "open"),
                ("/", "filter"),
                ("esc", "back"),
            ]
        }
    }

    fn handle_key(&mut self, key: KeyEvent, _ctx: &mut AppCtx) -> Action {
        if self.filtering {
            match key.code {
                KeyCode::Esc => {
                    self.filter.clear();
                    self.filtering = false;
                }
                KeyCode::Enter => self.filtering = false,
                KeyCode::Backspace => {
                    self.filter.pop();
                }
                KeyCode::Char(c) => self.filter.push(c),
                _ => {}
            }
            self.selected = 0;
            return Action::None;
        }

        let visible = self.visible();
        match key.code {
            KeyCode::Up | KeyCode::Char('k') => {
                move_selection(&mut self.selected, visible.len(), -1);
                Action::None
            }
            KeyCode::Down | KeyCode::Char('j') => {
                move_selection(&mut self.selected, visible.len(), 1);
                Action::None
            }
            KeyCode::Char('/') => {
                self.filtering = true;
                Action::None
            }
            KeyCode::Enter => match visible.get(self.selected) {
                Some(&idx) => {
                    let endpoint = self.bundle.spec.endpoints[idx].clone();
                    Action::Push(Box::new(RequestForm::new(self.bundle.clone(), endpoint)))
                }
                None => Action::None,
            },
            KeyCode::Esc => Action::Pop,
            _ => Action::None,
        }
    }

    fn draw(&mut self, frame: &mut Frame, area: Rect, _ctx: &AppCtx) {
        let mut list_area = area;

        // Search bar (visible while typing or when a filter is applied).
        if self.filtering || !self.filter.is_empty() {
            let [search, rest] =
                Layout::vertical([Constraint::Length(1), Constraint::Min(1)]).areas(area);
            list_area = rest;
            let mut spans = vec![
                Span::styled(" / ", Style::new().fg(theme::CYAN).bg(theme::SEL_BG)),
                Span::raw(" "),
                Span::styled(self.filter.clone(), Style::new().fg(theme::YELLOW)),
            ];
            if self.filtering {
                spans.push(Span::styled("▏", Style::new().fg(theme::YELLOW)));
            } else {
                spans.push(Span::styled("  (esc on / clears)", theme::dim()));
            }
            frame.render_widget(Paragraph::new(Line::from(spans)), search);
        }

        let visible = self.visible();
        if visible.is_empty() {
            widgets::empty_state(frame, list_area, "no endpoints match", "press / to refine");
            return;
        }

        let items: Vec<ListItem> = visible
            .iter()
            .map(|&idx| {
                let e = &self.bundle.spec.endpoints[idx];
                let mut spans = vec![
                    theme::method_badge(&e.method),
                    Span::raw(" "),
                    Span::styled(format!("{:<36}", e.path), theme::bold(theme::text())),
                    Span::styled(
                        e.summary.clone().unwrap_or_else(|| e.id.clone()),
                        theme::dim(),
                    ),
                ];
                if e.auth_required {
                    spans.push(Span::styled("  ⚿", Style::new().fg(theme::YELLOW)));
                }
                if e.deprecated {
                    spans.push(Span::styled(
                        "  deprecated",
                        Style::new()
                            .fg(theme::RED)
                            .add_modifier(Modifier::CROSSED_OUT),
                    ));
                }
                ListItem::new(Line::from(spans))
            })
            .collect();

        let list = List::new(items)
            .highlight_style(theme::selected_row())
            .highlight_symbol(Span::styled("▌", theme::accent()));
        let mut state = ListState::default().with_selected(Some(self.selected));
        frame.render_stateful_widget(list, list_area, &mut state);
    }
}
