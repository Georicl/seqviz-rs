//! Messages distinguish a viewport refresh from a user operation.
use super::data::{Format, Progress, RecordMeta, RecordSummary, SequenceWindow, Stats};
use std::sync::{Arc, atomic::AtomicBool};

#[derive(Clone, Debug)]
pub struct ViewRequest {
    /// Only viewport snapshots use this generation. Resizing does not cancel operations.
    pub token: u64,
    pub file: usize,
    pub record: u64,
    pub start: u64,
    pub count: usize,
    pub list_start: u64,
    pub list_count: usize,
}

#[derive(Clone, Debug)]
pub struct Snapshot {
    pub request: ViewRequest,
    pub format: Format,
    pub compression: String,
    /// Read on the worker when the source is opened, never during rendering.
    pub file_size: Option<u64>,
    pub records: Vec<RecordSummary>,
    pub record: Option<RecordMeta>,
    pub window: SequenceWindow,
    pub progress: Progress,
    pub stats: Option<Stats>,
    pub paused: bool,
}

pub enum Request {
    View(ViewRequest),
    Search {
        operation_id: u64,
        file: usize,
        query: String,
        after: u64,
        cancel: Arc<AtomicBool>,
    },
    Statistics {
        file: usize,
    },
    Copy {
        operation_id: u64,
        file: usize,
        record: u64,
        range: Option<(u64, u64)>,
        cancel: Arc<AtomicBool>,
    },
    Cancel(usize),
    Close(usize),
    Quit,
}

pub enum Update {
    View(Box<Snapshot>),
    Selected {
        operation_id: u64,
        file: usize,
        record: u64,
    },
    Copied {
        operation_id: u64,
        file: usize,
        text: String,
    },
    /// Operation errors have the same identity checks as successful results.
    OperationFailed {
        operation_id: u64,
        file: usize,
        text: String,
    },
    Message {
        file: usize,
        text: String,
    },
}
