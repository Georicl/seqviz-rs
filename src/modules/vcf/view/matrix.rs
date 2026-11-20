//! A matrix row is a VCF record and a column is an original sample.
use super::{Detail, Focus, Overlay, VcfView, layout, render::fit};
use crate::modules::vcf::palette;
use ratatui::{
    Frame,
    layout::Rect,
    style::{Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, Paragraph},
};

impl VcfView {
    /// Whether navigation targets the matrix instead of its detail panel.
    pub(super) fn matrix_focused(&self) -> bool {
        self.matrix && self.focus == Focus::Table
    }

    /// Current sample ordinal in the focused sample grid or detail list.
    pub(super) fn active_sample(&self) -> usize {
        if self.matrix_focused() {
            self.matrix_col
        } else {
            self.detail_row
        }
    }

    /// Number of samples in the current all/pinned projection.
    pub(super) fn sample_count(&self) -> usize {
        self.snapshot
            .as_ref()
            .map_or(0, |s| self.samples.len(s.header.samples.len()))
    }

    /// Keep enough room for a genotype even when the terminal is narrow.
    fn matrix_widths(&self) -> (usize, usize, usize) {
        let width = self.layout.rows.width as usize;
        let fixed = (width / 2).min(36);
        let cell = (width - fixed).clamp(1, 14);
        (fixed, cell, ((width - fixed) / cell).max(1))
    }

    /// First visible sample ordinal on the page containing the cursor.
    fn matrix_start(&self) -> usize {
        let (_, _, columns) = self.matrix_widths();
        self.matrix_col / columns * columns
    }

    /// Source sample indices required for the visible matrix page.
    pub(super) fn matrix_columns(&self) -> Vec<usize> {
        if !self.matrix {
            return Vec::new();
        }
        let total = self.snapshot.as_ref().map_or(0, |s| s.header.samples.len());
        self.samples
            .indices(total, self.matrix_start(), self.matrix_widths().2)
    }

    /// Move the cursor and request a new page when crossing its horizontal boundary.
    pub(super) fn set_matrix_column(&mut self, col: usize) {
        let old = self.matrix_start();
        self.matrix_col = col.min(self.sample_count().saturating_sub(1));
        self.dirty |= self.matrix && old != self.matrix_start();
    }

    /// Select the sample column under the pointer, preserving the frozen locus column.
    pub(super) fn select_matrix_x(&mut self, x: u16) {
        let (fixed, cell, _) = self.matrix_widths();
        let x = usize::from(x.saturating_sub(self.layout.rows.x));
        if x >= fixed {
            self.set_matrix_column(self.matrix_start() + (x - fixed) / cell);
        }
    }

    /// Explicit pinned cohort for GT calculations, independent of display scope.
    pub(super) fn af_samples(&self) -> Option<Vec<usize>> {
        self.af_pinned.then(|| self.samples.pinned_indices())
    }

    /// Switch AF denominator and refresh the active report or detail page.
    pub(super) fn toggle_af_scope(&mut self) {
        self.af_pinned = !self.af_pinned;
        if self.overlay == Some(Overlay::Statistics) {
            self.open_overlay(Overlay::Statistics);
        } else {
            self.change_detail(Detail::Af);
            self.dirty = true;
        }
    }

    /// Borrow frequency rows only for the currently selected source record and cohort.
    pub(super) fn af_rows(&self) -> &[String] {
        self.snapshot
            .as_ref()
            .filter(|s| {
                s.request.projection.show_af
                    && s.request.projection.af_samples == self.af_samples()
                    && self.entry().is_some()
            })
            .map_or(&[], |s| &s.frequencies)
    }
}

/// Render only rows and projected sample columns belonging to this viewport.
pub(super) fn draw(frame: &mut Frame, view: &VcfView) {
    let block = Block::default()
        .borders(Borders::ALL)
        .title(" Genotype matrix · M variants ");
    let inner = layout::inset(view.layout.table);
    frame.render_widget(block, view.layout.table);
    let Some(snapshot) = view.snapshot.as_ref() else {
        return;
    };
    let (fixed, cell, _) = view.matrix_widths();
    let columns = view.matrix_columns();
    let mut heading = fit(vec![Span::raw("LOCUS  REF>ALT")], fixed);
    for &i in &columns {
        heading.extend(fit(
            vec![Span::raw(snapshot.header.samples[i].clone())],
            cell,
        ));
    }
    frame.render_widget(
        Paragraph::new(Line::from(heading)),
        Rect::new(inner.x, inner.y, inner.width, inner.height.min(1)),
    );
    if snapshot.request.start != view.start
        || snapshot.request.query != view.query
        || snapshot.request.contig != view.contig
        || columns != snapshot.request.projection.matrix_columns
    {
        frame.render_widget(Paragraph::new("Loading matrix…"), view.layout.rows);
        return;
    }
    if columns.is_empty() || snapshot.rows.is_empty() {
        let message = if columns.is_empty() {
            "No samples in this view"
        } else {
            "No indexed matching records"
        };
        frame.render_widget(Paragraph::new(message), view.layout.rows);
        return;
    }
    let lines = snapshot
        .rows
        .iter()
        .zip(&snapshot.matrix)
        .map(|(row, values)| {
            let selected = row.ordinal == view.selected;
            let label = format!(
                "{}:{}  {}>{}",
                row.chrom, row.pos, row.reference, row.alternates
            );
            let mut spans = fit(vec![Span::raw(label)], fixed);
            for (offset, gt) in values.iter().enumerate() {
                let mut value = fit(palette::genotype(gt, &view.config, &view.theme), cell);
                if selected && view.matrix_start() + offset == view.matrix_col {
                    for span in &mut value {
                        span.style = span
                            .style
                            .bg(view.theme.selection)
                            .add_modifier(Modifier::UNDERLINED | Modifier::BOLD);
                    }
                }
                spans.extend(value);
            }
            let style = if selected {
                Style::default().add_modifier(Modifier::BOLD)
            } else {
                Style::default()
            };
            Line::from(spans).style(style)
        })
        .collect::<Vec<_>>();
    frame.render_widget(Paragraph::new(lines), view.layout.rows);
}
