//! One worker shares I/O time across tabs and checks for new input between scan blocks.
use crate::data::{Dataset, Format, Progress, RecordMeta, RecordSummary, SequenceWindow, Stats};
use std::collections::HashMap;
use std::io;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, mpsc};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

#[derive(Clone, Debug)]
pub struct ViewRequest {
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
        token: u64,
        file: usize,
        query: String,
        after: u64,
    },
    Statistics {
        file: usize,
    },
    Copy {
        token: u64,
        file: usize,
        record: u64,
        range: Option<(u64, u64)>,
    },
    Cancel(usize),
    Close(usize),
    Quit,
}

pub enum Update {
    View(Box<Snapshot>),
    Selected {
        token: u64,
        file: usize,
        record: u64,
    },
    Copied {
        token: u64,
        file: usize,
        text: String,
    },
    Message {
        file: usize,
        text: String,
    },
}

pub struct Worker {
    pub requests: mpsc::Sender<Request>,
    pub updates: mpsc::Receiver<Update>,
    pub cancel_copy: Arc<AtomicBool>,
    thread: Option<JoinHandle<()>>,
}

impl Worker {
    pub fn start(paths: Vec<PathBuf>, cache: PathBuf) -> Self {
        let (send, receive) = mpsc::channel();
        let (updates, events) = mpsc::channel();
        let cancel_copy = Arc::new(AtomicBool::new(false));
        let cancel = Arc::clone(&cancel_copy);
        let thread = thread::spawn(move || run(paths, cache, receive, updates, cancel));
        Self {
            requests: send,
            updates: events,
            cancel_copy,
            thread: Some(thread),
        }
    }

    pub fn send(&self, request: Request) {
        let _ = self.requests.send(request);
    }
}

