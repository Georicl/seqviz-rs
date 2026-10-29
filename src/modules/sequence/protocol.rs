//! Messages distinguish a viewport refresh from a user operation.
use super::data::{Format, Progress, RecordMeta, RecordSummary, SequenceWindow, Stats};
use std::sync::{Arc, atomic::AtomicBool};

/// Requested sequence and record-list windows, tagged with the current viewport generation.
#[derive(Clone, Debug)]
pub struct ViewRequest {
    /// Only viewport snapshots use this generation. Resizing does not cancel operations.
    pub token: u64,
    /// Stable tab index whose dataset and view state the request addresses.
    pub file: usize,
    /// Zero-based selected record ID.
    pub record: u64,
    /// Zero-based sequence offset at which the requested window begins.
    pub start: u64,
    /// Requested number of sequence characters, capped by the backend.
    pub count: usize,
    /// Zero-based first record ID requested for the record list.
    pub list_start: u64,
    /// Requested number of visible record summaries, capped by the backend.
    pub list_count: usize,
}

/// Immutable viewport data assembled on the worker for one view request.
#[derive(Clone, Debug)]
pub struct Snapshot {
    /// Original request used to match the snapshot to the active viewport.
    pub request: ViewRequest,
    /// Detected source sequence/alignment format.
    pub format: Format,
    /// Source compression label shown in file details and status.
    pub compression: String,
    /// Read on the worker when the source is opened, never during rendering.
    pub file_size: Option<u64>,
    /// Indexed summaries for the requested record-list window.
    pub records: Vec<RecordSummary>,
    /// Selected record metadata, absent if the scanner has not reached that record.
    pub record: Option<RecordMeta>,
    /// Available sequence and quality bytes beginning at the requested offset.
    pub window: SequenceWindow,
    /// Current scan progress, including recoverable and fatal error information.
    pub progress: Progress,
    /// Most recently requested statistics, possibly computed from a partial index.
    pub stats: Option<Stats>,
    /// Whether this tab has paused background indexing.
    pub paused: bool,
}

/// Commands accepted by the sequence backend; user operations have independent cancellation.
pub enum Request {
    /// Replace the target viewport without cancelling operations in the same file and record.
    View(ViewRequest),
    /// Search record names from an inclusive record ID, wrapping after scanning stops.
    Search {
        /// Monotonic operation identity used to reject stale requests and replies.
        operation_id: u64,
        /// Stable tab index associated with this command or result.
        file: usize,
        /// Literal substring to find in indexed record names.
        query: String,
        /// Inclusive zero-based starting record ID for the search.
        after: u64,
        /// Operation-owned flag shared with the foreground view; cancellation is permanent.
        cancel: Arc<AtomicBool>,
    },
    /// Request cached/refreshed statistics and resume this tab's background scan.
    Statistics {
        /// Stable tab index associated with this command or result.
        file: usize,
    },
    /// Copy a complete formatted record or a selected sequence interval once it is indexed.
    Copy {
        /// Monotonic operation identity used to reject stale requests and replies.
        operation_id: u64,
        /// Stable tab index associated with this command or result.
        file: usize,
        /// Zero-based record ID selected or copied by the operation.
        record: u64,
        /// Zero-based half-open sequence interval, or None to copy the complete record.
        range: Option<(u64, u64)>,
        /// Operation-owned flag shared with the foreground view; cancellation is permanent.
        cancel: Arc<AtomicBool>,
    },
    /// Pause scanning and cancel pending operations for the given tab.
    Cancel(usize),
    /// Close a tab and release its dataset when no alias remains open.
    Close(usize),
    /// Cancel sequence operations and stop the owning worker loop.
    Quit,
}

/// Worker results tagged by viewport generation or by user-operation identity.
pub enum Update {
    /// Publish loaded viewport data and the latest scan/statistics state.
    View(Box<Snapshot>),
    /// Report the record found by a still-current name-search operation.
    Selected {
        /// Monotonic operation identity used to reject stale requests and replies.
        operation_id: u64,
        /// Stable tab index associated with this command or result.
        file: usize,
        /// Zero-based record ID selected or copied by the operation.
        record: u64,
    },
    /// Return copied text for delivery through the application's clipboard service.
    Copied {
        /// Monotonic operation identity used to reject stale requests and replies.
        operation_id: u64,
        /// Stable tab index associated with this command or result.
        file: usize,
        /// Formatted record or selected sequence characters to send to the clipboard.
        text: String,
    },
    /// Operation errors have the same identity checks as successful results.
    OperationFailed {
        /// Monotonic operation identity used to reject stale requests and replies.
        operation_id: u64,
        /// Stable tab index associated with this command or result.
        file: usize,
        /// Failure details to show for the matching operation.
        text: String,
    },
    /// Publish a file-scoped status or backend error message.
    Message {
        /// Stable tab index associated with this command or result.
        file: usize,
        /// Status or backend error text to show for the associated file.
        text: String,
    },
}
