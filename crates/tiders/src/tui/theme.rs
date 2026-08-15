//! Tiders colour theme.
//!
//! A dark, ocean-tinted palette (fitting "Maré" = tide) in the spirit of
//! TokyoNight, with teal/cyan accents. Inspired by the theming approach in
//! xai-org/grok-build's fullscreen TUI.

use ratatui::style::{Color, Modifier, Style};

const fn rgb(r: u8, g: u8, b: u8) -> Color {
    Color::Rgb(r, g, b)
}

/// Primary accent (teal — the brand colour).
pub const ACCENT: Color = rgb(45, 212, 191);
/// Secondary accent (cyan).
pub const ACCENT2: Color = rgb(125, 207, 255);
/// Deep background.
pub const BG: Color = rgb(13, 17, 23);
/// Slightly raised surface (panels).
pub const SURFACE: Color = rgb(22, 27, 34);
/// Selected-row background.
pub const HIGHLIGHT_BG: Color = rgb(28, 41, 51);
/// Primary text.
pub const FG: Color = rgb(222, 226, 230);
/// Secondary/dim text.
pub const DIM: Color = rgb(120, 132, 144);
/// Chrome/borders.
pub const BORDER: Color = rgb(48, 56, 65);
/// Success / playing.
pub const GREEN: Color = rgb(158, 206, 106);
/// Warning / accents.
pub const YELLOW: Color = rgb(224, 175, 104);
/// Error.
pub const RED: Color = rgb(247, 118, 142);
/// Explicit badge / magenta.
pub const MAGENTA: Color = rgb(187, 154, 247);

/// Base text style.
pub fn base() -> Style {
    Style::default().fg(FG)
}

/// Dim/secondary text style.
pub fn dim() -> Style {
    Style::default().fg(DIM)
}

/// Accent text style (bold teal).
pub fn accent() -> Style {
    Style::default().fg(ACCENT).add_modifier(Modifier::BOLD)
}

/// Border style for panels.
pub fn border() -> Style {
    Style::default().fg(BORDER)
}

/// Border style for the focused panel.
pub fn border_focused() -> Style {
    Style::default().fg(ACCENT)
}

/// Selected list-row style.
pub fn selected() -> Style {
    Style::default()
        .fg(ACCENT)
        .bg(HIGHLIGHT_BG)
        .add_modifier(Modifier::BOLD)
}
