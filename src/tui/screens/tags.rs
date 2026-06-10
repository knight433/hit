//! Tag list for a project. Starts in a loading state until the spec arrives.

use std::sync::Arc;

use crossterm::event::{KeyCode, KeyEvent};
use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::style::Style;
use ratatui::text::{Line, Span};
use ratatui::widgets::{List, ListItem, ListState};

use super::{Action, Screen, endpoints::EndpointList, move_selection};
use crate::tui::{AppCtx, AppMsg, SpecBundle, theme, widgets};

pub struct TagList {
    project: String,
    bundle: Option<Arc<SpecBundle>>,
    selected: usize,
}

impl TagList {
    pub fn loading(project: String) -> Self {
        Self {
            project,
            bundle: None,
            selected: 0,
        }
    }
}

impl Screen for TagList {
    fn title(&self) -> String {
        format!("projects ▸ {}", self.project)
    }

    fn meta(&self) -> Option<String> {
        self.bundle.as_ref().map(|bundle| {
            format!(
                "{} v{} · {}",
                bundle.spec.title,
                bundle.spec.version,
                origin_label(bundle.origin)
            )
        })
    }

    fn key_hints(&self) -> Vec<(&'static str, &'static str)> {
        vec![("↑↓", "select"), ("enter", "open"), ("esc", "back")]
    }

    fn handle_key(&mut self, key: KeyEvent, ctx: &mut AppCtx) -> Action {
        // The spec may already be cached from an earlier visit.
        if self.bundle.is_none()
            && let Some(bundle) = ctx.specs.get(&self.project)
        {
            self.bundle = Some(bundle.clone());
        }
        let Some(bundle) = &self.bundle else {
            return match key.code {
                KeyCode::Esc => Action::Pop,
                _ => Action::None,
            };
        };
        let len = bundle.spec.tags.len();
        match key.code {
            KeyCode::Up | KeyCode::Char('k') => {
                move_selection(&mut self.selected, len, -1);
                Action::None
            }
            KeyCode::Down | KeyCode::Char('j') => {
                move_selection(&mut self.selected, len, 1);
                Action::None
            }
            KeyCode::Enter => match bundle.spec.tags.get(self.selected) {
                Some(tag) => Action::Push(Box::new(EndpointList::new(
                    bundle.clone(),
                    Some(tag.name.clone()),
                ))),
                None => Action::None,
            },
            KeyCode::Esc => Action::Pop,
            _ => Action::None,
        }
    }

    fn handle_msg(&mut self, msg: &AppMsg, ctx: &mut AppCtx) -> Action {
        if let AppMsg::SpecLoaded { project, result } = msg
            && project == &self.project
        {
            match result {
                Ok(bundle) => self.bundle = Some(bundle.clone()),
                Err(message) => {
                    ctx.show_error(format!("failed to load spec: {message}"));
                    return Action::Pop;
                }
            }
        }
        Action::None
    }

    fn draw(&mut self, frame: &mut Frame, area: Rect, ctx: &AppCtx) {
        if self.bundle.is_none()
            && let Some(bundle) = ctx.specs.get(&self.project)
        {
            // Picked up between events (e.g. first draw after cache hit).
            self.bundle = Some(bundle.clone());
        }
        let Some(bundle) = &self.bundle else {
            frame.render_widget(
                ratatui::widgets::Paragraph::new(widgets::loading_line("openapi.json", ctx.frame)),
                area,
            );
            return;
        };

        let items: Vec<ListItem> = bundle
            .spec
            .tags
            .iter()
            .map(|tag| {
                ListItem::new(Line::from(vec![
                    Span::styled("◆ ", theme::accent()),
                    Span::styled(format!("{:<24}", tag.name), theme::bold(theme::text())),
                    Span::styled(
                        format!("{:>3} ", tag.endpoint_ids.len()),
                        Style::new().fg(theme::CYAN),
                    ),
                    Span::styled("endpoints   ", theme::dim()),
                    Span::styled(tag.description.clone().unwrap_or_default(), theme::dim()),
                ]))
            })
            .collect();

        let list = List::new(items)
            .highlight_style(theme::selected_row())
            .highlight_symbol(Span::styled("▌", theme::accent()));
        let mut state = ListState::default().with_selected(Some(self.selected));
        frame.render_stateful_widget(list, area, &mut state);
    }
}

fn origin_label(origin: crate::spec::SpecOrigin) -> &'static str {
    match origin {
        crate::spec::SpecOrigin::Live => "live",
        crate::spec::SpecOrigin::Cache => "cached",
        crate::spec::SpecOrigin::StaleCache => "stale cache",
        crate::spec::SpecOrigin::File => "spec file",
    }
}
