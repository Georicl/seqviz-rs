//! Terminal geometry and coordinate mapping. This module performs no I/O.
use ratatui::layout::Rect;

/// Prepared terminal rectangles and sequence-to-cell geometry for rendering and hit testing.
#[derive(Default)]
pub(crate) struct Layout {
    /// Complete document body rectangle supplied by the application shell.
    pub area: Rect,
    /// Record-list pane including its border.
    pub left: Rect,
    /// Sequence pane including its border.
    pub right: Rect,
    /// Inner record-list rectangle used for row rendering and pointer selection.
    pub list: Rect,
    /// Sequence-pane interior before reserving the record-details rows.
    pub sequence_inner: Rect,
    /// Base and quality rows including the coordinate gutter.
    pub sequence: Rect,
    /// Number of rows reserved for record metadata above the sequence.
    pub details: u16,
    /// Sequence characters available per row after the gutter.
    pub bases_per_line: usize,
    /// Number of complete base rows, excluding paired quality rows.
    pub sequence_rows: usize,
    /// Terminal rows occupied by a base row and its optional quality row.
    pub stride: u16,
    /// Columns reserved for one-based sequence positions.
    pub gutter: u16,
    /// Inner rectangle available to an information-page overlay.
    pub info: Rect,
    /// Whether the body is below the minimum supported dimensions.
    pub too_small: bool,
}

/// Record metadata and display settings that determine sequence row geometry.
pub(crate) struct SequenceLayout {
    /// Currently indexed sequence length, used to size the coordinate gutter.
    pub known_length: Option<u64>,
    /// Zero-based sequence offset requested at the top of the viewport.
    pub start: u64,
    /// Whether the current record supplies quality characters.
    pub has_quality: bool,
    /// Whether the user has enabled aligned quality-character rows.
    pub show_quality: bool,
}

/// Allocate record and sequence panes, reserving coordinate and optional quality-row space.
pub(crate) fn calculate(area: Rect, view: SequenceLayout) -> Layout {
    if area.width < 30 || area.height < 5 {
        return Layout {
            area,
            too_small: true,
            ..Layout::default()
        };
    }
    let body = area;
    let left_width = (area.width / 4).clamp(12, 32);
    let left = Rect::new(body.x, body.y, left_width, body.height);
    let right = Rect::new(left.right(), body.y, body.width - left_width, body.height);
    let list = inset(left);
    let sequence_inner = inset(right);
    let stride = if view.has_quality && view.show_quality {
        2
    } else {
        1
    };
    let details = sequence_inner.height.min(4);
    let sequence = Rect::new(
        sequence_inner.x,
        sequence_inner.y + details,
        sequence_inner.width,
        sequence_inner.height - details,
    );
    let max_position = view
        .known_length
        .map_or(1, |length| length.max(view.start.saturating_add(1)));
    let gutter =
        (max_position.to_string().len() + 2).min(sequence.width.saturating_sub(1) as usize) as u16;
    let bases_per_line = sequence.width.saturating_sub(gutter).max(1) as usize;
    let sequence_rows = sequence.height as usize / stride as usize;
    Layout {
        area,
        left,
        right,
        list,
        sequence_inner,
        sequence,
        details,
        bases_per_line,
        sequence_rows,
        stride,
        gutter,
        info: inset(body),
        too_small: false,
    }
}

/// Remove a one-cell border from a rectangle with saturating inner dimensions.
fn inset(area: Rect) -> Rect {
    Rect::new(
        area.x + 1,
        area.y + 1,
        area.width.saturating_sub(2),
        area.height.saturating_sub(2),
    )
}

impl Layout {
    /// Map a pointer to the target viewport, independently of loaded window data.
    /// Selection endpoints are zero-based sequence coordinates; gutters are excluded.
    pub fn sequence_position(&self, start: u64, known_length: u64, x: u16, y: u16) -> Option<u64> {
        if self.sequence.width == 0
            || self.sequence_rows == 0
            || known_length == 0
            || x < self.sequence.x + self.gutter
        {
            return None;
        }
        let row = (y.saturating_sub(self.sequence.y) / self.stride.max(1))
            .min(self.sequence_rows.saturating_sub(1) as u16);
        let column = x
            .saturating_sub(self.sequence.x + self.gutter)
            .min(self.bases_per_line.saturating_sub(1) as u16);
        Some(
            start
                .saturating_add(row as u64 * self.bases_per_line as u64)
                .saturating_add(u64::from(column))
                .min(known_length - 1),
        )
    }
}
