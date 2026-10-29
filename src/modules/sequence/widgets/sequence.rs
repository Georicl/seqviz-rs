//! Render an indexed sequence window with residue colours, selection and quality rows.
use crate::modules::sequence::{data::SeqType, palette::ContentPalette, protocol::Snapshot};
use crate::widgets::theme::Theme;
use ratatui::{
    Frame,
    layout::Rect,
    style::{Modifier, Style},
    text::{Line, Span},
    widgets::Paragraph,
};
/// Borrowed data, geometry and UI state needed to draw a sequence window.
pub(crate) struct Props<'a> {
    /// Loaded record metadata and sequence bytes for the current viewport generation.
    pub snapshot: &'a Snapshot,
    /// Shared colours for text, selection and focus.
    pub theme: &'a Theme,
    /// Residue and Phred-score colours for sequence characters.
    pub palette: &'a ContentPalette,
    /// Sequence rectangle, including its coordinate gutter.
    pub area: Rect,
    /// Number of base rows that fit after accounting for quality rows.
    pub sequence_rows: usize,
    /// Maximum sequence characters in each base row.
    pub bases_per_line: usize,
    /// Terminal columns reserved for the one-based coordinate label.
    pub gutter: u16,
    /// Terminal rows per base row: one normally, two with aligned quality characters.
    pub stride: u16,
    /// Requested zero-based record ID, checked against the loaded snapshot.
    pub record: u64,
    /// Requested zero-based sequence offset, checked against the loaded window.
    pub start: u64,
    /// Zero-based sequence coordinate of the keyboard cursor.
    pub cursor: u64,
    /// Inclusive zero-based endpoints; dragging may reverse their order.
    pub selection: Option<(u64, u64)>,
    /// Whether to emphasise the cursor in the active sequence pane.
    pub focused: bool,
}

/// Draw only a window matching the requested record/start, using one-based labels for zero-based data.
pub(crate) fn draw(frame: &mut Frame, props: Props<'_>) {
    let snapshot = props.snapshot;
    let Some(meta) = &snapshot.record else {
        return;
    };
    let rect = props.area;
    if !meta.has_sequence {
        frame.render_widget(Paragraph::new("Sequence: missing"), rect);
        return;
    }
    if snapshot.window.start != props.start || snapshot.request.record != props.record {
        return;
    }
    for row in 0..props.sequence_rows {
        let offset = row * props.bases_per_line;
        if offset >= snapshot.window.sequence.len() {
            break;
        }
        let length = props
            .bases_per_line
            .min(snapshot.window.sequence.len() - offset);
        let position = props.start + offset as u64;
        let y = rect.y + row as u16 * props.stride;
        let number = format!(
            "{:>width$} ",
            position + 1,
            width = props.gutter.saturating_sub(1) as usize
        );
        frame.render_widget(
            Paragraph::new(number).style(Style::default().fg(props.theme.muted)),
            Rect::new(rect.x, y, props.gutter, 1),
        );
        let mut spans = Vec::with_capacity(length);
        let mut quality = Vec::with_capacity(length);
        for index in offset..offset + length {
            let byte = snapshot.window.sequence[index];
            let score = snapshot.window.quality.get(index).copied();
            let color = if let Some(q) = score {
                props.palette.quality_color(q.saturating_sub(33))
            } else if meta.seq_type == SeqType::Protein {
                props.palette.amino_acid_color(byte)
            } else {
                props.palette.nucleotide_color(byte)
            };
            let mut style = Style::default().fg(color);
            let at = props.start + index as u64;
            if props
                .selection
                .is_some_and(|(a, b)| at >= a.min(b) && at <= a.max(b))
            {
                style = style.bg(props.theme.selection);
            }
            if props.focused && at == props.cursor {
                style = style.add_modifier(Modifier::UNDERLINED | Modifier::BOLD);
            }
            spans.push(Span::styled(char::from(byte).to_string(), style));
            if let Some(q) = score {
                quality.push(Span::styled(
                    char::from(q).to_string(),
                    Style::default().fg(props.theme.muted),
                ));
            }
        }
        frame.render_widget(
            Paragraph::new(Line::from(spans)),
            Rect::new(
                rect.x + props.gutter,
                y,
                rect.width.saturating_sub(props.gutter),
                1,
            ),
        );
        if props.stride == 2 {
            frame.render_widget(
                Paragraph::new(Line::from(quality)),
                Rect::new(
                    rect.x + props.gutter,
                    y + 1,
                    rect.width.saturating_sub(props.gutter),
                    1,
                ),
            );
        }
    }
}
