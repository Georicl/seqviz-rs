//! Sequence worker regressions for scheduling, cancellation and alias-tab ownership.
use super::*;
use crate::core::tasks::TaskWorker;
use tempfile::TempDir;

/// Task runner specialised to sequence request and update messages.
type Worker = TaskWorker<Request, Update>;

/// Large fixtures need time to finish in debug builds on shared CI hosts.
const LARGE_FIXTURE_TIMEOUT: Duration = Duration::from_secs(60);

/// Temporary input files and cache directory kept alive for a backend test.
struct Fixture {
    /// Keeps fixture files and generated caches alive for the test.
    _directory: TempDir,
    /// Source paths supplied to the worker, including any alias tabs added by a test.
    paths: Vec<PathBuf>,
    /// Fixture-local cache root for indexes and source checkpoints.
    cache: PathBuf,
}

impl Fixture {
    /// Write each source into one temporary fixture directory and reserve a separate cache path.
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

    /// Start a sequence backend with requests queued before its worker thread begins processing.
    fn start(&self, initial: Vec<Request>) -> Worker {
        let paths = self.paths.clone();
        let cache = self.cache.clone();
        TaskWorker::spawn_with_requests(Request::Quit, initial, move |updates, cancel| {
            Backend::new(SessionFiles::new(paths), cache, updates, cancel)
        })
    }
}

/// Build an 80-base viewport request for record zero at a chosen sequence offset.
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

/// Wait up to five seconds for a snapshot satisfying the test's acceptance predicate.
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

/// Check that an idle worker publishes no update during a short observation window.
fn assert_quiet(worker: &Worker) {
    assert!(
        matches!(
            worker.updates.recv_timeout(Duration::from_millis(250)),
            Err(mpsc::RecvTimeoutError::Timeout)
        ),
        "Idle worker kept publishing updates"
    );
}

/// Check adjacent view requests skip opening a superseded missing source.
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

/// Check a paused unresolved viewport remains paused until an explicit statistics request.
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

/// Check statistics are opt-in and a completed result does not produce recurring updates.
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

/// Check stale viewport requests cannot resurrect a closed tab.
#[test]
fn closed_tabs_cannot_be_reopened_by_a_stale_view() {
    let fixture = Fixture::new(&[b">closed\nAAA\n", b">open\nCCC\n"]);
    let worker = fixture.start(vec![Request::Close(0), view(2, 1, 0)]);
    wait_view(&worker, |snapshot| snapshot.progress.complete);
    worker.send(view(3, 0, 0));
    assert_quiet(&worker);
}

/// Create a fresh uncancelled flag for one test operation.
fn action_cancel() -> Arc<AtomicBool> {
    Arc::new(AtomicBool::new(false))
}

/// Allow viewport traffic while rejecting replies from stale operations.
fn assert_no_action_updates(worker: &Worker) {
    let deadline = Instant::now() + Duration::from_millis(250);
    while let Ok(update) = worker
        .updates
        .recv_timeout(deadline.saturating_duration_since(Instant::now()))
    {
        assert!(
            !matches!(
                update,
                Update::Selected { .. } | Update::Copied { .. } | Update::OperationFailed { .. }
            ),
            "A stale operation produced a reply"
        );
    }
}

/// Check copied ranges retain their operation IDs and older operations receive no reply.
#[test]
fn copy_results_carry_operation_ids_and_stale_requests_are_ignored() {
    let fixture = Fixture::new(&[b">a\nACGT\n"]);
    let worker = fixture.start(vec![view(2, 0, 0)]);
    wait_view(&worker, |snapshot| snapshot.progress.complete);
    worker.send(Request::Copy {
        operation_id: 10,
        file: 0,
        record: 0,
        range: Some((1, 3)),
        cancel: action_cancel(),
    });
    loop {
        match worker.updates.recv_timeout(Duration::from_secs(5)).unwrap() {
            Update::Copied {
                operation_id,
                file,
                text,
            } => {
                assert_eq!((operation_id, file, text.as_str()), (10, 0, "CG"));
                break;
            }
            Update::OperationFailed { text, .. } => panic!("Copy failed: {text}"),
            _ => {}
        }
    }
    worker.send(Request::Copy {
        operation_id: 9,
        file: 0,
        record: 0,
        range: None,
        cancel: action_cancel(),
    });
    assert_no_action_updates(&worker);
}

