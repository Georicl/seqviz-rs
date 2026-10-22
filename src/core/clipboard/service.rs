use super::{ClipboardOutput, osc52_sequence, system};
use std::{
    env, io,
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, Ordering},
        mpsc,
    },
    thread::{self, JoinHandle},
};

struct CopyRequest {
    operation_id: u64,
    file: usize,
    text: String,
    cancel: Arc<AtomicBool>,
}

pub struct ClipboardUpdate {
    pub operation_id: u64,
    pub file: usize,
    pub result: io::Result<ClipboardOutput>,
}

/// One backend thread and at most one queued copy. Submitting never waits for a command.
pub struct ClipboardService {
    requests: Option<mpsc::SyncSender<CopyRequest>>,
    pub updates: mpsc::Receiver<ClipboardUpdate>,
    state: Arc<Mutex<ServiceState>>,
    thread: Option<JoinHandle<()>>,
}

#[derive(Default)]
struct ServiceState {
    stopping: bool,
    active: Option<Arc<AtomicBool>>,
}

impl ClipboardService {
    pub fn start() -> Self {
        Self::start_with(
            env::var_os("SSH_CONNECTION").is_some() || env::var_os("SSH_TTY").is_some(),
            system::copy,
        )
    }

    fn start_with(
        ssh: bool,
        mut backend: impl FnMut(&str, &AtomicBool) -> io::Result<()> + Send + 'static,
    ) -> Self {
        let (send, receive) = mpsc::sync_channel::<CopyRequest>(1);
        let (updates, events) = mpsc::channel();
        let state = Arc::new(Mutex::new(ServiceState::default()));
        let thread_state = Arc::clone(&state);
        let thread = thread::spawn(move || {
            while let Ok(request) = receive.recv() {
                {
                    let mut state = thread_state.lock().unwrap();
                    if state.stopping {
                        break;
                    }
                    state.active = Some(Arc::clone(&request.cancel));
                }
                let result = prepare(&request.text, ssh, &request.cancel, &mut backend);
                thread_state.lock().unwrap().active = None;
                if !request.cancel.load(Ordering::Relaxed) {
                    let _ = updates.send(ClipboardUpdate {
                        operation_id: request.operation_id,
                        file: request.file,
                        result,
                    });
                }
            }
        });
        Self {
            requests: Some(send),
            updates: events,
            state,
            thread: Some(thread),
        }
    }

    /// The file permission is checked before queueing text or calling a backend.
    pub fn submit(
        &self,
        operation_id: u64,
        file: usize,
        text: String,
        copy_allowed: bool,
        cancel: Arc<AtomicBool>,
    ) -> io::Result<()> {
        if !copy_allowed {
            return Err(io::Error::new(
                io::ErrorKind::PermissionDenied,
                "Copying is disabled for this document",
            ));
        }
        self.requests
            .as_ref()
            .unwrap()
            .try_send(CopyRequest {
                operation_id,
                file,
                text,
                cancel,
            })
            .map_err(|error| match error {
                mpsc::TrySendError::Full(_) => io::Error::new(
                    io::ErrorKind::WouldBlock,
                    "Clipboard is busy; try copying again",
                ),
                mpsc::TrySendError::Disconnected(_) => {
                    io::Error::new(io::ErrorKind::BrokenPipe, "Clipboard service has stopped")
                }
            })
    }
}

