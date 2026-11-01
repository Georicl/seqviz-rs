//! Connect sample preferences to document navigation and lossless displayed text.
use super::{Detail, Focus, VcfView};

impl VcfView {
    /// Locate a sample in the active list and focus its row without reading other samples.
    pub(super) fn find_sample(&mut self, query: &str, next: bool) {
        let Some(snapshot) = &self.snapshot else {
            self.message = "Reading sample names…".into();
            return;
        };
        let after = if next {
            self.detail_row.saturating_add(1)
        } else {
            0
        };
        let found = self.samples.find(&snapshot.header.samples, query, after);
        self.sample_query = query.to_owned();
        let Some(row) = found else {
            self.message = format!("No sample matches {query:?}");
            return;
        };
        self.change_detail(Detail::Samples);
        self.focus = Focus::Detail;
        self.select_detail_row(row);
        self.message.clear();
    }

    /// Pin or unpin the selected source sample and keep the remaining row visible.
    pub(super) fn pin_sample(&mut self) {
        if self.detail != Detail::Samples {
            self.message = "Open Samples to pin a sample".into();
            return;
        }
        let total = self.snapshot.as_ref().map_or(0, |s| s.header.samples.len());
        let row = self.samples.toggle_pin(total, self.detail_row);
        self.cancel_copy();
        self.reset_detail_position();
        self.select_detail_row(row.unwrap_or(0));
        self.message = format!("{} pinned samples", self.samples.pinned_count());
    }

    /// Switch between the complete sample list and the pinned subset.
    pub(super) fn toggle_pinned_samples(&mut self) {
        let total = self.snapshot.as_ref().map_or(0, |s| s.header.samples.len());
        let row = self.samples.toggle_pinned_only(total, self.detail_row);
        self.change_detail(Detail::Samples);
        self.focus = Focus::Detail;
        self.select_detail_row(row);
        self.message = if self.samples.pinned_only() {
            "Showing pinned samples; P shows all".into()
        } else {
            "Showing all samples".into()
        };
    }

    /// Validate a FORMAT selection before changing the displayed and copied columns.
    pub(super) fn choose_fields(&mut self, text: &str) -> Result<(), String> {
        let mut available = self
            .entry()
            .ok_or("Select a variant first")?
            .record
            .format
            .clone();
        if let Some(snapshot) = &self.snapshot {
            // Declared columns remain selectable when the current variant omits them.
            for field in &snapshot.header.definitions {
                if field.kind == "FORMAT" && !available.contains(&field.id) {
                    available.push(field.id.clone());
                }
            }
        }
        self.samples.set_fields(text, &available)?;
        let row = if self.detail == Detail::Samples {
            self.detail_row
        } else {
            0
        };
        self.change_detail(Detail::Samples);
        self.focus = Focus::Detail;
        self.select_detail_row(row);
        self.message.clear();
        Ok(())
    }

    /// Return the exact FORMAT headings used for sample rendering and clipboard output.
    pub(super) fn sample_fields(&self) -> Vec<&str> {
        self.entry()
            .map_or_else(Vec::new, |entry| self.samples.fields(&entry.record.format))
    }

    /// Build the logical sample-table heading in the selected field order.
    pub(super) fn sample_heading(&self) -> String {
        let fields = self.sample_fields();
        if fields.is_empty() {
            "SAMPLE".into()
        } else {
            format!("SAMPLE\t{}", fields.join("\t"))
        }
    }

    /// Decode only selected sample columns, traversing the raw row once per visible window.
    pub(super) fn sample_rows(&self, start: usize, count: usize) -> Vec<String> {
        let Some(entry) = self.entry() else {
            return Vec::new();
        };
        let names = &self.snapshot.as_ref().unwrap().header.samples;
        let indices = self.samples.indices(names.len(), start, count);
        let mut wanted = indices.into_iter().peekable();
        let mut lines = Vec::new();
        // Sample rows can be sparse after pinning; decode only those source columns.
        for (index, text) in entry.record.raw.split('\t').skip(9).enumerate() {
            if wanted.peek().is_none() {
                break;
            }
            if wanted.peek() != Some(&index) {
                continue;
            }
            wanted.next();
            let values = text.split(':').map(str::to_owned).collect::<Vec<_>>();
            let values = self.samples.project_values(&entry.record.format, &values);
            let name = names.get(index).map(String::as_str).unwrap_or(".");
            let line = if values.is_empty() {
                name.to_owned()
            } else {
                format!("{name}\t{}", values.join("\t"))
            };
            lines.push(line);
        }
        lines
    }
}
