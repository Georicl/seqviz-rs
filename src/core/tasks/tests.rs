use super::*;

enum Request {
    Value(u8),
    Quit,
}

struct Recorder {
    updates: mpsc::Sender<Vec<u8>>,
    values: Vec<u8>,
    cancel: Arc<AtomicBool>,
    stopped: Arc<AtomicBool>,
}

impl Driver for Recorder {
    type Request = Request;

    fn timeout(&self) -> Duration {
        Duration::from_secs(60)
    }

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

    fn tick(&mut self) -> bool {
        if !self.values.is_empty() {
            return self.updates.send(std::mem::take(&mut self.values)).is_ok();
        }
        true
    }
}

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
