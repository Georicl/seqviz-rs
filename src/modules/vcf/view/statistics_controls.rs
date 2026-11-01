//! Explicit statistics requests and their cancellation lifetime on the document page.
use super::{Overlay, PendingOperation, VcfView};
use crate::modules::vcf::protocol::Request;
use std::sync::{
    Arc,
    atomic::{AtomicBool, Ordering},
};

impl VcfView {
    /// Permanently cancel the previous statistics operation and forget its identity.
    pub(super) fn cancel_statistics(&mut self) {
        if let Some(operation) = self.pending_statistics.take() {
            operation.cancel.store(true, Ordering::Relaxed);
        }
    }

    /// Request one explicit statistical snapshot of the currently indexed result set.
    pub(super) fn request_statistics(&mut self) {
        self.cancel_statistics();
        self.statistics = None;
        self.statistics_error = None;
        // Ensure the backend has opened this view before it receives the statistics request.
        self.request();
        // Viewport tokens change on resize; this explicit operation needs its own identity.
        let id = self.clock.fetch_add(1, Ordering::Relaxed) + 1;
        let cancel = Arc::new(AtomicBool::new(false));
        self.send(Request::Statistics {
            file: self.file,
            token: id,
            contig: self.contig.clone(),
            query: self.query.clone(),
            cancel: Arc::clone(&cancel),
        });
        self.pending_statistics = Some(PendingOperation { id, cancel });
    }

    /// Accept statistics only while the original panel and operation are still active.
    pub(super) fn accepts_statistics(&self, file: usize, token: u64) -> bool {
        self.enabled
            && file == self.file
            && self.overlay == Some(Overlay::Statistics)
            && self.pending_statistics.as_ref().is_some_and(|operation| {
                operation.id == token && !operation.cancel.load(Ordering::Relaxed)
            })
    }

    /// Explain the scope of an explicit statistics result, including partial indexing.
    pub(super) fn statistics_lines(&self) -> Vec<String> {
        if let Some(statistics) = &self.statistics {
            let mut lines = statistics.lines();
            lines.insert(
                1,
                "F4 refreshes statistics from the currently indexed records.".into(),
            );
            return lines;
        }
        let mut lines = vec![
            "VCF STATISTICS".into(),
            format!(
                "Contig: {}",
                self.contig.as_deref().unwrap_or("All records")
            ),
            format!("Query: {}", self.query.summary()),
            "F4 refreshes statistics from the currently indexed records.".into(),
            String::new(),
        ];
        if let Some(error) = &self.statistics_error {
            lines.push(format!("Statistics failed: {error}"));
        } else {
            lines.push("Calculating… Esc cancels".into());
        }
        lines
    }
}
