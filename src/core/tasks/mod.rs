//! Domain-independent background execution, channels and cancellation lifetime.
pub mod protocol;

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, mpsc};
use std::thread::{self, JoinHandle};
use std::time::Duration;

#[cfg(test)]
mod tests;

/// A domain backend decides how to accept a batch and advance its current work.
/// The runner only handles waiting, bounded request batches and thread lifetime.
pub trait Driver {
    type Request;

    fn timeout(&self) -> Duration;
    fn handle_requests(&mut self, requests: impl Iterator<Item = Self::Request>) -> bool;
    fn tick(&mut self) -> bool;
}

pub struct TaskWorker<R, U> {
    pub requests: mpsc::Sender<R>,
    pub updates: mpsc::Receiver<U>,
    pub cancel_background: Arc<AtomicBool>,
    shutdown: Option<R>,
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

    pub fn send(&self, request: R) {
        let _ = self.requests.send(request);
    }
}

impl<R, U> Drop for TaskWorker<R, U> {
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
