//! Render caller-provided status and shortcuts without interpreting their contents.
use super::theme::Theme;
use ratatui::{Frame, layout::Rect, style::Style, text::Line, widgets::Paragraph};

/// Render the status line followed by muted keyboard hints.
///
/// # Arguments
///
/// - `frame`: Terminal frame into which the current content is drawn.
/// - `area`: Terminal rectangle available for this layout or component.
/// - `status`: Status message rendered on the first footer row.
/// - `hints`: Keyboard hints rendered on the second footer row.
/// - `theme`: Shared application colours for text, borders, focus and selection.
pub fn draw(frame: &mut Frame, area: Rect, status: &str, hints: &str, theme: &Theme) {
    frame.render_widget(
        Paragraph::new(vec![
            Line::from(status),
            Line::styled(hints, Style::default().fg(theme.muted)),
        ]),
        area,
    );
}