impl Drop for Worker {
    fn drop(&mut self) {
        self.cancel_copy.store(true, Ordering::Relaxed);
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

struct SearchJob {
    token: u64,
    file: usize,
    query: String,
    after: u64,
    tried: Option<ProgressKey>,
}

struct CopyJob {
    token: u64,
    file: usize,
    record: u64,
    range: Option<(u64, u64)>,
}

fn run(
    paths: Vec<PathBuf>,
    cache: PathBuf,
    requests: mpsc::Receiver<Request>,
    updates: mpsc::Sender<Update>,
    cancel: Arc<AtomicBool>,
) {
    // Aliases retain separate UI state, but must never maintain two live
    // scanners writing the same record database.
    let mut canonical_owners = HashMap::new();
    let owners: Vec<usize> = paths
        .iter()
        .enumerate()
        .map(|(file, path)| {
            let canonical = std::fs::canonicalize(path).unwrap_or_else(|_| path.clone());
            *canonical_owners.entry(canonical).or_insert(file)
        })
        .collect();
    let mut slots: Vec<Option<Slot>> = (0..paths.len()).map(|_| None).collect();
    let mut tabs: Vec<TabState> = (0..paths.len()).map(|_| TabState::default()).collect();
    let mut closed = vec![false; paths.len()];
    let mut view: Option<ViewRequest> = None;
    let mut search: Option<SearchJob> = None;
    let mut pending_copy: Option<CopyJob> = None;
    let mut foreground_read = false;
    let mut dirty = false;
    let mut last_publish = Instant::now() - PUBLISH_INTERVAL;
    let mut working = false;
    loop {
        // A completed or paused tab sleeps until a command arrives. While
        // scanning, check commands between bounded steps without imposing a
        // fixed delay on each block.
        let timeout = if working {
            Duration::ZERO
        } else if dirty {
            PUBLISH_INTERVAL.saturating_sub(last_publish.elapsed())
        } else {
            Duration::from_secs(60)
        };
        let request = requests.recv_timeout(timeout);
        if matches!(request, Err(mpsc::RecvTimeoutError::Disconnected)) {
            break;
        }
        let incoming: Vec<_> = request
            .ok()
            .into_iter()
            .chain(requests.try_iter().take(511))
            .collect();
        let last_view = incoming.iter().rposition(|r| matches!(r, Request::View(_)));
        for (index, request) in incoming.into_iter().enumerate() {
            match request {
                Request::Quit => return,
                Request::View(next) if Some(index) == last_view => {
                    let file = next.file;
                    if file >= slots.len() || closed[file] {
                        continue;
                    }
                    let owner = owners[file];
                    if slots[owner].is_none() {
                        match Dataset::open(&paths[file], &cache) {
                            Ok(dataset) => {
                                slots[owner] = Some(Slot {
                                    dataset,
                                    failure: None,
                                    stats: None,
                                    stats_progress: None,
                                    last_stats: Instant::now() - STATS_INTERVAL,
                                })
                            }
                            Err(error) => {
                                view = None;
                                search = None;
                                pending_copy = None;
                                foreground_read = false;
                                dirty = false;
                                message(&updates, file, error.to_string());
                                continue;
                            }
                        }
                    }
                    // A new position can request its necessary prefix while
                    // indexing is paused. A refresh/resize after Esc cannot.
                    let moved = view.as_ref().is_none_or(|old| {
                        old.file != file || old.record != next.record || old.start != next.start
                    });
                    foreground_read |= moved;
                    if search
                        .as_ref()
                        .is_some_and(|job| job.file != file || job.token != next.token)
                    {
                        search = None;
                    }
                    if pending_copy
                        .as_ref()
                        .is_some_and(|job| job.file != file || job.token != next.token)
                    {
                        pending_copy = None;
                    }
                    view = Some(next);
                    dirty = true;
                    last_publish = Instant::now() - PUBLISH_INTERVAL;
                }
                Request::View(_) => {}
                Request::Search {
                    token,
                    file,
                    query,
                    after,
                } => {
                    if !matches_view(&view, file, token) || query.is_empty() {
                        continue;
                    }
                    search = Some(SearchJob {
                        token,
                        file,
                        query,
                        after,
                        tried: None,
                    });
                    tabs[file].paused = false;
                    dirty = true;
                }
                Request::Statistics { file } => {
                    if view.as_ref().is_none_or(|v| v.file != file) {
                        continue;
                    }
                    tabs[file].statistics = true;
                    tabs[file].paused = false;
                    dirty = true;
                }
                Request::Copy {
                    token,
                    file,
                    record,
                    range,
                } => {
                    if !matches_view(&view, file, token) {
                        continue;
                    }
                    pending_copy = Some(CopyJob {
                        token,
                        file,
                        record,
                        range,
                    });
                    tabs[file].paused = false;
                    message(
                        &updates,
                        file,
                        "Reading selection for clipboard… Esc cancels".into(),
                    );
                    dirty = true;
                }
                Request::Cancel(file) => {
                    if let Some(tab) = tabs.get_mut(file) {
                        tab.paused = true;
                        tab.statistics = false;
                    }
                    if search.as_ref().is_some_and(|job| job.file == file) {
                        search = None;
                    }
                    if pending_copy.as_ref().is_some_and(|job| job.file == file) {
                        pending_copy = None;
                    }
                    if view.as_ref().is_some_and(|v| v.file == file) {
                        foreground_read = false;
                        dirty = true;
                        last_publish = Instant::now() - PUBLISH_INTERVAL;
                    }
                    message(&updates, file, "Background work paused".into());
                }
                Request::Close(file) => {
                    if file >= slots.len() {
                        continue;
                    }
                    closed[file] = true;
                    let owner = owners[file];
                    if !owners
                        .iter()
                        .enumerate()
                        .any(|(other, &candidate)| candidate == owner && !closed[other])
                    {
                        slots[owner] = None;
                    }
                    if view.as_ref().is_some_and(|v| v.file == file) {
                        view = None;
                        foreground_read = false;
                        dirty = false;
                    }
                    if search.as_ref().is_some_and(|job| job.file == file) {
                        search = None;
                    }
                    if pending_copy.as_ref().is_some_and(|job| job.file == file) {
                        pending_copy = None;
                    }
                }
            }
        }
        working = false;
        let Some(current) = &view else {
            continue;
        };
        let file = current.file;
        let Some(slot) = &mut slots[owners[file]] else {
            continue;
        };
        let tab = &mut tabs[file];
        let before = slot.dataset.progress();
        let stopped = before.complete || before.error.is_some() || slot.failure.is_some();
        let needs_view = foreground_read
            && slot
                .dataset
                .record(current.record)
                .ok()
                .flatten()
                .is_none_or(|r| {
                    !r.complete && r.length < current.start.saturating_add(current.count as u64)
                });
        if !needs_view {
            foreground_read = false;
        }
        if !stopped && (!tab.paused || needs_view) {
            match slot.dataset.step(64 * 1024) {
                Ok(after) => dirty |= ProgressKey::from(&before) != ProgressKey::from(&after),
                Err(error) => {
                    slot.failure = Some(error.to_string());
                    message(&updates, file, error.to_string());
                    dirty = true;
                }
            }
        }
        let progress = slot.dataset.progress();
        let key = ProgressKey::from(&progress);
        let stopped = progress.complete || progress.error.is_some() || slot.failure.is_some();
        working = !stopped && (!tab.paused || needs_view);

        if cancel.load(Ordering::Relaxed) {
            search = None;
            pending_copy = None;
        }
        if let Some(job) = &mut search
            && job.file == file
            && job.tried.as_ref().is_none_or(|last| {
                last.records != key.records
                    || last.complete != key.complete
                    || last.errors != key.errors
                    || last.error != key.error
            })
        {
            job.tried = Some(key.clone());
            match slot
                .dataset
                .find_name_cancellable(&job.query, job.after, Arc::clone(&cancel))
            {
                Ok(Some(record)) => {
                    if !cancel.load(Ordering::Relaxed) {
                        let _ = updates.send(Update::Selected {
                            token: job.token,
                            file,
                            record,
                        });
                    }
                    search = None;
                }
                Ok(None) if stopped => {
                    let wrapped = if job.after > 0 {
                        slot.dataset
                            .find_name_cancellable(&job.query, 0, Arc::clone(&cancel))
                    } else {
                        Ok(None)
                    };
                    match wrapped {
                        Ok(Some(record)) if !cancel.load(Ordering::Relaxed) => {
                            let _ = updates.send(Update::Selected {
                                token: job.token,
                                file,
                                record,
                            });
                        }
                        Ok(_) => message(
                            &updates,
                            file,
                            format!("No record matching {:?}", job.query),
                        ),
                        Err(error) if error.kind() == io::ErrorKind::Interrupted => {}
                        Err(error) => message(&updates, file, error.to_string()),
                    }
                    search = None;
                }
                Err(error) => {
                    if error.kind() != io::ErrorKind::Interrupted {
                        message(&updates, file, error.to_string());
                    }
                    search = None;
                }
                _ => {}
            }
        }
        if let Some(job) = &pending_copy
            && job.file == file
        {
            let mut finished = false;
            if !slot.dataset.format().can_copy() {
                message(&updates, file, "Copy is disabled for SAM/BAM".into());
                finished = true;
            } else {
                match slot.dataset.record(job.record) {
                    Ok(Some(meta)) => {
                        let ready = match job.range {
                            Some((start, end)) => {
                                start <= end && (meta.length >= end || meta.complete)
                            }
                            None => meta.complete,
                        };
                        if ready {
                            let result = match job.range {
                                Some((start, end)) => {
                                    copy_range(&mut slot.dataset, job.record, start, end, &cancel)
                                }
                                None => slot.dataset.copy_record(job.record, &cancel),
                            };
                            match result {
                                Ok(text) if !cancel.load(Ordering::Relaxed) => {
                                    let _ = updates.send(Update::Copied {
                                        token: job.token,
                                        file,
                                        text,
                                    });
                                }
                                Ok(_) => {}
                                Err(error) if error.kind() == io::ErrorKind::Interrupted => {}
                                Err(error) => message(&updates, file, error.to_string()),
                            }
                            finished = true;
                        } else if stopped || job.range.is_some_and(|(start, end)| start > end) {
                            message(
                                &updates,
                                file,
                                "Cannot copy an incomplete record or invalid range".into(),
                            );
                            finished = true;
                        }
                    }
                    Ok(None) if stopped => {
                        message(&updates, file, "Record is not available".into());
                        finished = true;
                    }
                    Err(error) => {
                        message(&updates, file, error.to_string());
                        finished = true;
                    }
                    _ => {}
                }
            }
            if finished {
                pending_copy = None;
            }
        }
        // Statistics are opt-in, at most once per second during scanning, and
        // once for the final state. A completed tab reuses its cached result.
        if tab.statistics
            && !cancel.load(Ordering::Relaxed)
            && slot.stats_progress.as_ref() != Some(&key)
            && (slot.stats.is_none() || stopped || slot.last_stats.elapsed() >= STATS_INTERVAL)
        {
            match slot.dataset.stats_cancellable(Arc::clone(&cancel)) {
                Ok(stats) => {
                    slot.stats = Some(stats);
                    slot.stats_progress = Some(key);
                    dirty = true;
                }
                Err(error) => {
                    if error.kind() != io::ErrorKind::Interrupted {
                        message(&updates, file, error.to_string());
                    }
                    tab.statistics = false;
                }
            }
            slot.last_stats = Instant::now();
        }
        if dirty && last_publish.elapsed() >= PUBLISH_INTERVAL {
            match snapshot(slot, current.clone(), tab.paused) {
                Ok(snapshot) => {
                    if updates.send(Update::View(Box::new(snapshot))).is_err() {
                        break;
                    }
                }
                Err(error) => message(&updates, file, error.to_string()),
            }
            dirty = false;
            last_publish = Instant::now();
        }
    }
}

fn matches_view(view: &Option<ViewRequest>, file: usize, token: u64) -> bool {
    view.as_ref()
        .is_some_and(|v| v.file == file && v.token == token)
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
        record,
        window,
        progress,
        stats: slot.stats.clone(),
        request,
        paused,
    })
}

