//! Outer application regions; document layouts are owned by their pages.
use ratatui::layout::Rect;
#[derive(Default, Clone, Copy)]
pub(crate) struct Layout {
    pub area: Rect,
    pub tabs: Rect,
    pub body: Rect,
    pub footer: Rect,
    pub too_small: bool,
}
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
