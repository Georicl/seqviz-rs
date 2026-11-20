//! Messages distinguish a viewport refresh from a user operation.
use super::data::{Format, Progress, RecordMeta, RecordSummary, SequenceWindow, Stats};
use super::query::RecordQuery;
use std::sync::{Arc, atomic::AtomicBool};

/// Requested sequence and record-list windows, tagged with the current viewport generation.
///
/// # Fields
///
/// - `token`: Only viewport snapshots use this generation. Resizing does not cancel operations.
/// - `file`: Stable tab index whose dataset and view state the request addresses.
/// - `record`: Zero-based selected record ID.
/// - `position`: Selected position within the filtered, sorted result.
/// - `anchor`: Whether to preserve the source record as indexing reorders results.
/// - `query`: Record predicates and ordering applied by the data layer.
/// - `start`: Zero-based sequence offset at which the requested window begins.
/// - `count`: Requested number of sequence characters, capped by the backend.
/// - `list_start`: Zero-based first result position requested for the record list.
/// - `list_count`: Requested number of visible record summaries, capped by the backend.
#[derive(Clone, Debug)]
pub struct ViewRequest {
    /// Only viewport snapshots use this generation. Resizing does not cancel operations.
    pub token: u64,
    /// Stable tab index whose dataset and view state the request addresses.
    pub file: usize,
    /// Zero-based selected record ID.
    pub record: u64,
    /// Position of the selected row in the filtered/sorted result, independent of the source ID.
    pub position: u64,
    /// Preserve `record` across incremental reordering; false selects `position` instead.
    pub anchor: bool,
    /// Record predicates and ordering applied by the data layer.
    pub query: Box<RecordQuery>,
    /// Zero-based sequence offset at which the requested window begins.
    pub start: u64,
    /// Requested number of sequence characters, capped by the backend.
    pub count: usize,
    /// Zero-based first result position requested for the record list.
    pub list_start: u64,
    /// Requested number of visible record summaries, capped by the backend.
    pub list_count: usize,
}