fn copy_range(
    dataset: &mut Dataset,
    record: u64,
    start: u64,
    end: u64,
    cancel: &AtomicBool,
) -> io::Result<String> {
    let mut result = Vec::new();
    let mut offset = start;
    while offset < end {
        if cancel.load(Ordering::Relaxed) {
            return Err(io::Error::new(io::ErrorKind::Interrupted, "Copy cancelled"));
        }
        let window = dataset.read_range(record, offset, (end - offset).min(64 * 1024) as usize)?;
        if window.sequence.is_empty() {
            return Err(io::Error::new(
                io::ErrorKind::UnexpectedEof,
                "Selection is not fully available",
            ));
        }
        offset += window.sequence.len() as u64;
        result.extend(window.sequence);
    }
    String::from_utf8(result).map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))
}

pub fn cache_directory() -> PathBuf {
    if let Some(path) = std::env::var_os("XDG_CACHE_HOME") {
        PathBuf::from(path).join("seqviz-rs")
    } else if let Some(home) = std::env::var_os("HOME") {
        PathBuf::from(home).join(".cache/seqviz-rs")
    } else {
        std::env::temp_dir().join("seqviz-rs")
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    struct Fixture {
        _directory: TempDir,
        paths: Vec<PathBuf>,
        cache: PathBuf,
    }

    impl Fixture {
        fn new(contents: &[&[u8]]) -> Self {
            let directory = TempDir::new().unwrap();
            let paths = contents
                .iter()
                .enumerate()
                .map(|(index, bytes)| {
                    let path = directory.path().join(format!("input-{index}.fa"));
                    std::fs::write(&path, bytes).unwrap();
                    path
                })
                .collect();
            let cache = directory.path().join("cache");
            Self {
                _directory: directory,
                paths,
                cache,
            }
        }

        fn start(&self, initial: Vec<Request>) -> Worker {
            let (send, receive) = mpsc::channel();
            let (updates, events) = mpsc::channel();
            for request in initial {
                send.send(request).unwrap();
            }
            let cancel = Arc::new(AtomicBool::new(false));
            let thread_cancel = Arc::clone(&cancel);
            let paths = self.paths.clone();
            let cache = self.cache.clone();
            let thread = thread::spawn(move || run(paths, cache, receive, updates, thread_cancel));
            Worker {
                requests: send,
                updates: events,
                cancel_copy: cancel,
                thread: Some(thread),
            }
        }
    }

    fn view(token: u64, file: usize, start: u64) -> Request {
        Request::View(ViewRequest {
            token,
            file,
            record: 0,
            start,
            count: 80,
            list_start: 0,
            list_count: 10,
        })
    }

    fn wait_view(worker: &Worker, accept: impl Fn(&Snapshot) -> bool) -> Snapshot {
        let deadline = Instant::now() + Duration::from_secs(5);
        loop {
            let update = worker
                .updates
                .recv_timeout(deadline.saturating_duration_since(Instant::now()))
                .expect("Worker did not publish the requested view");
            if let Update::View(snapshot) = update
                && accept(&snapshot)
            {
                return *snapshot;
            }
        }
    }

    fn assert_quiet(worker: &Worker) {
        assert!(
            matches!(
                worker.updates.recv_timeout(Duration::from_millis(250)),
                Err(mpsc::RecvTimeoutError::Timeout)
            ),
            "Idle worker kept publishing updates"
        );
    }

    #[test]
    fn coalesces_views_before_opening_obsolete_files() {
        let mut fixture = Fixture::new(&[b">first\nAAA\n", b">second\nTTT\n"]);
        fixture.paths[0] = fixture._directory.path().join("does-not-exist.fa");
        let worker = fixture.start(vec![view(1, 0, 0), view(2, 1, 0)]);
        loop {
            match worker.updates.recv_timeout(Duration::from_secs(5)).unwrap() {
                Update::Message { file: 0, text } => panic!("Obsolete file was opened: {text}"),
                Update::View(snapshot) if snapshot.progress.complete => {
                    assert_eq!(snapshot.request.file, 1);
                    assert_eq!(snapshot.request.token, 2);
                    assert_eq!(snapshot.window.sequence, b"TTT");
                    break;
                }
                _ => {}
            }
        }
        assert_quiet(&worker);
    }

    #[test]
    fn cancel_pauses_an_unready_view_and_refresh_does_not_restart_it() {
        let mut data = b">long\n".to_vec();
        data.extend(std::iter::repeat_n(b'A', 300_000));
        let fixture = Fixture::new(&[&data]);
        let worker = fixture.start(vec![view(1, 0, 100_000), Request::Cancel(0)]);
        let paused = wait_view(&worker, |snapshot| snapshot.paused);
        assert_eq!(paused.progress.scanned_bytes, 0);
        worker.send(view(2, 0, 100_000));
        let refreshed = wait_view(&worker, |snapshot| snapshot.request.token == 2);
        assert!(refreshed.paused);
        assert_eq!(refreshed.progress.scanned_bytes, 0);
        assert_quiet(&worker);
        worker.send(Request::Statistics { file: 0 });
        let complete = wait_view(&worker, |snapshot| {
            snapshot.progress.complete && snapshot.stats.is_some()
        });
        assert!(!complete.paused);
        assert_eq!(complete.stats.unwrap().total_length, 300_000);
        assert_quiet(&worker);
    }

    #[test]
    fn completed_statistics_are_cached_and_not_published_repeatedly() {
        let fixture = Fixture::new(&[b">a\nACGT\n>b\nAA\n"]);
        let worker = fixture.start(vec![view(1, 0, 0)]);
        let initial = wait_view(&worker, |snapshot| snapshot.progress.complete);
        assert!(
            initial.stats.is_none(),
            "Browsing alone must not calculate statistics"
        );
        worker.send(Request::Statistics { file: 0 });
        let complete = wait_view(&worker, |snapshot| {
            snapshot.stats.as_ref().is_some_and(|stats| stats.complete)
        });
        assert_eq!(complete.stats.unwrap().total_length, 6);
        assert_quiet(&worker);
    }

    #[test]
    fn closed_tabs_cannot_be_reopened_by_a_stale_view() {
        let fixture = Fixture::new(&[b">closed\nAAA\n", b">open\nCCC\n"]);
        let worker = fixture.start(vec![Request::Close(0), view(2, 1, 0)]);
        wait_view(&worker, |snapshot| snapshot.progress.complete);
        worker.send(view(3, 0, 0));
        assert_quiet(&worker);
    }

    #[test]
    fn copy_results_carry_the_view_token_and_stale_requests_are_ignored() {
        let fixture = Fixture::new(&[b">a\nACGT\n"]);
        let worker = fixture.start(vec![view(2, 0, 0)]);
        wait_view(&worker, |snapshot| snapshot.progress.complete);
        worker.send(Request::Copy {
            token: 1,
            file: 0,
            record: 0,
            range: None,
        });
        assert_quiet(&worker);
        worker.send(Request::Copy {
            token: 2,
            file: 0,
            record: 0,
            range: Some((1, 3)),
        });
        loop {
            match worker.updates.recv_timeout(Duration::from_secs(5)).unwrap() {
                Update::Copied { token, file, text } => {
                    assert_eq!((token, file, text.as_str()), (2, 0, "CG"));
                    break;
                }
                Update::Message { text, .. }
                    if text != "Reading selection for clipboard… Esc cancels" =>
                {
                    panic!("Copy failed: {text}");
                }
                _ => {}
            }
        }
    }

    #[test]
    fn fatal_scan_errors_stop_work_and_do_not_flood_the_update_queue() {
        let fixture = Fixture::new(&[b">valid\nACGT\n>\n"]);
        let worker = fixture.start(vec![view(1, 0, 0)]);
        let failed = wait_view(&worker, |snapshot| snapshot.progress.error.is_some());
        assert!(failed.progress.error.is_some());
        assert_quiet(&worker);
    }

    #[test]
    fn search_uses_tokens_and_wraps_only_after_the_indexed_end() {
        let fixture = Fixture::new(&[b">match-first\nA\n>other\nC\n>match-last\nT\n"]);
        let worker = fixture.start(vec![view(4, 0, 0)]);
        wait_view(&worker, |snapshot| snapshot.progress.complete);
        worker.send(Request::Search {
            token: 3,
            file: 0,
            query: "match".into(),
            after: 0,
        });
        assert_quiet(&worker);
        for (after, expected) in [(0, 0), (1, 2), (3, 0)] {
            worker.send(Request::Search {
                token: 4,
                file: 0,
                query: "match".into(),
                after,
            });
            loop {
                match worker.updates.recv_timeout(Duration::from_secs(5)).unwrap() {
                    Update::Selected {
                        token,
                        file,
                        record,
                    } => {
                        assert_eq!((token, file, record), (4, 0, expected));
                        break;
                    }
                    Update::Message { text, .. } => panic!("Search failed: {text}"),
                    _ => {}
                }
            }
        }
    }

    #[test]
    fn cancelled_actions_do_not_clear_the_shared_cancellation_flag() {
        let fixture = Fixture::new(&[b">match\nACGT\n"]);
        let worker = fixture.start(vec![view(5, 0, 0)]);
        wait_view(&worker, |snapshot| snapshot.progress.complete);
        worker.cancel_copy.store(true, Ordering::Relaxed);
        worker.send(Request::Search {
            token: 5,
            file: 0,
            query: "match".into(),
            after: 0,
        });
        worker.send(Request::Copy {
            token: 5,
            file: 0,
            record: 0,
            range: None,
        });
        let deadline = Instant::now() + Duration::from_millis(250);
        while let Ok(update) = worker
            .updates
            .recv_timeout(deadline.saturating_duration_since(Instant::now()))
        {
            assert!(
                !matches!(update, Update::Copied { .. } | Update::Selected { .. }),
                "A cancelled operation published a successful result"
            );
        }
        assert!(worker.cancel_copy.load(Ordering::Relaxed));
    }

    #[test]
    fn duplicate_paths_share_one_incremental_dataset_across_tab_switches() {
        let mut data = b">long\n".to_vec();
        data.extend(std::iter::repeat_n(b'A', 300_000));
        data.extend_from_slice(b"\n>last\nCCCCCCCC\n");
        let mut fixture = Fixture::new(&[&data]);
        fixture.paths.push(fixture.paths[0].clone());
        let worker = fixture.start(vec![view(1, 0, 0), Request::Cancel(0)]);
        wait_view(&worker, |snapshot| snapshot.paused);
        worker.send(view(2, 0, 150_000));
        let first = wait_view(&worker, |snapshot| {
            snapshot.request.token == 2
                && snapshot
                    .record
                    .as_ref()
                    .is_some_and(|record| record.length >= 150_080)
        });
        assert!(!first.progress.complete);
        worker.send(view(3, 1, 0));
        worker.send(Request::Cancel(1));
        wait_view(&worker, |snapshot| {
            snapshot.request.token == 3 && snapshot.paused
        });
        worker.send(view(4, 0, 100_000));
        worker.send(Request::Statistics { file: 0 });
        let complete = wait_view(&worker, |snapshot| {
            snapshot.request.token == 4
                && (snapshot.progress.complete || snapshot.progress.error.is_some())
        });
        assert!(
            complete.progress.error.is_none(),
            "{:?}",
            complete.progress.error
        );
        assert_eq!(complete.progress.records, 2);
        assert_eq!(complete.records.len(), 2);
        assert_eq!(complete.window.sequence, vec![b'A'; 80]);
        assert_eq!(complete.stats.unwrap().total_length, 300_008);
        worker.send(Request::Close(0));
        worker.send(view(5, 1, 299_990));
        let remaining = wait_view(&worker, |snapshot| snapshot.request.token == 5);
        assert_eq!(remaining.window.sequence, vec![b'A'; 10]);
        assert_eq!(remaining.progress.records, 2);
    }
}
