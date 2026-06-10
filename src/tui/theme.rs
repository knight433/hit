//! Central color palette and shared style helpers (Tokyo Night-ish).
//! Every screen pulls from here so the UI stays visually consistent.

use ratatui::style::{Color, Modifier, Style};
use ratatui::text::Span;

// Palette
pub const FG: Color = Color::Rgb(0xc0, 0xca, 0xf5);
pub const FG_SOFT: Color = Color::Rgb(0xa9, 0xb1, 0xd6);
pub const DIM: Color = Color::Rgb(0x56, 0x5f, 0x89);
pub const ACCENT: Color = Color::Rgb(0x7a, 0xa2, 0xf7); // blue
pub const CYAN: Color = Color::Rgb(0x7d, 0xcf, 0xff);
pub const GREEN: Color = Color::Rgb(0x9e, 0xce, 0x6a);
pub const YELLOW: Color = Color::Rgb(0xe0, 0xaf, 0x68);
pub const ORANGE: Color = Color::Rgb(0xff, 0x9e, 0x64);
pub const RED: Color = Color::Rgb(0xf7, 0x76, 0x8e);
pub const MAGENTA: Color = Color::Rgb(0xbb, 0x9a, 0xf7);
pub const SEL_BG: Color = Color::Rgb(0x29, 0x2e, 0x42); // selection / chip bg
pub const BADGE_FG: Color = Color::Rgb(0x1a, 0x1b, 0x26); // dark text on bright badges

pub fn text() -> Style {
    Style::new().fg(FG)
}

pub fn soft() -> Style {
    Style::new().fg(FG_SOFT)
}

pub fn dim() -> Style {
    Style::new().fg(DIM)
}

pub fn accent() -> Style {
    Style::new().fg(ACCENT)
}

pub fn bold(style: Style) -> Style {
    style.add_modifier(Modifier::BOLD)
}

pub fn selected_row() -> Style {
    Style::new().bg(SEL_BG)
}

pub fn border() -> Style {
    Style::new().fg(SEL_BG).bg(Color::Reset).fg(DIM)
}

pub fn method_color(method: &str) -> Color {
    match method {
        "GET" => GREEN,
        "POST" => ACCENT,
        "PUT" => YELLOW,
        "PATCH" => MAGENTA,
        "DELETE" => RED,
        "HEAD" | "OPTIONS" => CYAN,
        _ => DIM,
    }
}

/// ` GET ` rendered as a colored badge.
pub fn method_badge(method: &str) -> Span<'static> {
    Span::styled(
        format!(" {method:^6} "),
        Style::new()
            .fg(BADGE_FG)
            .bg(method_color(method))
            .add_modifier(Modifier::BOLD),
    )
}

pub fn status_color(status: u16) -> Color {
    match status {
        200..=299 => GREEN,
        300..=399 => CYAN,
        400..=499 => YELLOW,
        _ => RED,
    }
}

/// ` 201 ` rendered as a colored badge.
pub fn status_badge(status: u16) -> Span<'static> {
    Span::styled(
        format!(" {status} "),
        Style::new()
            .fg(BADGE_FG)
            .bg(status_color(status))
            .add_modifier(Modifier::BOLD),
    )
}

/// Braille spinner frame for animated loading states.
pub fn spinner(frame: u64) -> char {
    const FRAMES: [char; 10] = ['⠋', '⠙', '⠹', '⠸', '⠼', '⠴', '⠦', '⠧', '⠇', '⠏'];
    FRAMES[(frame as usize) % FRAMES.len()]
}
