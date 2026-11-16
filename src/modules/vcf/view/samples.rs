//! Sample selection and FORMAT projection, independent of parsing and terminal input.

use std::collections::BTreeSet;

/// View preferences expressed in source sample indices and FORMAT identifiers.
#[derive(Debug, Default)]
pub(super) struct SampleNavigation {
    /// Original sample columns retained as favourites in source order.
    pinned: BTreeSet<usize>,
    /// Whether the sample page contains only pinned source columns.
    pinned_only: bool,
    /// Selected FORMAT identifiers in display order; empty means all current fields.
    fields: Vec<String>,
}

impl SampleNavigation {
    /// Count sample rows without copying the header's sample-name list.
    pub(super) fn len(&self, total: usize) -> usize {
        if self.pinned_only {
            self.pinned.range(..total).count()
        } else {
            total
        }
    }

    /// Return the source sample column corresponding to a visible zero-based row.
    pub(super) fn source_index(&self, total: usize, row: usize) -> Option<usize> {
        if self.pinned_only {
            self.pinned.range(..total).nth(row).copied()
        } else {
            (row < total).then_some(row)
        }
    }

    /// Locate a source sample column in the current visible row order.
    fn row_of(&self, total: usize, source: usize) -> Option<usize> {
        if source >= total {
            return None;
        }
        if self.pinned_only {
            self.pinned
                .range(..total)
                .position(|&index| index == source)
        } else {
            Some(source)
        }
    }

    /// Collect only the requested page of source sample indices.
    pub(super) fn indices(&self, total: usize, start: usize, count: usize) -> Vec<usize> {
        if self.pinned_only {
            self.pinned
                .range(..total)
                .skip(start)
                .take(count)
                .copied()
                .collect()
        } else {
            (start.min(total)..start.saturating_add(count).min(total)).collect()
        }
    }

    /// Find a sample-name substring from an inclusive visible row, wrapping once.
    /// Empty input has no match; matching is Unicode lowercase insensitive.
    pub(super) fn find(&self, names: &[String], query: &str, after_row: usize) -> Option<usize> {
        let query = query.trim().to_lowercase();
        let count = self.len(names.len());
        if query.is_empty() || count == 0 {
            return None;
        }
        let start = after_row % count;
        if self.pinned_only {
            let rows = || self.pinned.range(..names.len()).copied().enumerate();
            rows()
                .skip(start)
                .chain(rows().take(start))
                .find_map(|(row, source)| {
                    names[source].to_lowercase().contains(&query).then_some(row)
                })
        } else {
            (start..count)
                .chain(0..start)
                .find(|&row| names[row].to_lowercase().contains(&query))
        }
    }

    /// Report whether an original sample column is pinned.
    pub(super) fn is_pinned(&self, source: usize) -> bool {
        self.pinned.contains(&source)
    }

    /// Report whether the page is restricted to pinned samples.
    pub(super) fn pinned_only(&self) -> bool {
        self.pinned_only
    }

    /// Count explicitly pinned source sample columns.
    pub(super) fn pinned_count(&self) -> usize {
        self.pinned.len()
    }

    /// Toggle the current row's pin and return the remaining selected visible row.
    /// Removing a row in pinned-only mode selects its successor, or the final row.
    pub(super) fn toggle_pin(&mut self, total: usize, row: usize) -> Option<usize> {
        let source = self.source_index(total, row)?;
        if !self.pinned.remove(&source) {
            self.pinned.insert(source);
        }
        let count = self.len(total);
        self.row_of(total, source)
            .or_else(|| (count > 0).then(|| row.min(count - 1)))
    }

    /// Toggle pinned-only mode while retaining the selected source sample when visible.
    /// Otherwise select the nearest following pinned sample, or the final pinned row.
    pub(super) fn toggle_pinned_only(&mut self, total: usize, row: usize) -> usize {
        let source = self.source_index(total, row);
        self.pinned_only = !self.pinned_only;
        let Some(source) = source else {
            return 0;
        };
        if let Some(row) = self.row_of(total, source) {
            return row;
        }
        let count = self.len(total);
        if count == 0 {
            return 0;
        }
        self.pinned.range(..source).count().min(count - 1)
    }

    /// Validate a comma-separated FORMAT list before replacing the current selection.
    /// An empty string, `*` or `all` restores all fields; identifiers are case sensitive.
    pub(super) fn set_fields(&mut self, text: &str, available: &[String]) -> Result<(), String> {
        let text = text.trim();
        if text.is_empty() || text == "*" || text.eq_ignore_ascii_case("all") {
            self.fields.clear();
            return Ok(());
        }
        let mut selected = Vec::new();
        for field in text.split(',').map(str::trim) {
            if field.is_empty() {
                return Err("Enter comma-separated FORMAT fields, such as GT,DP,AD".into());
            }
            if !available.iter().any(|name| name == field) {
                return Err(format!("Unknown FORMAT field: {field}"));
            }
            if !selected.iter().any(|name| name == field) {
                selected.push(field.to_owned());
            }
        }
        self.fields = selected;
        Ok(())
    }

