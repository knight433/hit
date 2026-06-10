//! Interactive TUI: tokio event loop, screen stack, async task plumbing.
//!
//! Logs go to a file (set up in main); nothing here may write to
//! stdout/stderr while the alternate screen is active.

pub mod form;
pub mod screens;
pub mod widgets;

use std::collections::HashMap;
use std::sync::Arc;

use crossterm::event::{Event, EventStream, KeyCode, KeyEvent, KeyEventKind};
use futures::StreamExt;
use ratatui::Frame;
use ratatui::layout::{Constraint, Layout};
use tokio::sync::mpsc;

use crate::http::ApiResponse;
use crate::spec::SpecOrigin;
use crate::{AppServices, config, model::ApiSpec, spec};
use screens::{Action, Screen};

/// A loaded, shareable spec bundle.
pub struct SpecBundle {
    pub project: String,
    pub spec: ApiSpec,
    pub origin: SpecOrigin,
}

/// Results of async work, sent back into the event loop.
pub enum AppMsg {
    SpecLoaded {
        project: String,
        result: Result<Arc<SpecBundle>, String>,
    },
    Response {
        request_seq: u64,
        result: Result<ApiResponse, String>,
    },
    Error(String),
}

/// Shared context handed to screens: services, async spawning, modals.
pub struct AppCtx {
    pub services: Arc<AppServices>,
    pub tx: mpsc::UnboundedSender<AppMsg>,
    pub specs: HashMap<String, Arc<SpecBundle>>,
    pub modal: Option<Modal>,
    pub status: Option<String>,
    /// Monotonic id matching in-flight requests to Response messages.
    pub request_seq: u64,
}

impl AppCtx {
    pub fn show_error(&mut self, message: impl Into<String>) {
        self.modal = Some(Modal {
            title: "error".into(),
            body: message.into(),
        });
    }

    pub fn set_status(&mut self, message: impl Into<String>) {
        self.status = Some(message.into());
    }

    /// Kick off a spec load for a project; result arrives as `SpecLoaded`.
    pub fn load_spec(&mut self, project_name: &str) {
        let services = self.services.clone();
        let tx = self.tx.clone();
        let name = project_name.to_string();
        tokio::spawn(async move {
            let result = match config::project(&services.config, &name) {
                Ok(project) => spec::load(
                    &services.client,
                    &name,
                    project,
                    services.settings(),
                    &services.paths.spec_cache_dir,
                    false,
                )
                .await
                .map(|loaded| {
                    Arc::new(SpecBundle {
                        project: name.clone(),
                        spec: loaded.spec,
                        origin: loaded.origin,
                    })
                })
                .map_err(|e| e.to_string()),
                Err(e) => Err(e.to_string()),
            };
            let _ = tx.send(AppMsg::SpecLoaded {
                project: name,
                result,
            });
        });
    }
}

pub struct Modal {
    pub title: String,
    pub body: String,
}

/// TUI entry point; returns the process exit code.
pub async fn run(services: AppServices, initial_project: Option<String>) -> i32 {
    // Restore the terminal even if we panic mid-draw — non-negotiable.
    let default_hook = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        ratatui::restore();
        default_hook(info);
    }));

    let mut terminal = ratatui::init();
    let result = event_loop(&mut terminal, services, initial_project).await;
    ratatui::restore();

    match result {
        Ok(()) => 0,
        Err(e) => {
            eprintln!("tui error: {e}");
            1
        }
    }
}

async fn event_loop(
    terminal: &mut ratatui::DefaultTerminal,
    services: AppServices,
    initial_project: Option<String>,
) -> Result<(), Box<dyn std::error::Error>> {
    let (tx, mut rx) = mpsc::unbounded_channel();
    let mut ctx = AppCtx {
        services: Arc::new(services),
        tx,
        specs: HashMap::new(),
        modal: None,
        status: None,
        request_seq: 0,
    };

    let mut stack: Vec<Box<dyn Screen>> =
        vec![Box::new(screens::projects::ProjectList::new(&ctx.services))];

    // `hit tui <project>` jumps straight into the project.
    if let Some(name) = initial_project {
        if ctx.services.config.projects.contains_key(&name) {
            ctx.load_spec(&name);
            stack.push(Box::new(screens::tags::TagList::loading(name)));
        } else {
            ctx.show_error(format!("unknown project '{name}'"));
        }
    }

    let mut events = EventStream::new();

    loop {
        terminal.draw(|frame| draw(frame, &mut stack, &ctx))?;

        let action = tokio::select! {
            event = events.next() => match event {
                Some(Ok(Event::Key(key))) if key.kind != KeyEventKind::Release => {
                    handle_key(key, &mut stack, &mut ctx)
                }
                Some(Err(e)) => return Err(e.into()),
                None => return Ok(()),
                _ => Action::None, // resize triggers redraw; mouse ignored
            },
            msg = rx.recv() => match msg {
                Some(msg) => handle_msg(msg, &mut stack, &mut ctx),
                None => return Ok(()),
            },
        };

        let mut action = action;
        loop {
            match action {
                Action::None => break,
                Action::Push(screen) => {
                    stack.push(screen);
                    break;
                }
                Action::Pop => {
                    stack.pop();
                    if stack.is_empty() {
                        return Ok(());
                    }
                    break;
                }
                Action::Quit => return Ok(()),
                Action::RunEditor { seed } => {
                    let text = run_external_editor(terminal, &seed);
                    let top = stack.last_mut().expect("stack is never empty");
                    action = top.handle_editor_result(text, &mut ctx);
                }
            }
        }
    }
}

