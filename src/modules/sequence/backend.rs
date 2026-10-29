//! Sequence scanning, view snapshots and cancellable domain operations.
use super::data::{Dataset, Progress, SequenceWindow, Stats};
use super::operations::{CopyJob, SearchJob};
use super::protocol::{Request, Snapshot, Update, ViewRequest};
use crate::core::tasks::Driver;
use crate::modules::session::SessionFiles;
use std::io;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, mpsc};
use std::time::{Duration, Instant};

/// Worker scheduling, cancellation and shared-dataset regression tests.
#[cfg(test)]
#[path = "backend_tests.rs"]
mod tests;

/// Throttle scan-progress snapshots; explicit view or cancel requests can publish immediately.
const PUBLISH_INTERVAL: Duration = Duration::from_millis(100);
/// Minimum interval between partial statistics refreshes.
const STATS_INTERVAL: Duration = Duration::from_secs(1);

/// A dataset shared by alias tabs, with cached metadata, statistics and fatal I/O state.
pub(super) struct Slot {
    /// Indexed source shared by every tab with the same canonical owner.
    pub(super) dataset: Dataset,
    /// Physical source size captured once when the shared dataset opens.
    file_size: Option<u64>,
    /// Fatal I/O failure remembered to stop retries and include in snapshots.
    failure: Option<String>,
    /// Last statistics result, reused until indexed progress changes.
    stats: Option<Stats>,
    /// Scan state used to produce the cached statistics.
    stats_progress: Option<ProgressKey>,
    /// Time of the last statistics attempt, used to throttle partial refreshes.
    last_stats: Instant,
}

/// Pause and statistics preferences that remain independent for each alias tab.
#[derive(Default)]
struct TabState {
    /// Whether this tab has explicitly paused background indexing.
    paused: bool,
    /// Whether this tab has requested statistics updates.
    statistics: bool,
}

/// Comparable scan progress used to invalidate snapshots and cached statistics.
#[derive(Clone, PartialEq, Eq)]
pub(super) struct ProgressKey {
    /// Number of records discovered by the scanner.
    pub(super) records: u64,
    /// Number of decoded source bytes scanned.
    pub(super) bytes: u64,
    /// Whether scanning reached the end of the source.
    pub(super) complete: bool,
    /// Number of parser errors recorded in the index.
    pub(super) errors: u64,
    /// Fatal scan error that prevents further progress.
    pub(super) error: Option<String>,
}
impl From<&Progress> for ProgressKey {
    /// Extract the progress fields that determine whether cached results have changed.
    fn from(progress: &Progress) -> Self {
        Self {
            records: progress.records,
            bytes: progress.scanned_bytes,
            complete: progress.complete,
            errors: progress.errors,
            error: progress.error.clone(),
        }
    }
}

/// Owns sequence datasets and advances work for the active viewport on the worker thread.
pub(crate) struct Backend {
    /// Open-tab state and canonical dataset ownership mapping.
    files: SessionFiles,
    /// Directory for sequence indexes and source seek checkpoints.
    cache: PathBuf,
    /// Lazily opened datasets stored only at canonical owner indices.
    slots: Vec<Option<Slot>>,
    /// Independent pause/statistics preferences indexed by tab.
    tabs: Vec<TabState>,
    /// Active viewport controlling which dataset advances.
    view: Option<ViewRequest>,
    /// Pending cancellable name search for the active context.
    search: Option<SearchJob>,
    /// Pending cancellable copy waiting for its record or range.
    copy: Option<CopyJob>,
    /// Highest accepted operation ID, preventing stale requests from replacing current work.
    last_operation_id: u64,
    /// Whether navigation still needs an indexed prefix even while background work is paused.
    foreground_read: bool,
    /// Whether changes require a fresh viewport snapshot.
    dirty: bool,
    /// Timestamp used to throttle viewport publication.
    last_publish: Instant,
    /// Whether the next worker tick should run without waiting for a request.
    working: bool,
    /// Channel carrying sequence snapshots and operation results to the session.
    updates: mpsc::Sender<Update>,
    /// Shared cancellation flag checked by expensive statistics queries.
    cancel: Arc<AtomicBool>,
}

impl Backend {
    /// Create unopened dataset slots and independent pause/statistics state for each tab.
    pub(crate) fn new(
        files: SessionFiles,
        cache: PathBuf,
        updates: mpsc::Sender<Update>,
        cancel: Arc<AtomicBool>,
    ) -> Self {
        let count = files.len();
        Self {
            files,
            cache,
            slots: (0..count).map(|_| None).collect(),
            tabs: (0..count).map(|_| TabState::default()).collect(),
            view: None,
            search: None,
            copy: None,
            last_operation_id: 0,
            foreground_read: false,
            dirty: false,
            last_publish: Instant::now() - PUBLISH_INTERVAL,
            working: false,
            updates,
            cancel,
        }
    }

    /// Run immediately while scanning, otherwise wait until a pending snapshot is due.
    fn timeout(&self) -> Duration {
        if self.working {
            Duration::ZERO
        } else if self.dirty {
            PUBLISH_INTERVAL.saturating_sub(self.last_publish.elapsed())
        } else {
            Duration::from_secs(60)
        }
    }

