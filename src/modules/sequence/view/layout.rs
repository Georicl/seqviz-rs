//! Terminal geometry and coordinate mapping. This module performs no I/O.
use ratatui::layout::Rect;

#[derive(Default)]
pub(crate) struct Layout {
    pub area: Rect,
    pub left: Rect,
    pub right: Rect,
    pub list: Rect,
    pub sequence_inner: Rect,
    pub sequence: Rect,
    pub details: u16,
    pub bases_per_line: usize,
    pub sequence_rows: usize,
    pub stride: u16,
    pub gutter: u16,
    pub info: Rect,
    pub too_small: bool,
}

pub(crate) struct SequenceLayout {
    pub known_length: Option<u64>,
    pub start: u64,
    pub has_quality: bool,
    pub show_quality: bool,
}

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
