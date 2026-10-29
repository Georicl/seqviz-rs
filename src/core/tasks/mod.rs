//! Domain-independent background execution, channels and cancellation lifetime.
/// Routing and lifetime identity shared by all domain message payloads.
pub mod protocol;

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, mpsc};
use std::thread::{self, JoinHandle};
use std::time::Duration;

/// Generic worker batching, cancellation and shutdown tests.
#[cfg(test)]
mod tests;

/// A domain backend decides how to accept a batch and advance its current work.
/// The runner only handles waiting, bounded request batches and thread lifetime.
pub trait Driver {
    /// Messages the concrete backend accepts from the task runner.
    type Request;

    /// Return how long the worker may wait for requests before advancing background work.
    fn timeout(&self) -> Duration;
    /// Apply a bounded batch in arrival order; return false to stop the worker.
    fn handle_requests(&mut self, requests: impl Iterator<Item = Self::Request>) -> bool;
    /// Advance one unit of background work; return false to end the worker loop.
    fn tick(&mut self) -> bool;
}

/// Owns request/update channels, cooperative cancellation and the background thread.
pub struct TaskWorker<R, U> {
    /// Sender used by the application to queue backend requests.
    pub requests: mpsc::Sender<R>,
    /// Receiver drained by the application for completed backend work.
    pub updates: mpsc::Receiver<U>,
    /// Shared cooperative cancellation flag observed by background operations.
    pub cancel_background: Arc<AtomicBool>,
    /// Request sent once when the worker is dropped.
    shutdown: Option<R>,
    /// Join handle retained until the backend has shut down.
    thread: Option<JoinHandle<()>>,
}

impl<R: Send + 'static, U: Send + 'static> TaskWorker<R, U> {
    /// Construct the backend on its owning thread so its I/O handles stay there.
    pub fn spawn<B, F>(shutdown: R, build: F) -> Self
    where
        B: Driver<Request = R> + 'static,
        F: FnOnce(mpsc::Sender<U>, Arc<AtomicBool>) -> B + Send + 'static,
    {
        Self::spawn_with_requests(shutdown, std::iter::empty(), build)
    }

    /// Queue initial requests before starting a backend constructed on its owning thread.
    pub fn spawn_with_requests<B, F>(
        shutdown: R,
        initial: impl IntoIterator<Item = R>,
        build: F,
    ) -> Self
    where
        B: Driver<Request = R> + 'static,
        F: FnOnce(mpsc::Sender<U>, Arc<AtomicBool>) -> B + Send + 'static,
    {
        let (send, receive) = mpsc::channel();
        let (updates, events) = mpsc::channel();
        for request in initial {
            let _ = send.send(request);
        }
        let cancel_background = Arc::new(AtomicBool::new(false));
        let cancel = Arc::clone(&cancel_background);
        let thread = thread::spawn(move || run(build(updates, cancel), receive));
        Self {
            requests: send,
            updates: events,
            cancel_background,
            shutdown: Some(shutdown),
            thread: Some(thread),
        }
    }

    /// Queue a request without waiting for processing; ignore a disconnected worker.
    pub fn send(&self, request: R) {
        let _ = self.requests.send(request);
    }
}

impl<R, U> Drop for TaskWorker<R, U> {
    /// Set cancellation, send the shutdown request and join the backend thread.
    fn drop(&mut self) {
        self.cancel_background.store(true, Ordering::Relaxed);
        if let Some(shutdown) = self.shutdown.take() {
            let _ = self.requests.send(shutdown);
        }
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}

/// Wait for a request or timeout, drain up to 511 more requests, then advance the driver once.
fn run<B: Driver>(mut driver: B, requests: mpsc::Receiver<B::Request>) {
    loop {
        let request = requests.recv_timeout(driver.timeout());
        if matches!(request, Err(mpsc::RecvTimeoutError::Disconnected)) {
            return;
        }
        let incoming = request
            .ok()
            .into_iter()
            .chain(requests.try_iter().take(511));
        if !driver.handle_requests(incoming) || !driver.tick() {
            return;
        }
    }
}