    /// Cancel and remove pending search and copy jobs without resetting their flags.
    fn cancel_operations(&mut self) {
        if let Some(job) = self.search.take() {
            job.cancel.store(true, Ordering::Relaxed);
        }
        if let Some(job) = self.copy.take() {
            job.cancel.store(true, Ordering::Relaxed);
        }
    }

    /// Accept a newer, uncancelled operation for the current file and resume its scan.
    fn accept_operation(&mut self, file: usize, operation_id: u64, cancel: &AtomicBool) -> bool {
        if self.view.as_ref().is_none_or(|v| v.file != file)
            || operation_id <= self.last_operation_id
            || cancel.load(Ordering::Relaxed)
        {
            return false;
        }
        self.cancel_operations();
        self.last_operation_id = operation_id;
        self.tabs[file].paused = false;
        self.dirty = true;
        true
    }

    /// Open the shared dataset on demand, replace the viewport and cancel jobs on context changes.
    fn set_view(&mut self, next: ViewRequest) {
        let file = next.file;
        if !self.files.is_open(file) {
            return;
        }
        let owner = self.files.owner(file);
        if self.slots[owner].is_none() {
            match Dataset::open(self.files.path(file), &self.cache) {
                Ok(dataset) => {
                    self.slots[owner] = Some(Slot {
                        dataset,
                        file_size: std::fs::metadata(self.files.path(file))
                            .ok()
                            .map(|m| m.len()),
                        failure: None,
                        stats: None,
                        stats_progress: None,
                        last_stats: Instant::now() - STATS_INTERVAL,
                    })
                }
                Err(error) => {
                    self.cancel_operations();
                    self.view = None;
                    self.foreground_read = false;
                    self.dirty = false;
                    message(&self.updates, file, error.to_string());
                    return;
                }
            }
        }
        let context_changed = self
            .view
            .as_ref()
            .is_some_and(|old| old.file != file || old.record != next.record);
        if context_changed {
            self.cancel_operations();
        }
        let moved = self.view.as_ref().is_none_or(|old| {
            old.file != file || old.record != next.record || old.start != next.start
        });
        // Refresh/resize after Esc cannot restart scanning; a new position may read its prefix.
        self.foreground_read |= moved;
        self.view = Some(next);
        self.dirty = true;
        self.last_publish = Instant::now() - PUBLISH_INTERVAL;
    }

    /// Apply a request, opening a dataset for a new view when needed; return false on Quit.
    fn request(&mut self, request: Request) -> bool {
        match request {
            Request::Quit => {
                self.cancel_operations();
                return false;
            }
            Request::View(next) => self.set_view(next),
            Request::Search {
                operation_id,
                file,
                query,
                after,
                cancel,
            } => {
                if !query.is_empty() && self.accept_operation(file, operation_id, &cancel) {
                    self.search = Some(SearchJob {
                        operation_id,
                        file,
                        query,
                        after,
                        cancel,
                        tried: None,
                    });
                }
            }
            Request::Copy {
                operation_id,
                file,
                record,
                range,
                cancel,
            } => {
                if self
                    .view
                    .as_ref()
                    .is_some_and(|v| v.file == file && v.record == record)
                    && self.accept_operation(file, operation_id, &cancel)
                {
                    self.copy = Some(CopyJob {
                        operation_id,
                        file,
                        record,
                        range,
                        cancel,
                    });
                }
            }
            Request::Statistics { file } => {
                if self.view.as_ref().is_some_and(|v| v.file == file) {
                    self.tabs[file].statistics = true;
                    self.tabs[file].paused = false;
                    self.dirty = true;
                }
            }
            Request::Cancel(file) => {
                if let Some(tab) = self.tabs.get_mut(file) {
                    tab.paused = true;
                    tab.statistics = false;
                }
                if self.view.as_ref().is_some_and(|v| v.file == file) {
                    self.cancel_operations();
                    self.foreground_read = false;
                    self.dirty = true;
                    self.last_publish = Instant::now() - PUBLISH_INTERVAL;
                }
                message(&self.updates, file, "Background work paused".into());
            }
            Request::Close(file) => self.close(file),
        }
        true
    }

    /// Close one tab and release its shared dataset only when its final alias closes.
    fn close(&mut self, file: usize) {
        let Some((owner, last_alias)) = self.files.close(file) else {
            return;
        };
        if last_alias {
            self.slots[owner] = None;
        }
        if self.view.as_ref().is_some_and(|v| v.file == file) {
            self.cancel_operations();
            self.view = None;
            self.foreground_read = false;
            self.dirty = false;
        }
    }

