//! A file-agnostic tab bar with the browser's existing paging geometry.
use super::theme::Theme;
use ratatui::{
    Frame,
    layout::Rect,
    style::{Modifier, Style},
    widgets::Paragraph,
};

/// Place up to 20-column tab cells after the logo, keeping the active tab visible.
pub fn layout(area: Rect, open: &[usize], active: usize) -> Vec<(usize, Rect)> {
    let active_index = open.iter().position(|id| *id == active).unwrap_or(0);
    let visible = (area.width.saturating_sub(4) / 20).max(1) as usize;
    let start = active_index.saturating_sub(visible.saturating_sub(1));
    let mut x = area.x.saturating_add(4);
    open.iter()
        .skip(start)
        .take(visible)
        .map(|&id| {
            let rect = Rect::new(x, area.y, 20.min(area.right().saturating_sub(x)), 1);
            x = x.saturating_add(rect.width);
            (id, rect)
        })
        .collect()
}

/// Render file labels with stable one-based tab numbers and an active-tab highlight.
pub fn draw(
    frame: &mut Frame,
    labels: &[(usize, String)],
    active: usize,
    area: Rect,
    theme: &Theme,
) {
    frame.render_widget(
        Paragraph::new(" sq ").style(
            Style::default()
                .fg(theme.focus)
                .add_modifier(Modifier::BOLD),
        ),
        Rect::new(area.x, area.y, area.width.min(4), area.height.min(1)),
    );
    let open: Vec<_> = labels.iter().map(|(id, _)| *id).collect();
    for (id, rect) in layout(area, &open, active) {
        let name = &labels
            .iter()
            .find(|(candidate, _)| *candidate == id)
            .unwrap()
            .1;
        let style = if id == active {
            Style::default()
                .fg(theme.focus)
                .bg(theme.selection)
                .add_modifier(Modifier::BOLD)
        } else {
            Style::default().fg(theme.muted)
        };
        frame.render_widget(
            Paragraph::new(format!(" {}:{} ", id + 1, name)).style(style),
            rect,
        );
    }
}

/// Return the tab whose visible rectangle contains the given terminal cell.
pub fn hit(layout: &[(usize, Rect)], x: u16, y: u16) -> Option<usize> {
    layout
        .iter()
        .find(|(_, area)| x >= area.x && x < area.right() && y >= area.y && y < area.bottom())
        .map(|(id, _)| *id)
}
