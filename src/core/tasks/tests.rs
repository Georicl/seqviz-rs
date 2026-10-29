//! Generic worker batching, cancellation and shutdown tests.

use super::*;

/// Minimal messages used to observe batching and ordered shutdown.
enum Request {
    /// Value accumulated for the next batch update.
    Value(u8),
    /// Stop the recorder after checking the cancellation flag.
    Quit,
}

/// Test driver that reports each batch and records cancellation during shutdown.
struct Recorder {
    /// Channel used to report values processed in one batch.
    updates: mpsc::Sender<Vec<u8>>,
    /// Values accumulated since the previous tick.
    values: Vec<u8>,
    /// Cancellation flag supplied by the task worker.
    cancel: Arc<AtomicBool>,
    /// Records whether shutdown observed cancellation.
    stopped: Arc<AtomicBool>,
}

impl Driver for Recorder {
    /// Value and shutdown messages understood by the recorder.
    type Request = Request;

    /// Use a long idle wait so test progress is driven by incoming requests.
    fn timeout(&self) -> Duration {
        Duration::from_secs(60)
    }

    /// Accumulate values and capture the cancellation flag when shutdown is received.
    fn handle_requests(&mut self, requests: impl Iterator<Item = Request>) -> bool {
        for request in requests {
            match request {
                Request::Value(value) => self.values.push(value),
                Request::Quit => {
                    self.stopped
                        .store(self.cancel.load(Ordering::Relaxed), Ordering::Relaxed);
                    return false;
                }
            }
        }
        true
    }

    /// Publish the accumulated batch, stopping if the receiver has gone away.
    fn tick(&mut self) -> bool {
        if !self.values.is_empty() {
            return self.updates.send(std::mem::take(&mut self.values)).is_ok();
        }
        true
    }
}

/// Verify initial batching, subsequent delivery and cancellation before the thread joins.
#[test]
fn batches_requests_before_advancing_and_joins_after_cancellation() {
    let stopped = Arc::new(AtomicBool::new(false));
    let thread_stopped = Arc::clone(&stopped);
    let worker = TaskWorker::spawn_with_requests(
        Request::Quit,
        [Request::Value(1), Request::Value(2)],
        move |updates, cancel| Recorder {
            updates,
            values: Vec::new(),
            cancel,
            stopped: thread_stopped,
        },
    );
    assert_eq!(
        worker.updates.recv_timeout(Duration::from_secs(5)).unwrap(),
        vec![1, 2]
    );
    worker.send(Request::Value(3));
    assert_eq!(
        worker.updates.recv_timeout(Duration::from_secs(5)).unwrap(),
        vec![3]
    );
    drop(worker);
    assert!(stopped.load(Ordering::Relaxed));
}