    /// Return the editable FORMAT expression, or empty text for the default selection.
    pub(super) fn field_expression(&self) -> String {
        self.fields.join(",")
    }

    /// Return headings in the selected order, retaining fields absent from this record.
    pub(super) fn fields<'a>(&'a self, current: &'a [String]) -> Vec<&'a str> {
        let fields = if self.fields.is_empty() {
            current
        } else {
            &self.fields
        };
        fields.iter().map(String::as_str).collect()
    }

    /// Project parsed FORMAT values into display order, padding absent fields with dots.
    pub(super) fn project_values(&self, current: &[String], values: &[String]) -> Vec<String> {
        self.fields(current)
            .into_iter()
            .map(|field| {
                current
                    .iter()
                    .position(|name| name == field)
                    .and_then(|index| values.get(index))
                    .cloned()
                    .unwrap_or_else(|| ".".into())
            })
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::SampleNavigation;

    /// Construct a compact header or FORMAT list for navigation tests.
    fn strings(names: &[&str]) -> Vec<String> {
        names.iter().map(|name| (*name).into()).collect()
    }

    /// Page source columns without materializing every sample name or value.
    #[test]
    fn sample_pages_keep_source_indices_and_respect_bounds() {
        let mut navigation = SampleNavigation::default();
        assert_eq!(
            navigation.indices(1_000_000, 999_998, 10),
            [999_998, 999_999]
        );
        assert!(navigation.indices(3, usize::MAX, 1).is_empty());
        assert_eq!(navigation.source_index(3, 3), None);
        navigation.toggle_pin(10, 2);
        navigation.toggle_pin(10, 7);
        assert_eq!(navigation.toggle_pinned_only(10, 7), 1);
        assert_eq!(navigation.indices(10, 0, 10), [2, 7]);
        assert_eq!(navigation.indices(10, 1, 1), [7]);
        assert_eq!(navigation.row_of(10, 7), Some(1));
        assert_eq!(navigation.source_index(10, 1), Some(7));
    }

    /// Search names with wraparound and preserve the visible pinned-sample order.
    #[test]
    fn name_search_wraps_and_obeys_pinned_selection() {
        let names = strings(&["Control", "Patient_A", "patient_B", "Änne"]);
        let mut navigation = SampleNavigation::default();
        assert_eq!(navigation.find(&names, "PATIENT", 0), Some(1));
        assert_eq!(navigation.find(&names, "patient", 2), Some(2));
        assert_eq!(navigation.find(&names, "patient", 3), Some(1));
        assert_eq!(navigation.find(&names, "änne", 0), Some(3));
        assert_eq!(navigation.find(&names, "", 0), None);
        assert_eq!(navigation.find(&names, "absent", 0), None);
        navigation.toggle_pin(names.len(), 2);
        navigation.toggle_pinned_only(names.len(), 0);
        assert_eq!(navigation.find(&names, "PATIENT", 0), Some(0));
        assert_eq!(navigation.find(&names, "control", 0), None);
    }

    /// Pin changes preserve the source selection and gracefully handle an empty page.
    #[test]
    fn pin_toggles_keep_selection_or_choose_an_adjacent_row() {
        let mut navigation = SampleNavigation::default();
        assert_eq!(navigation.toggle_pin(8, 2), Some(2));
        assert_eq!(navigation.toggle_pin(8, 5), Some(5));
        assert!(navigation.is_pinned(5));
        assert_eq!(navigation.pinned_count(), 2);
        assert_eq!(navigation.toggle_pinned_only(8, 5), 1);
        assert!(navigation.pinned_only());
        assert_eq!(navigation.toggle_pin(8, 1), Some(0));
        assert_eq!(navigation.source_index(8, 0), Some(2));
        assert_eq!(navigation.toggle_pin(8, 0), None);
        assert_eq!(navigation.len(8), 0);
        assert_eq!(navigation.toggle_pin(8, 0), None);
        assert_eq!(navigation.toggle_pinned_only(8, 0), 0);
        assert!(!navigation.pinned_only());
        assert_eq!(navigation.len(8), 8);
    }

    /// Deduplicate field lists and reject malformed identifiers before committing them.
    #[test]
    fn field_selection_is_atomic_and_preserves_default_padding() {
        let mut navigation = SampleNavigation::default();
        let fields = strings(&["GT", "AD", "DP"]);
        navigation.set_fields(" DP,GT,DP ", &fields).unwrap();
        assert_eq!(navigation.field_expression(), "DP,GT");
        assert!(navigation.set_fields("GT,,DP", &fields).is_err());
        assert!(navigation.set_fields("gt", &fields).is_err());
        assert_eq!(navigation.field_expression(), "DP,GT");
        navigation.set_fields("all", &fields).unwrap();
        assert_eq!(navigation.fields(&fields), ["GT", "AD", "DP"]);
        assert_eq!(
            navigation.project_values(&fields, &strings(&["./."])),
            ["./.", ".", "."]
        );
    }
}