/// Suspend the TUI, run $EDITOR on the seed text, and return the edited text
/// (None when unchanged or anything failed — failures land in the log).
fn run_external_editor(terminal: &mut ratatui::DefaultTerminal, seed: &str) -> Option<String> {
    let editor = std::env::var("VISUAL")
        .or_else(|_| std::env::var("EDITOR"))
        .unwrap_or_else(|_| "vi".to_string());
    let path = std::env::temp_dir().join(format!("hitpoint-body-{}.json", std::process::id()));
    if let Err(e) = std::fs::write(&path, seed) {
        tracing::warn!(error = %e, "failed to write editor temp file");
        return None;
    }

    let run = || -> std::io::Result<Option<String>> {
        crossterm::terminal::disable_raw_mode()?;
        crossterm::execute!(std::io::stdout(), crossterm::terminal::LeaveAlternateScreen)?;

        // $EDITOR may carry arguments ("code -w").
        let mut parts = editor.split_whitespace();
        let program = parts.next().unwrap_or("vi");
        let status = std::process::Command::new(program)
            .args(parts)
            .arg(&path)
            .status();

        crossterm::execute!(std::io::stdout(), crossterm::terminal::EnterAlternateScreen)?;
        crossterm::terminal::enable_raw_mode()?;

        match status {
            Ok(s) if s.success() => Ok(Some(std::fs::read_to_string(&path)?)),
            Ok(s) => {
                tracing::info!(status = ?s.code(), "editor exited non-zero; discarding");
                Ok(None)
            }
            Err(e) => {
                tracing::warn!(error = %e, editor, "failed to launch editor");
                Ok(None)
            }
        }
    };

    let result = tokio::task::block_in_place(run);
    let _ = std::fs::remove_file(&path);
    let _ = terminal.clear(); // force a full repaint after the alt-screen round trip

    match result {
        Ok(text) => {
            let text = text?;
            (text.trim() != seed.trim()).then_some(text)
        }
        Err(e) => {
            tracing::warn!(error = %e, "terminal suspend/resume failed");
            None
        }
    }
}

fn handle_key(key: KeyEvent, stack: &mut Vec<Box<dyn Screen>>, ctx: &mut AppCtx) -> Action {
    ctx.status = None;
    // Modal swallows the next key.
    if ctx.modal.is_some() {
        ctx.modal = None;
        return Action::None;
    }
    let top = stack.last_mut().expect("stack is never empty");
    let action = top.handle_key(key, ctx);
    if matches!(action, Action::None)
        && let KeyCode::Char('q') = key.code
        && stack.len() == 1
    {
        // 'q' quits from the root screen unless the screen consumed it.
        return Action::Quit;
    }
    action
}

fn handle_msg(msg: AppMsg, stack: &mut [Box<dyn Screen>], ctx: &mut AppCtx) -> Action {
    if let AppMsg::SpecLoaded { project, result } = &msg
        && let Ok(bundle) = result
    {
        ctx.specs.insert(project.clone(), bundle.clone());
    }
    if let AppMsg::Error(message) = &msg {
        ctx.show_error(message.clone());
        return Action::None;
    }
    let top = stack.last_mut().expect("stack is never empty");
    top.handle_msg(&msg, ctx)
}

fn draw(frame: &mut Frame, stack: &mut [Box<dyn Screen>], ctx: &AppCtx) {
    let [header, body, footer] = Layout::vertical([
        Constraint::Length(1),
        Constraint::Min(1),
        Constraint::Length(1),
    ])
    .areas(frame.area());

    let top = stack.last_mut().expect("stack is never empty");
    widgets::draw_header(frame, header, &top.title());
    top.draw(frame, body, ctx);
    widgets::draw_footer(frame, footer, &top.key_hints(), ctx.status.as_deref());

    if let Some(modal) = &ctx.modal {
        widgets::draw_modal(frame, modal);
    }
}
