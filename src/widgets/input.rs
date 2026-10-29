//! One-line input rendering and editing, independent of the input's meaning.
use super::theme::Theme;
use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use ratatui::{
    Frame,
    layout::Rect,
    style::Style,
    widgets::{Clear, Paragraph},
};
use unicode_width::UnicodeWidthStr;

/// Draw one input line and position the cursor using Unicode display width.
pub fn draw(frame: &mut Frame, area: Rect, label: &str, text: &str, theme: &Theme) {
    if area.width == 0 || area.height == 0 {
        return;
    }
    let line = Rect::new(area.x, area.y, area.width, 1);
    frame.render_widget(Clear, line);
    frame.render_widget(
        Paragraph::new(format!("{label}{text}")).style(Style::default().fg(theme.focus)),
        line,
    );
    let cursor = (label.width() + text.width()).min(area.width.saturating_sub(1) as usize) as u16;
    frame.set_cursor_position((area.x + cursor, area.y));
}

/// Append ordinary characters or remove the last character on Backspace.
pub fn edit(text: &mut String, key: KeyEvent) {
    match key.code {
        KeyCode::Backspace => {
            text.pop();
        }
        KeyCode::Char(c) if !key.modifiers.contains(KeyModifiers::CONTROL) => text.push(c),
        _ => {}
    }
}
