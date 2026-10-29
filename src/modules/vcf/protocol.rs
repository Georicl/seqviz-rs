//! VCF viewport requests and snapshots retain only one selected raw record.
use super::model::{ContigSummary, Progress, RecordSummary, VcfEntry, VcfHeader};
use std::sync::Arc;

/// Identify a tab, request generation and bounded region of the variant table.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ViewRequest {
    /// Tab index used to route the response.
    pub file: usize,
    /// View generation used to reject stale responses.
    pub token: u64,
    /// Contig filter, or None to show source order across all contigs.
    pub contig: Option<String>,
    /// Zero-based ordinal within the current filter; equals the source ID without a filter.
    pub selected: u64,
    /// Zero-based first table ordinal within the current filter.
    pub start: u64,
    /// Requested number of table rows, capped by the data layer.
    pub count: usize,
}

/// One viewport response with scan progress and at most one complete selected record.
#[derive(Clone, Debug)]
pub struct Snapshot {
    /// Original viewport identity and filter for this response.
    pub request: ViewRequest,
    /// Shared original header and parsed metadata.
    pub header: Arc<VcfHeader>,
    /// Bounded table summaries beginning at the requested ordinal.
    pub rows: Vec<RecordSummary>,
    /// Complete selected record, if it has already been indexed.
    pub selected: Option<Arc<VcfEntry>>,
    /// Currently indexed record count within the requested filter.
    pub total: u64,
    /// Contig choices and their current indexed counts.
    pub contigs: Vec<ContigSummary>,
    /// Source-wide indexing progress and input errors.
    pub progress: Progress,
    /// Compression label reported by the source reader.
    pub compression: String,
    /// Whether automatic background indexing is paused for this tab.
    pub paused: bool,
}

/// Commands sent from the document view to the VCF backend.
#[derive(Clone, Debug)]
pub enum Request {
    /// Replace the active viewport and request its current indexed content.
    View(ViewRequest),
    /// Find a genomic coordinate, scanning further when needed.
    Locate {
        /// Tab that initiated the lookup.
        file: usize,
        /// Lookup generation echoed in the response.
        token: u64,
        /// Exact contig identifier to search.
        contig: String,
        /// Requested genomic coordinate in VCF POS units.
        position: u64,
    },
    /// Pause indexing and cancel a pending lookup for this tab index.
    Cancel(usize),
    /// Close this tab index, releasing the dataset after its last alias closes.
    Close(usize),
    /// Stop processing requests in the backend.
    Quit,
}

/// Backend responses carrying the originating tab or viewport request identity.
#[derive(Clone, Debug)]
pub enum Update {
    /// Publish rows, selected-record details and progress for a viewport request.
    View(Box<Snapshot>),
    /// Report the filtered ordinal found for a position lookup.
    Located {
        /// Tab that initiated the lookup.
        file: usize,
        /// Lookup generation used to reject stale results.
        token: u64,
        /// Zero-based record ordinal within the lookup's contig.
        selected: u64,
    },
    /// Report a lookup error or a contig with no indexed records.
    LocateFailed {
        /// Tab that initiated the lookup.
        file: usize,
        /// Lookup generation used to reject stale failures.
        token: u64,
        /// Underlying read/cache error or missing-contig explanation.
        text: String,
    },
    /// Report a general backend status or error for one tab.
    Message {
        /// Tab whose status should change.
        file: usize,
        /// Human-readable status or error message.
        text: String,
    },
}
