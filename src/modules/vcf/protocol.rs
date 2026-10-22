//! VCF viewport requests and snapshots retain only one selected raw record.
use super::model::{ContigSummary, Progress, RecordSummary, VcfEntry, VcfHeader};
use std::sync::Arc;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ViewRequest {
    pub file: usize,
    pub token: u64,
    pub contig: Option<String>,
    /// Ordinal within the current contig filter, rather than the source record ID.
    pub selected: u64,
    pub start: u64,
    pub count: usize,
}

#[derive(Clone, Debug)]
pub struct Snapshot {
    pub request: ViewRequest,
    pub header: Arc<VcfHeader>,
    pub rows: Vec<RecordSummary>,
    pub selected: Option<Arc<VcfEntry>>,
    pub total: u64,
    pub contigs: Vec<ContigSummary>,
    pub progress: Progress,
    pub compression: String,
    pub paused: bool,
}

#[derive(Clone, Debug)]
pub enum Request {
    View(ViewRequest),
    Locate {
        file: usize,
        token: u64,
        contig: String,
        position: u64,
    },
    Cancel(usize),
    Close(usize),
    Quit,
}

#[derive(Clone, Debug)]
pub enum Update {
    View(Box<Snapshot>),
    Located {
        file: usize,
        token: u64,
        selected: u64,
    },
    LocateFailed {
        file: usize,
        token: u64,
        text: String,
    },
    Message {
        file: usize,
        text: String,
    },
}
