//! The palette and glyphs the interface is drawn with.
//!
//! One Dark, taken from the colours the shell is already themed with, so the
//! inbox does not look like a visitor in its own terminal.

use ratatui::style::{Color, Modifier, Style};
use ratatui::text::Span;
use ratatui::widgets::{Block, BorderType, Borders};

pub const FG: Color = Color::Rgb(0xab, 0xb2, 0xbf);
pub const BLUE: Color = Color::Rgb(0x61, 0xaf, 0xef);
pub const CYAN: Color = Color::Rgb(0x56, 0xb6, 0xc2);
pub const PURPLE: Color = Color::Rgb(0xc6, 0x78, 0xdd);
pub const GREEN: Color = Color::Rgb(0x98, 0xc3, 0x79);
pub const RED: Color = Color::Rgb(0xe0, 0x6c, 0x75);
pub const YELLOW: Color = Color::Rgb(0xe5, 0xc0, 0x7b);
pub const ORANGE: Color = Color::Rgb(0xd1, 0x9a, 0x66);
/// Comment grey — labels and chrome.
pub const MUTED: Color = Color::Rgb(0x5c, 0x63, 0x70);
/// Gutter grey — a step brighter than MUTED, for secondary text.
pub const DIM: Color = Color::Rgb(0x63, 0x6d, 0x83);
pub const SELECTION: Color = Color::Rgb(0x3e, 0x44, 0x52);

/// The bar marking the selected row. A background alone is easy to lose track
/// of when the row is mostly padding.
pub const CURSOR: &str = "▌";

pub fn label() -> Style {
    Style::default().fg(MUTED)
}

pub fn dim() -> Style {
    Style::default().fg(DIM)
}

pub fn body() -> Style {
    Style::default().fg(FG)
}

pub fn selected() -> Style {
    Style::default().bg(SELECTION).add_modifier(Modifier::BOLD)
}

/// A pane. The focused one is outlined in blue so the eye knows where the keys
/// are going.
pub fn pane(title: &str, focused: bool) -> Block<'static> {
    let accent = if focused { BLUE } else { MUTED };
    Block::default()
        .borders(Borders::ALL)
        .border_type(BorderType::Rounded)
        .border_style(Style::default().fg(accent))
        .title(Span::styled(
            format!(" {title} "),
            Style::default().fg(accent).add_modifier(Modifier::BOLD),
        ))
}

/// Nerd Font marks for the package sources. The manifest installs the fonts
/// these come from; a terminal without them shows a box, not a broken layout.
pub fn source_glyph(id: &str) -> &'static str {
    match id {
        "all" => "",
        "brew" | "cask" | "tap" => "",
        "mas" => "",
        "app" => "",
        "go" => "",
        "cargo" => "",
        "npm" => "",
        "uv" => "",
        "vscode" => "",
        "claude" => "",
        "ollama" => "",
        _ => "",
    }
}
