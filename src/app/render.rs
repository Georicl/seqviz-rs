//! Compose application chrome around the active document page.
use super::state::App;
use crate::widgets::{input, status_bar, tabs};
use ratatui::{
    Frame,
    style::Style,
    widgets::{Block, Paragraph},
};
pub(crate) fn draw(frame: &mut Frame, app: &App) {
    let theme = &app.config.theme;
    let layout = app.hits;
    frame.render_widget(
        Block::default().style(Style::default().bg(theme.background).fg(theme.foreground)),
        layout.area,
    );
    if layout.too_small {
        frame.render_widget(
            Paragraph::new("Resize terminal to at least 30 × 8. q quits."),
            layout.area,
        );
        return;
    }
    tabs::draw(frame, &app.labels(), app.active, layout.tabs, theme);
    app.view().draw(frame);
    let status = app.view().status();
    status_bar::draw(frame, layout.footer, &status.message, &status.hints, theme);
    if let Some(prompt) = app.view().input_prompt() {
        input::draw(frame, layout.footer, prompt.label, prompt.text, theme);
    }
}
