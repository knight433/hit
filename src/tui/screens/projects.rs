//! Root screen: the registered projects menu.

use crossterm::event::{KeyCode, KeyEvent};
use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::style::Style;
use ratatui::text::{Line, Span};
use ratatui::widgets::{List, ListItem, ListState};

use super::{Action, Screen, move_selection, tags::TagList};
use crate::AppServices;
use crate::auth::AuthManager;
use crate::tui::{AppCtx, AppMsg, theme, widgets};

/// Clear the project's cached token (TUI counterpart of `hit logout`).
fn logout(project_name: &str, ctx: &mut AppCtx) {
    let result = (|| {
        let project = crate::config::project(&ctx.services.config, project_name)?;
        if project.auth.is_none() {
            ctx.set_status(format!("'{project_name}' has no auth configured"));
            return Ok(());
        }
        let store = crate::auth::new_token_store(
            ctx.services.settings().token_store,
            ctx.services.paths.token_dir.clone(),
        )
        .map_err(crate::error::HitError::from)?;
        store
            .clear(project_name)
            .map_err(crate::error::HitError::from)?;
        ctx.set_status(format!("logged out of '{project_name}'"));
        Ok::<_, crate::error::HitError>(())
    })();
    if let Err(e) = result {
        ctx.show_error(e.to_string());
    }
}

/// Authenticate now (prompting through TUI modals as needed) and report
/// the result on the status line.
fn login(project_name: String, ctx: &mut AppCtx) {
    let services = ctx.services.clone();
    let tx = ctx.tx.clone();
    let interactor = ctx.interactor();
    ctx.set_status(format!("logging in to '{project_name}'…"));
    tokio::spawn(async move {
        let result = async {
            let project = crate::config::project(&services.config, &project_name)
                .map_err(|e| e.to_string())?;
            let auth = AuthManager::for_project(
                &project_name,
                project,
                services.settings(),
                &services.paths,
                services.client.clone(),
                interactor,
                false,
            )
            .map_err(|e| e.to_string())?
            .ok_or_else(|| {
                format!(
                    "project '{project_name}' has no auth configured — add a \
                     [projects.{project_name}.auth] block to projects.toml"
                )
            })?;
            auth.invalidate().await;
            auth.bearer().await.map_err(|e| e.to_string())?;
            Ok::<_, String>(auth.cached_expiry())
        }
        .await;
        let msg = match result {
            Ok(Some(exp)) => {
                let remaining = exp.saturating_sub(crate::auth::token_store::now_unix());
                AppMsg::Notify(format!(
                    "logged in to '{project_name}' (token expires in {remaining}s)"
                ))
            }
            Ok(None) => AppMsg::Notify(format!("logged in to '{project_name}'")),
            Err(message) => AppMsg::Error(message),
        };
        let _ = tx.send(msg);
    });
}

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
            ("l", "login"),
            ("L", "logout"),
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
            KeyCode::Char('l') => {
                if let Some(name) = self.names.get(self.selected) {
                    login(name.clone(), ctx);
                }
                Action::None
            }
            // Shift+L (uppercase regardless of reported modifiers).
            KeyCode::Char('L') => {
                if let Some(name) = self.names.get(self.selected) {
                    logout(name, ctx);
                }
                Action::None
            }
            KeyCode::Esc => Action::Pop,
            _ => Action::None,
        }
    }

    fn draw(&mut self, frame: &mut Frame, area: Rect, ctx: &AppCtx) {
        if self.names.is_empty() {
            widgets::empty_state(
                frame,
                area,
                "no projects registered yet",
                "hit projects add <name> --base-url http://localhost:8000",
            );
            return;
        }

        let items: Vec<ListItem> = self
            .names
            .iter()
            .map(|name| {
                let project = &ctx.services.config.projects[name];
                let loaded = ctx.specs.contains_key(name);
                let auth = project.auth.as_ref().map(|a| a.type_name());
                let mut spans = vec![
                    Span::styled(
                        if loaded { "● " } else { "○ " },
                        Style::new().fg(if loaded { theme::GREEN } else { theme::DIM }),
                    ),
                    Span::styled(format!("{name:<24}"), theme::bold(theme::text())),
                    Span::styled(format!("{:<36}", project.base_url), theme::soft()),
                ];
                match auth {
                    Some(auth_type) => spans.push(Span::styled(
                        format!(" {auth_type} "),
                        Style::new().fg(theme::MAGENTA).bg(theme::SEL_BG),
                    )),
                    None => spans.push(Span::styled(" no auth ", theme::dim())),
                }
                ListItem::new(Line::from(spans))
            })
            .collect();

        let list = List::new(items)
            .highlight_style(theme::selected_row())
            .highlight_symbol(Span::styled("▌", theme::accent()));
        let mut state = ListState::default().with_selected(Some(self.selected));
        frame.render_stateful_widget(list, area, &mut state);
    }
}
