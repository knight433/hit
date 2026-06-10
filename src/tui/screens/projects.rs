//! Root screen: the registered projects menu.

use crossterm::event::{KeyCode, KeyEvent};
use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{List, ListItem, ListState, Paragraph};

use super::{Action, Screen, move_selection, tags::TagList};
use crate::AppServices;
use crate::tui::AppCtx;

pub struct ProjectList {
    names: Vec<String>,
    selected: usize,
}

impl ProjectList {
    pub fn new(services: &AppServices) -> Self {
        Self {
            names: services.config.projects.keys().cloned().collect(),
            selected: 0,
        }
    }
}

impl Screen for ProjectList {
    fn title(&self) -> String {
        "projects".into()
    }

    fn key_hints(&self) -> Vec<(&'static str, &'static str)> {
        vec![
            ("↑↓", "select"),
            ("enter", "open"),
            ("r", "reload spec"),
            ("q", "quit"),
        ]
    }

    fn handle_key(&mut self, key: KeyEvent, ctx: &mut AppCtx) -> Action {
        match key.code {
            KeyCode::Up | KeyCode::Char('k') => {
                move_selection(&mut self.selected, self.names.len(), -1);
                Action::None
            }
            KeyCode::Down | KeyCode::Char('j') => {
                move_selection(&mut self.selected, self.names.len(), 1);
                Action::None
            }
            KeyCode::Enter => {
                let Some(name) = self.names.get(self.selected) else {
                    return Action::None;
                };
                if !ctx.specs.contains_key(name) {
                    ctx.load_spec(name);
                }
                Action::Push(Box::new(TagList::loading(name.clone())))
            }
            KeyCode::Char('r') => {
                if let Some(name) = self.names.get(self.selected) {
                    ctx.specs.remove(name);
                    ctx.set_status(format!("spec cache for '{name}' dropped"));
                }
                Action::None
            }
            KeyCode::Esc => Action::Pop,
            _ => Action::None,
        }
    }

    fn draw(&mut self, frame: &mut Frame, area: Rect, ctx: &AppCtx) {
        if self.names.is_empty() {
            let help = Paragraph::new(vec![
                Line::raw(""),
                Line::raw("  no projects registered."),
                Line::raw(""),
                Line::from(Span::styled(
                    "  hit projects add <name> --base-url http://localhost:8000",
                    Style::new().fg(Color::Cyan),
                )),
            ]);
            frame.render_widget(help, area);
            return;
        }

        let items: Vec<ListItem> = self
            .names
            .iter()
            .map(|name| {
                let project = &ctx.services.config.projects[name];
                let auth = project
                    .auth
                    .as_ref()
                    .map_or("no auth".to_string(), |a| a.type_name().to_string());
                let loaded = if ctx.specs.contains_key(name) {
                    "●"
                } else {
                    " "
                };
                ListItem::new(Line::from(vec![
                    Span::styled(format!(" {loaded} "), Style::new().fg(Color::Green)),
                    Span::styled(
                        format!("{name:<24}"),
                        Style::new().add_modifier(Modifier::BOLD),
                    ),
                    Span::raw(format!("{}  ", project.base_url)),
                    Span::styled(format!("[{auth}]"), Style::new().fg(Color::DarkGray)),
                ]))
            })
            .collect();

        let list = List::new(items)
            .highlight_style(Style::new().bg(Color::Rgb(40, 40, 60)))
            .highlight_symbol("▶");
        let mut state = ListState::default().with_selected(Some(self.selected));
        frame.render_stateful_widget(list, area, &mut state);
    }
}
