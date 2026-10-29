//! Terminal geometry shared by the VCF renderer and input hit testing.
use ratatui::layout::Rect;

/// Prepared terminal rectangles shared by rendering and pointer hit testing.
#[derive(Clone, Copy, Default)]
pub(super) struct Layout {
    /// Entire document content rectangle.
    pub area: Rect,
    /// Version, counts and current-locus lines.
    pub header: Rect,
    /// Variant table including its border and column heading.
    pub table: Rect,
    /// Variant data rows, excluding border and column heading.
    pub rows: Rect,
    /// Detail panel including its border, tabs and heading.
    pub details: Rect,
    /// Single row of INFO, Samples and Raw tab labels.
    pub detail_tabs: Rect,
    /// Column headings for the active detail representation.
    pub detail_heading: Rect,
    /// Visible detail text and pointer-selection area.
    pub detail_body: Rect,
}

/// Remove a one-cell border, saturating widths and heights for tiny terminals.
pub(super) fn inset(area: Rect) -> Rect {
    Rect::new(
        area.x.saturating_add(1),
        area.y.saturating_add(1),
        area.width.saturating_sub(2),
        area.height.saturating_sub(2),
    )
}

/// Reserve header/table space and show details only when enough rows remain.
pub(super) fn calculate(area: Rect, detail_rows: u16) -> Layout {
    let header_height = area.height.min(2);
    let remaining = area.height.saturating_sub(header_height);
    let detail_height = if remaining >= 9 {
        detail_rows.max(5).min(remaining.saturating_sub(4))
    } else {
        0
    };
    let table = Rect::new(
        area.x,
        area.y.saturating_add(header_height),
        area.width,
        remaining.saturating_sub(detail_height),
    );
    let table_inner = inset(table);
    let details = Rect::new(area.x, table.bottom(), area.width, detail_height);
    let detail_inner = inset(details);
    let tabs_height = detail_inner.height.min(1);
    let heading_height = detail_inner.height.saturating_sub(tabs_height).min(1);
    Layout {
        area,
        header: Rect::new(area.x, area.y, area.width, header_height),
        table,
        rows: Rect::new(
            table_inner.x,
            table_inner.y.saturating_add(table_inner.height.min(1)),
            table_inner.width,
            table_inner.height.saturating_sub(1),
        ),
        details,
        detail_tabs: Rect::new(
            detail_inner.x,
            detail_inner.y,
            detail_inner.width,
            tabs_height,
        ),
        detail_heading: Rect::new(
            detail_inner.x,
            detail_inner.y.saturating_add(tabs_height),
            detail_inner.width,
            heading_height,
        ),
        detail_body: Rect::new(
            detail_inner.x,
            detail_inner.y.saturating_add(tabs_height + heading_height),
            detail_inner.width,
            detail_inner
                .height
                .saturating_sub(tabs_height + heading_height),
        ),
    }
}

/// Test membership using inclusive left/top and exclusive right/bottom boundaries.
pub(super) fn contains(area: Rect, x: u16, y: u16) -> bool {
    x >= area.x && x < area.right() && y >= area.y && y < area.bottom()
}