impl Drop for ClipboardService {
    fn drop(&mut self) {
        {
            let mut state = self.state.lock().unwrap();
            state.stopping = true;
            if let Some(cancel) = &state.active {
                cancel.store(true, Ordering::Relaxed);
            }
        }
        self.requests.take();
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}

fn prepare(
    text: &str,
    ssh: bool,
    cancel: &AtomicBool,
    backend: &mut impl FnMut(&str, &AtomicBool) -> io::Result<()>,
) -> io::Result<ClipboardOutput> {
    if cancel.load(Ordering::Relaxed) {
        return Err(system::cancelled());
    }
    if !ssh {
        match backend(text, cancel) {
            Ok(()) => return Ok(ClipboardOutput::System),
            Err(error) if error.kind() == io::ErrorKind::Interrupted => return Err(error),
            Err(_) => {}
        }
    }
    if cancel.load(Ordering::Relaxed) {
        return Err(system::cancelled());
    }
    Ok(ClipboardOutput::Osc52(osc52_sequence(text)))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    #[test]
    fn slow_backend_does_not_block_submission_or_caller_work() {
        let caller = thread::current().id();
        let (started, ready) = mpsc::channel();
        let (release, wait) = mpsc::channel();
        let service = ClipboardService::start_with(false, move |text, _| {
            assert_eq!(text, "ACGT");
            assert_ne!(caller, thread::current().id());
            started.send(()).unwrap();
            wait.recv().unwrap();
            Ok(())
        });
        service
            .submit(7, 2, "ACGT".into(), true, Arc::new(AtomicBool::new(false)))
            .unwrap();
        ready.recv_timeout(Duration::from_secs(2)).unwrap();
        assert!(matches!(
            service.updates.try_recv(),
            Err(mpsc::TryRecvError::Empty)
        ));
        // The caller can process other work while the injected backend is still waiting.
        let (send, receive) = mpsc::channel();
        send.send("frame and input processed").unwrap();
        assert_eq!(receive.try_recv().unwrap(), "frame and input processed");
        release.send(()).unwrap();
        let update = service
            .updates
            .recv_timeout(Duration::from_secs(2))
            .unwrap();
        assert_eq!((update.operation_id, update.file), (7, 2));
        assert_eq!(update.result.unwrap(), ClipboardOutput::System);
    }

    #[test]
    fn denied_and_cancelled_requests_never_call_the_backend() {
        let service = ClipboardService::start_with(false, |_, _| panic!("backend called"));
        assert_eq!(
            service
                .submit(
                    1,
                    0,
                    "SAM text".into(),
                    false,
                    Arc::new(AtomicBool::new(false))
                )
                .unwrap_err()
                .kind(),
            io::ErrorKind::PermissionDenied
        );
        service
            .submit(
                2,
                0,
                "cancelled".into(),
                true,
                Arc::new(AtomicBool::new(true)),
            )
            .unwrap();
        assert!(
            service
                .updates
                .recv_timeout(Duration::from_millis(100))
                .is_err()
        );
    }

    #[test]
    fn ssh_and_failed_local_backend_return_osc52_without_terminal_output() {
        let ssh = ClipboardService::start_with(true, |_, _| panic!("server clipboard called"));
        ssh.submit(
            3,
            0,
            "SSH bases".into(),
            true,
            Arc::new(AtomicBool::new(false)),
        )
        .unwrap();
        assert!(matches!(
            ssh.updates
                .recv_timeout(Duration::from_secs(2))
                .unwrap()
                .result
                .unwrap(),
            ClipboardOutput::Osc52(_)
        ));
        let local = ClipboardService::start_with(false, |_, _| Err(io::ErrorKind::NotFound.into()));
        local
            .submit(4, 0, "local".into(), true, Arc::new(AtomicBool::new(false)))
            .unwrap();
        assert!(matches!(
            local
                .updates
                .recv_timeout(Duration::from_secs(2))
                .unwrap()
                .result
                .unwrap(),
            ClipboardOutput::Osc52(_)
        ));
    }

    #[test]
    fn dropping_the_service_cancels_a_registered_backend_before_joining() {
        let (started, ready) = mpsc::channel();
        let service = ClipboardService::start_with(false, move |_, cancel| {
            started.send(()).unwrap();
            while !cancel.load(Ordering::Relaxed) {
                thread::yield_now();
            }
            Err(system::cancelled())
        });
        let cancel = Arc::new(AtomicBool::new(false));
        service
            .submit(6, 0, "bases".into(), true, Arc::clone(&cancel))
            .unwrap();
        ready.recv_timeout(Duration::from_secs(2)).unwrap();
        drop(service);
        assert!(cancel.load(Ordering::Relaxed));
    }

    #[test]
    fn cancelling_an_in_flight_backend_suppresses_its_reply() {
        let (started, ready) = mpsc::channel();
        let service = ClipboardService::start_with(false, move |_, cancel| {
            started.send(()).unwrap();
            while !cancel.load(Ordering::Relaxed) {
                thread::yield_now();
            }
            Err(system::cancelled())
        });
        let cancel = Arc::new(AtomicBool::new(false));
        service
            .submit(5, 0, "bases".into(), true, Arc::clone(&cancel))
            .unwrap();
        ready.recv_timeout(Duration::from_secs(2)).unwrap();
        cancel.store(true, Ordering::Relaxed);
        assert!(
            service
                .updates
                .recv_timeout(Duration::from_millis(100))
                .is_err()
        );
    }
}