/// Immutable viewport data assembled on the worker for one view request.
///
/// # Fields
///
/// - `request`: Original request used to match the snapshot to the active viewport.
/// - `format`: Detected source sequence/alignment format.
/// - `compression`: Source compression label shown in file details and status.
/// - `file_size`: Read on the worker when the source is opened, never during rendering.
/// - `records`: Indexed summaries for the requested record-list window.
/// - `matching_records`: Number of currently indexed records matching the query.
/// - `record`: Selected record metadata, absent if the scanner has not reached that record.
/// - `window`: Available sequence and quality bytes beginning at the requested offset.
/// - `progress`: Current scan progress, including recoverable and fatal error information.
/// - `stats`: Most recently requested statistics, possibly computed from a partial index.
/// - `paused`: Whether this tab has paused background indexing.
#[derive(Clone, Debug)]
pub struct Snapshot {
    /// Identity of the indexed source from which the visible record was read.
    pub(crate) source_identity: [u64; 7],
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
    /// Number of matching indexed records; this is provisional while scanning continues.
    pub matching_records: u64,
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
///
/// # Variants
///
/// - `View`: Replace the target viewport without cancelling operations in the same file and record.
/// - `Search`: Search filtered/sorted names from an inclusive result position, wrapping after scanning stops.
///   - `operation_id`: Monotonic operation identity used to reject stale requests and replies.
///   - `file`: Stable tab index associated with this command or result.
///   - `query`: Literal substring to find in indexed record names.
///   - `after`: Inclusive zero-based result position in the filtered, sorted list.
///   - `cancel`: Operation-owned flag shared with the foreground view; cancellation is permanent.
/// - `Statistics`: Request cached/refreshed statistics and resume this tab's background scan.
///   - `file`: Stable tab index associated with this command or result.
/// - `Copy`: Copy a complete formatted record or a selected sequence interval once it is indexed.
///   - `operation_id`: Monotonic operation identity used to reject stale requests and replies.
///   - `file`: Stable tab index associated with this command or result.
///   - `record`: Zero-based record ID selected or copied by the operation.
///   - `range`: Zero-based half-open sequence interval, or None to copy the complete record.
///   - `cancel`: Operation-owned flag shared with the foreground view; cancellation is permanent.
/// - `Cancel`: Pause scanning and cancel pending operations for the given tab.
/// - `Close`: Close a tab and release its dataset when no alias remains open.
/// - `Quit`: Cancel sequence operations and stop the owning worker loop.
pub enum Request {
    /// Load a complete record for explicit editing.
    Edit {
        /// Prevent an old tab viewport from editing a newer revision.
        source_identity: [u64; 7],
        file: usize,
        operation_id: u64,
        record: u64,
    },
    /// Atomically commit the validated draft to its original source.
    Save {
        file: usize,
        operation_id: u64,
        draft: Arc<super::data::edit::Draft>,
    },
    /// Replace the target viewport without cancelling operations in the same file and record.
    View(ViewRequest),
    /// Search filtered/sorted record names from an inclusive result position, wrapping after scanning stops.
    ///
    /// # Fields
    ///
    /// - `operation_id`: Monotonic operation identity used to reject stale requests and replies.
    /// - `file`: Stable tab index associated with this command or result.
    /// - `query`: Literal substring to find in indexed record names.
    /// - `after`: Inclusive zero-based result position in the filtered, sorted list.
    /// - `cancel`: Operation-owned flag shared with the foreground view; cancellation is permanent.
    Search {
        /// Monotonic operation identity used to reject stale requests and replies.
        operation_id: u64,
        /// Stable tab index associated with this command or result.
        file: usize,
        /// Literal substring to find in indexed record names.
        query: String,
        /// Inclusive zero-based starting position in the current query result.
        after: u64,
        /// Operation-owned flag shared with the foreground view; cancellation is permanent.
        cancel: Arc<AtomicBool>,
    },
    /// Request cached/refreshed statistics and resume this tab's background scan.
    ///
    /// # Fields
    ///
    /// - `file`: Stable tab index associated with this command or result.
    Statistics {
        /// Stable tab index associated with this command or result.
        file: usize,
    },
    /// Copy a complete formatted record or a selected sequence interval once it is indexed.
    ///
    /// # Fields
    ///
    /// - `operation_id`: Monotonic operation identity used to reject stale requests and replies.
    /// - `file`: Stable tab index associated with this command or result.
    /// - `record`: Zero-based record ID selected or copied by the operation.
    /// - `range`: Zero-based half-open sequence interval, or None to copy the complete record.
    /// - `cancel`: Operation-owned flag shared with the foreground view; cancellation is permanent.
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
///
/// # Variants
///
/// - `View`: Publish loaded viewport data and the latest scan/statistics state.
/// - `Selected`: Report the record found by a still-current name-search operation.
///   - `operation_id`: Monotonic operation identity used to reject stale requests and replies.
///   - `file`: Stable tab index associated with this command or result.
///   - `record`: Zero-based record ID selected or copied by the operation.
/// - `Copied`: Return copied text for delivery through the application's clipboard service.
///   - `operation_id`: Monotonic operation identity used to reject stale requests and replies.
///   - `file`: Stable tab index associated with this command or result.
///   - `text`: Formatted record or selected sequence characters to send to the clipboard.
/// - `OperationFailed`: Operation errors have the same identity checks as successful results.
///   - `operation_id`: Monotonic operation identity used to reject stale requests and replies.
///   - `file`: Stable tab index associated with this command or result.
///   - `text`: Failure details to show for the matching operation.
/// - `Message`: Publish a file-scoped status or backend error message.
///   - `file`: Stable tab index associated with this command or result.
///   - `text`: Status or backend error text to show for the associated file.
pub enum Update {
    /// The requested edit buffer or a recoverable load error.
    EditReady {
        file: usize,
        operation_id: u64,
        result: Result<super::data::edit::Draft, String>,
    },
    /// Result of an explicit source save.
    Saved {
        file: usize,
        operation_id: u64,
        result: Result<(), String>,
    },
    /// Publish loaded viewport data and the latest scan/statistics state.
    View(Box<Snapshot>),
    /// Report the record found by a still-current name-search operation.
    ///
    /// # Fields
    ///
    /// - `operation_id`: Monotonic operation identity used to reject stale requests and replies.
    /// - `file`: Stable tab index associated with this command or result.
    /// - `record`: Zero-based record ID selected or copied by the operation.
    Selected {
        /// Monotonic operation identity used to reject stale requests and replies.
        operation_id: u64,
        /// Stable tab index associated with this command or result.
        file: usize,
        /// Zero-based record ID selected or copied by the operation.
        record: u64,
    },
    /// Return copied text for delivery through the application's clipboard service.
    ///
    /// # Fields
    ///
    /// - `operation_id`: Monotonic operation identity used to reject stale requests and replies.
    /// - `file`: Stable tab index associated with this command or result.
    /// - `text`: Formatted record or selected sequence characters to send to the clipboard.
    Copied {
        /// Monotonic operation identity used to reject stale requests and replies.
        operation_id: u64,
        /// Stable tab index associated with this command or result.
        file: usize,
        /// Formatted record or selected sequence characters to send to the clipboard.
        text: String,
    },
    /// Operation errors have the same identity checks as successful results.
    ///
    /// # Fields
    ///
    /// - `operation_id`: Monotonic operation identity used to reject stale requests and replies.
    /// - `file`: Stable tab index associated with this command or result.
    /// - `text`: Failure details to show for the matching operation.
    OperationFailed {
        /// Monotonic operation identity used to reject stale requests and replies.
        operation_id: u64,
        /// Stable tab index associated with this command or result.
        file: usize,
        /// Failure details to show for the matching operation.
        text: String,
    },
    /// Publish a file-scoped status or backend error message.
    ///
    /// # Fields
    ///
    /// - `file`: Stable tab index associated with this command or result.
    /// - `text`: Status or backend error text to show for the associated file.
    Message {
        /// Stable tab index associated with this command or result.
        file: usize,
        /// Status or backend error text to show for the associated file.
        text: String,
    },
}