/// Check a fatal parser error produces a terminal snapshot without repeated idle updates.
#[test]
fn fatal_scan_errors_stop_work_and_do_not_flood_the_update_queue() {
    let fixture = Fixture::new(&[b">valid\nACGT\n>\n"]);
    let worker = fixture.start(vec![view(1, 0, 0)]);
    let failed = wait_view(&worker, |snapshot| snapshot.progress.error.is_some());
    assert!(failed.progress.error.is_some());
    assert_quiet(&worker);
}

/// Check search identities, inclusive starting IDs and wraparound after the final indexed record.
#[test]
fn search_uses_operation_ids_and_wraps_only_after_the_indexed_end() {
    let fixture = Fixture::new(&[b">match-first\nA\n>other\nC\n>match-last\nT\n"]);
    let worker = fixture.start(vec![view(4, 0, 0)]);
    wait_view(&worker, |snapshot| snapshot.progress.complete);
    for (operation_id, after, expected) in [(1, 0, 0), (2, 1, 2), (3, 3, 0)] {
        worker.send(Request::Search {
            operation_id,
            file: 0,
            query: "match".into(),
            after,
            cancel: action_cancel(),
        });
        loop {
            match worker.updates.recv_timeout(Duration::from_secs(5)).unwrap() {
                Update::Selected {
                    operation_id: actual,
                    file,
                    record,
                } => {
                    assert_eq!((actual, file, record), (operation_id, 0, expected));
                    break;
                }
                Update::OperationFailed { text, .. } => panic!("Search failed: {text}"),
                _ => {}
            }
        }
    }
    worker.send(Request::Search {
        operation_id: 2,
        file: 0,
        query: "match".into(),
        after: 0,
        cancel: action_cancel(),
    });
    assert_no_action_updates(&worker);
}

/// Check cancelled requests remain cancelled even after a new operation succeeds.
#[test]
fn cancelled_actions_keep_their_flags_and_never_publish_success() {
    let fixture = Fixture::new(&[b">match\nACGT\n"]);
    let worker = fixture.start(vec![view(5, 0, 0)]);
    wait_view(&worker, |snapshot| snapshot.progress.complete);
    let cancelled = Arc::new(AtomicBool::new(true));
    worker.send(Request::Search {
        operation_id: 1,
        file: 0,
        query: "match".into(),
        after: 0,
        cancel: Arc::clone(&cancelled),
    });
    worker.send(Request::Copy {
        operation_id: 2,
        file: 0,
        record: 0,
        range: None,
        cancel: Arc::clone(&cancelled),
    });
    assert_quiet(&worker);
    assert!(cancelled.load(Ordering::Relaxed));
    // A new action owns a new flag; it cannot resurrect either old action.
    worker.send(Request::Copy {
        operation_id: 3,
        file: 0,
        record: 0,
        range: Some((0, 1)),
        cancel: action_cancel(),
    });
    loop {
        if let Update::Copied {
            operation_id, text, ..
        } = worker.updates.recv_timeout(Duration::from_secs(5)).unwrap()
        {
            assert_eq!(operation_id, 3);
            assert_eq!(text, "A");
            break;
        }
    }
    assert!(cancelled.load(Ordering::Relaxed));
}