    /// Advance one scan chunk, service pending jobs and publish a throttled snapshot.
    /// Return false when the snapshot receiver has disconnected.
    fn tick(&mut self) -> bool {
        self.working = false;
        let Some(current) = &self.view else {
            return true;
        };
        let file = current.file;
        let Some(slot) = &mut self.slots[self.files.owner(file)] else {
            return true;
        };
        let tab = &mut self.tabs[file];
        let before = slot.dataset.progress();
        let stopped = before.complete || before.error.is_some() || slot.failure.is_some();
        let needs_view = self.foreground_read
            && slot
                .dataset
                .record(current.record)
                .ok()
                .flatten()
                .is_none_or(|r| {
                    !r.complete && r.length < current.start.saturating_add(current.count as u64)
                });
        if !needs_view {
            self.foreground_read = false;
        }
        if !stopped && (!tab.paused || needs_view) {
            match slot.dataset.step(64 * 1024) {
                Ok(after) => self.dirty |= ProgressKey::from(&before) != ProgressKey::from(&after),
                Err(error) => {
                    slot.failure = Some(error.to_string());
                    message(&self.updates, file, error.to_string());
                    self.dirty = true;
                }
            }
        }
        let progress = slot.dataset.progress();
        let key = ProgressKey::from(&progress);
        let stopped = progress.complete || progress.error.is_some() || slot.failure.is_some();
        self.working = !stopped && (!tab.paused || needs_view);
        if self
            .search
            .as_mut()
            .is_some_and(|job| job.step(slot, stopped, &self.updates))
        {
            self.search = None;
        }
        if self
            .copy
            .as_ref()
            .is_some_and(|job| job.step(slot, stopped, &self.updates))
        {
            self.copy = None;
        }
        self.dirty |=
            update_statistics(slot, tab, &key, stopped, &self.cancel, &self.updates, file);
        if self.dirty && self.last_publish.elapsed() >= PUBLISH_INTERVAL {
            match snapshot(slot, current.clone(), tab.paused) {
                Ok(snapshot) => {
                    if self.updates.send(Update::View(Box::new(snapshot))).is_err() {
                        return false;
                    }
                }
                Err(error) => message(&self.updates, file, error.to_string()),
            }
            self.dirty = false;
            self.last_publish = Instant::now();
        }
        true
    }
}

/// Recompute requested statistics when progress changes and the throttle allows it.
/// Return whether the cached statistics were replaced.
fn update_statistics(
    slot: &mut Slot,
    tab: &mut TabState,
    key: &ProgressKey,
    stopped: bool,
    cancel: &Arc<AtomicBool>,
    updates: &mpsc::Sender<Update>,
    file: usize,
) -> bool {
    if !tab.statistics
        || cancel.load(Ordering::Relaxed)
        || slot.stats_progress.as_ref() == Some(key)
        || (slot.stats.is_some() && !stopped && slot.last_stats.elapsed() < STATS_INTERVAL)
    {
        return false;
    }
    let changed = match slot.dataset.stats_cancellable(Arc::clone(cancel)) {
        Ok(stats) => {
            slot.stats = Some(stats);
            slot.stats_progress = Some(key.clone());
            true
        }
        Err(error) => {
            if error.kind() != io::ErrorKind::Interrupted {
                message(updates, file, error.to_string());
            }
            tab.statistics = false;
            false
        }
    };
    slot.last_stats = Instant::now();
    changed
}

/// Publish a file-scoped status message, ignoring a disconnected receiver.
fn message(updates: &mpsc::Sender<Update>, file: usize, text: String) {
    let _ = updates.send(Update::Message { file, text });
}

/// Read the bounded record list and sequence window, attaching cached statistics and failures.
fn snapshot(slot: &mut Slot, request: ViewRequest, paused: bool) -> io::Result<Snapshot> {
    let record = slot.dataset.record(request.record)?;
    let window = if record.is_some() {
        slot.dataset
            .read_range(request.record, request.start, request.count.min(128 * 1024))?
    } else {
        SequenceWindow {
            start: request.start,
            sequence: Vec::new(),
            quality: Vec::new(),
        }
    };
    let mut progress = slot.dataset.progress();
    if let Some(error) = &slot.failure {
        progress.error = Some(error.clone());
    }
    Ok(Snapshot {
        records: slot
            .dataset
            .records(request.list_start, request.list_count.min(256))?,
        format: slot.dataset.format(),
        compression: slot.dataset.compression().to_owned(),
        file_size: slot.file_size,
        record,
        window,
        progress,
        stats: slot.stats.clone(),
        request,
        paused,
    })
}

impl Driver for Backend {
    /// Sequence-domain commands accepted by the shared task runner.
    type Request = Request;

    /// Use the sequence scheduler to choose the worker receive timeout.
    fn timeout(&self) -> Duration {
        Backend::timeout(self)
    }

    /// Coalesce adjacent viewport requests while preserving intervening operation ordering.
    fn handle_requests(&mut self, incoming: impl Iterator<Item = Request>) -> bool {
        // Coalesce adjacent views only. An action between views needs the preceding context.
        let mut pending_view = None;
        for request in incoming {
            if let Request::View(view) = request {
                pending_view = Some(view);
                continue;
            }
            if let Some(view) = pending_view.take() {
                self.set_view(view);
            }
            if !self.request(request) {
                return false;
            }
        }
        if let Some(view) = pending_view {
            self.set_view(view);
        }
        true
    }

    /// Advance the sequence backend once after the worker processes its request batch.
    fn tick(&mut self) -> bool {
        Backend::tick(self)
    }
}
