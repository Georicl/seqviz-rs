//! Outer application regions; document layouts are owned by their pages.
use ratatui::layout::Rect;
/// Terminal regions shared by rendering and application-level pointer hit testing.
#[derive(Default, Clone, Copy)]
pub(crate) struct Layout {
    /// Full terminal rectangle.
    pub area: Rect,
    /// Single-row tab bar.
    pub tabs: Rect,
    /// Region assigned to the active document page.
    pub body: Rect,
    /// Two rows for status, shortcuts or an input prompt.
    pub footer: Rect,
    /// Whether the terminal fails the minimum size required by the browser.
    pub too_small: bool,
}
/// Reserve one tab row and two footer rows, or flag terminals smaller than 30 × 8.
pub(crate) fn calculate(area: Rect) -> Layout {
    if area.width < 30 || area.height < 8 {
        return Layout {
            area,
            too_small: true,
            ..Default::default()
        };
    }
    Layout {
        area,
        tabs: Rect::new(area.x, area.y, area.width, 1),
        body: Rect::new(area.x, area.y + 1, area.width, area.height - 3),
        footer: Rect::new(area.x, area.bottom() - 2, area.width, 2),
        too_small: false,
    }
}