/// Check a large full-record copy survives adjacent viewport generations unchanged.
#[test]
fn a_32_mib_copy_survives_view_resizes_without_losing_operation_identity() {
    let length = 32 * 1024 * 1024;
    let mut data = b">large full header\n".to_vec();
    data.extend(std::iter::repeat_n(b'A', length));
    data.push(b'\n');
    let fixture = Fixture::new(&[&data]);
    let cancel = action_cancel();
    let worker = fixture.start(vec![
        view(1, 0, 0),
        Request::Copy {
            operation_id: 71,
            file: 0,
            record: 0,
            range: None,
            cancel: Arc::clone(&cancel),
        },
        view(2, 0, 0),
        view(3, 0, 0),
    ]);
    let deadline = Instant::now() + LARGE_FIXTURE_TIMEOUT;
    loop {
        match worker
            .updates
            .recv_timeout(deadline.saturating_duration_since(Instant::now()))
            .unwrap()
        {
            Update::Copied {
                operation_id,
                file,
                text,
            } => {
                assert_eq!((operation_id, file), (71, 0));
                let sequence = text.strip_prefix(">large full header\n").unwrap();
                assert_eq!(sequence.bytes().filter(|b| *b != b'\n').count(), length);
                assert!(sequence.bytes().all(|b| b == b'A' || b == b'\n'));
                break;
            }
            Update::OperationFailed { text, .. } => panic!("Copy failed: {text}"),
            _ => {}
        }
    }
    assert!(!cancel.load(Ordering::Relaxed));
}

/// Check a pending name search survives viewport refreshes while scanning a large preceding record.
#[test]
fn search_survives_layout_generations_until_its_target_is_indexed() {
    let mut data = b">large\n".to_vec();
    data.extend(std::iter::repeat_n(b'A', 32 * 1024 * 1024));
    data.extend_from_slice(b"\n>target-read\nCGT\n");
    let fixture = Fixture::new(&[&data]);
    let cancel = action_cancel();
    let worker = fixture.start(vec![
        view(1, 0, 0),
        Request::Search {
            operation_id: 82,
            file: 0,
            query: "target".into(),
            after: 0,
            cancel: Arc::clone(&cancel),
        },
        view(2, 0, 0),
        view(3, 0, 0),
    ]);
    let deadline = Instant::now() + LARGE_FIXTURE_TIMEOUT;
    loop {
        match worker
            .updates
            .recv_timeout(deadline.saturating_duration_since(Instant::now()))
            .unwrap()
        {
            Update::Selected {
                operation_id,
                file,
                record,
            } => {
                assert_eq!((operation_id, file, record), (82, 0, 1));
                break;
            }
            Update::OperationFailed { text, .. } => panic!("Search failed: {text}"),
            _ => {}
        }
    }
    assert!(!cancel.load(Ordering::Relaxed));
}

/// Check cancel, close, file changes and record changes terminate a pending copy.
#[test]
fn context_changes_and_explicit_cancel_end_pending_operations() {
    for change in [
        Request::Cancel(0),
        Request::Close(0),
        view(2, 1, 0),
        Request::View(ViewRequest {
            token: 2,
            file: 0,
            record: 1,
            start: 0,
            count: 80,
            list_start: 0,
            list_count: 10,
        }),
    ] {
        let fixture = Fixture::new(&[b">first\nAAA\n>last\nCCC\n", b">other\nTTT\n"]);
        let cancel = action_cancel();
        let worker = fixture.start(vec![
            view(1, 0, 0),
            Request::Copy {
                operation_id: 1,
                file: 0,
                record: 0,
                range: None,
                cancel: Arc::clone(&cancel),
            },
            change,
        ]);
        let deadline = Instant::now() + Duration::from_millis(250);
        while let Ok(update) = worker
            .updates
            .recv_timeout(deadline.saturating_duration_since(Instant::now()))
        {
            assert!(
                !matches!(update, Update::Copied { .. }),
                "Cancelled copy completed"
            );
        }
        assert!(cancel.load(Ordering::Relaxed));
    }
}

/// Check alias tabs reuse partial scan progress and retain the dataset until the last tab closes.
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
