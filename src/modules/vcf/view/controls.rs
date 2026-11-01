//! Commit record queries and reset viewport state when their result order changes.
use super::VcfView;
use crate::modules::vcf::query::{Query, Sort, SortField};

impl VcfView {
    /// Replace the record query after validation, leaving the current query intact on error.
    pub(super) fn apply_filter(&mut self, text: &str) -> Result<(), String> {
        let query = Query::parse(text)?;
        self.query = query;
        self.filter_text = text.to_owned();
        self.reset_query_position();
        Ok(())
    }

    /// Cycle deterministic record orders without changing the active predicates.
    pub(super) fn cycle_order(&mut self) {
        let orders = [
            Sort {
                field: SortField::Source,
                descending: false,
            },
            Sort {
                field: SortField::Position,
                descending: false,
            },
            Sort {
                field: SortField::Position,
                descending: true,
            },
            Sort {
                field: SortField::Qual,
                descending: true,
            },
            Sort {
                field: SortField::Qual,
                descending: false,
            },
            Sort {
                field: SortField::Type,
                descending: false,
            },
        ];
        let current = orders
            .iter()
            .position(|sort| *sort == self.query.sort)
            .unwrap_or(0);
        self.query.sort = orders[(current + 1) % orders.len()];
        self.filter_text = self.query.summary();
        self.reset_query_position();
    }

    /// Reset result ordinals and transient operations when a filter or contig changes.
    pub(super) fn reset_query_position(&mut self) {
        self.cancel_copy();
        self.cancel_statistics();
        self.statistics = None;
        self.statistics_error = None;
        self.locating = None;
        // A new result order invalidates both ordinal positions and replies to the old order.
        self.token += 1;
        self.anchor = None;
        self.selected = 0;
        self.start = 0;
        self.reset_detail_position();
        self.dirty = true;
        self.redraw = true;
        self.message.clear();
    }

    /// Clear text selections before changing the logical detail-row mapping.
    pub(super) fn reset_detail_position(&mut self) {
        self.detail_row = 0;
        self.detail_start = 0;
        self.horizontal = 0;
        self.selection = None;
        self.dragging = false;
        self.drag_anchor = None;
        self.redraw = true;
    }
}
