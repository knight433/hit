//! The screen stack abstraction: each screen handles keys and async results,
//! and draws itself. Navigation is push/pop driven by returned `Action`s.

pub mod endpoints;
pub mod form;
pub mod projects;
pub mod response;
pub mod tags;

use crossterm::event::KeyEvent;
use ratatui::Frame;
use ratatui::layout::Rect;

use super::{AppCtx, AppMsg};

pub enum Action {
    None,
    Push(Box<dyn Screen>),
    Pop,
    Quit,
    /// Suspend the TUI and open $EDITOR on `seed`; the result comes back via
    /// `handle_editor_result`.
    RunEditor {
        seed: String,
    },
}

pub trait Screen {
    /// Breadcrumb segments separated by " ▸ " (rendered in the header).
    fn title(&self) -> String;
    /// Right-aligned header info (e.g. spec title/version/origin).
    fn meta(&self) -> Option<String> {
        None
    }
    /// Footer key hints, e.g. [("enter", "open"), ("q", "quit")].
    fn key_hints(&self) -> Vec<(&'static str, &'static str)>;
    fn handle_key(&mut self, key: KeyEvent, ctx: &mut AppCtx) -> Action;
    /// React to async results (spec loads, responses). Default: ignore.
    fn handle_msg(&mut self, _msg: &AppMsg, _ctx: &mut AppCtx) -> Action {
        Action::None
    }
    /// Receive the edited text after a `RunEditor` action (None = unchanged
    /// or editor failed). Default: ignore.
    fn handle_editor_result(&mut self, _text: Option<String>, _ctx: &mut AppCtx) -> Action {
        Action::None
    }
    fn draw(&mut self, frame: &mut Frame, area: Rect, ctx: &AppCtx);
}

/// Shared list-navigation helper.
pub fn move_selection(selected: &mut usize, len: usize, delta: i64) {
    if len == 0 {
        return;
    }
    let new = (*selected as i64 + delta).clamp(0, len as i64 - 1);
    *selected = new as usize;
}
