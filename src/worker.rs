//! One worker shares I/O time across tabs and checks for new input between scan blocks.
mod operations;
mod protocol;
#[cfg(test)]
mod tests;

use crate::data::{Dataset, Progress, SequenceWindow, Stats};
use operations::{CopyJob, SearchJob};
pub use protocol::{Request, Snapshot, Update, ViewRequest};
use std::collections::HashMap;
use std::io;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, mpsc};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

pub struct Worker {
    pub requests: mpsc::Sender<Request>,
    pub updates: mpsc::Receiver<Update>,
    /// Cancels statistics / shutdown work. Each search or copy owns a separate flag.
    pub cancel_background: Arc<AtomicBool>,
    thread: Option<JoinHandle<()>>,
}

impl Worker {
    pub fn start(paths: Vec<PathBuf>, cache: PathBuf) -> Self {
        let (send, receive) = mpsc::channel();
        let (updates, events) = mpsc::channel();
        let cancel_background = Arc::new(AtomicBool::new(false));
        let cancel = Arc::clone(&cancel_background);
        let thread = thread::spawn(move || run(paths, cache, receive, updates, cancel));
        Self {
            requests: send,
            updates: events,
            cancel_background,
            thread: Some(thread),
        }
    }
    pub fn send(&self, request: Request) {
        let _ = self.requests.send(request);
    }
}

impl Drop for Worker {
    fn drop(&mut self) {
        self.cancel_background.store(true, Ordering::Relaxed);
        let _ = self.requests.send(Request::Quit);
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}

const PUBLISH_INTERVAL: Duration = Duration::from_millis(100);
const STATS_INTERVAL: Duration = Duration::from_secs(1);

struct Slot {
    dataset: Dataset,
    file_size: Option<u64>,
    failure: Option<String>,
    stats: Option<Stats>,
    stats_progress: Option<ProgressKey>,
    last_stats: Instant,
}

#[derive(Default)]
struct TabState {
    paused: bool,
    statistics: bool,
}

#[derive(Clone, PartialEq, Eq)]
struct ProgressKey {
    records: u64,
    bytes: u64,
    complete: bool,
    errors: u64,
    error: Option<String>,
}
impl From<&Progress> for ProgressKey {
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

struct Scheduler {
    paths: Vec<PathBuf>,
    cache: PathBuf,
    owners: Vec<usize>,
    slots: Vec<Option<Slot>>,
    tabs: Vec<TabState>,
    closed: Vec<bool>,
    view: Option<ViewRequest>,
    search: Option<SearchJob>,
    copy: Option<CopyJob>,
    last_operation_id: u64,
    foreground_read: bool,
    dirty: bool,
    last_publish: Instant,
    working: bool,
    updates: mpsc::Sender<Update>,
    cancel: Arc<AtomicBool>,
}

impl Scheduler {
    fn new(
        paths: Vec<PathBuf>,
        cache: PathBuf,
        updates: mpsc::Sender<Update>,
        cancel: Arc<AtomicBool>,
    ) -> Self {
        // Alias tabs keep separate view state and share one live index writer.
        let mut canonical_owners = HashMap::new();
        let owners = paths
            .iter()
            .enumerate()
            .map(|(file, path)| {
                let canonical = std::fs::canonicalize(path).unwrap_or_else(|_| path.clone());
                *canonical_owners.entry(canonical).or_insert(file)
            })
            .collect();
        let count = paths.len();
        Self {
            paths,
            cache,
            owners,
            slots: (0..count).map(|_| None).collect(),
            tabs: (0..count).map(|_| TabState::default()).collect(),
            closed: vec![false; count],
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

    fn timeout(&self) -> Duration {
        if self.working {
            Duration::ZERO
        } else if self.dirty {
            PUBLISH_INTERVAL.saturating_sub(self.last_publish.elapsed())
        } else {
            Duration::from_secs(60)
        }
    }

    fn cancel_operations(&mut self) {
        if let Some(job) = self.search.take() {
            job.cancel.store(true, Ordering::Relaxed);
        }
        if let Some(job) = self.copy.take() {
            job.cancel.store(true, Ordering::Relaxed);
        }
    }

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

    fn set_view(&mut self, next: ViewRequest) {
        let file = next.file;
        if file >= self.slots.len() || self.closed[file] {
            return;
        }
        let owner = self.owners[file];
        if self.slots[owner].is_none() {
            match Dataset::open(&self.paths[file], &self.cache) {
                Ok(dataset) => {
                    self.slots[owner] = Some(Slot {
                        dataset,
                        file_size: std::fs::metadata(&self.paths[file]).ok().map(|m| m.len()),
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

    /// Returns false on Quit; all other requests update scheduling state only.
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

    fn close(&mut self, file: usize) {
        if file >= self.slots.len() {
            return;
        }
        self.closed[file] = true;
        let owner = self.owners[file];
        if !self
            .owners
            .iter()
            .enumerate()
            .any(|(other, &candidate)| candidate == owner && !self.closed[other])
        {
            self.slots[owner] = None;
        }
        if self.view.as_ref().is_some_and(|v| v.file == file) {
            self.cancel_operations();
            self.view = None;
            self.foreground_read = false;
            self.dirty = false;
        }
    }

    fn tick(&mut self) -> bool {
        self.working = false;
        let Some(current) = &self.view else {
            return true;
        };
        let file = current.file;
        let Some(slot) = &mut self.slots[self.owners[file]] else {
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

fn run(
    paths: Vec<PathBuf>,
    cache: PathBuf,
    requests: mpsc::Receiver<Request>,
    updates: mpsc::Sender<Update>,
    cancel: Arc<AtomicBool>,
) {
    let mut scheduler = Scheduler::new(paths, cache, updates, cancel);
    loop {
        let request = requests.recv_timeout(scheduler.timeout());
        if matches!(request, Err(mpsc::RecvTimeoutError::Disconnected)) {
            return;
        }
        let incoming = request
            .ok()
            .into_iter()
            .chain(requests.try_iter().take(511));
        // Coalesce adjacent views only. An action between views needs the preceding context.
        let mut pending_view = None;
        for request in incoming {
            if let Request::View(view) = request {
                pending_view = Some(view);
                continue;
            }
            if let Some(view) = pending_view.take() {
                scheduler.set_view(view);
            }
            if !scheduler.request(request) {
                return;
            }
        }
        if let Some(view) = pending_view {
            scheduler.set_view(view);
        }
        if !scheduler.tick() {
            return;
        }
    }
}

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

fn message(updates: &mpsc::Sender<Update>, file: usize, text: String) {
    let _ = updates.send(Update::Message { file, text });
}

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
