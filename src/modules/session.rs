//! File ownership across tabs and the application's background session handle.
use super::protocol::{ModuleKind, ModuleRequest, ModuleUpdate};
use super::{registry, sequence, vcf};
use crate::core::{
    source::Source,
    tasks::{Driver, TaskWorker},
};
use std::collections::HashMap;
use std::ops::Deref;
use std::path::{Path, PathBuf};
use std::{
    io::{self, Read},
    sync::{Arc, atomic::AtomicBool, mpsc},
    time::Duration,
};

/// Alias tabs keep independent view state and share a single backend dataset.
pub(crate) struct SessionFiles {
    /// Original input paths indexed by stable tab ID.
    paths: Vec<PathBuf>,
    /// First tab ID for each canonical path, shared by alias tabs.
    owners: Vec<usize>,
    /// Per-tab closure flags used to find the final live alias.
    closed: Vec<bool>,
}

/// Resolve canonical paths to stable dataset owners while retaining independent tab slots.
impl SessionFiles {
    /// Group aliases by canonical path, falling back to the original path if resolution fails.
    pub(crate) fn new(paths: Vec<PathBuf>) -> Self {
        let mut canonical_owners = HashMap::new();
        let owners = paths
            .iter()
            .enumerate()
            .map(|(file, path)| {
                let canonical = std::fs::canonicalize(path).unwrap_or_else(|_| path.clone());
                *canonical_owners.entry(canonical).or_insert(file)
            })
            .collect();
        let closed = vec![false; paths.len()];
        Self {
            paths,
            owners,
            closed,
        }
    }

    /// Return the number of original input slots, including closed tabs and aliases.
    pub(crate) fn len(&self) -> usize {
        self.paths.len()
    }

    /// Return the original input path for a valid tab ID.
    pub(crate) fn path(&self, file: usize) -> &Path {
        &self.paths[file]
    }

    /// Return the first tab ID whose canonical path matches this tab's path.
    pub(crate) fn owner(&self, file: usize) -> usize {
        self.owners[file]
    }

    /// Check that a tab ID exists and has not been closed.
    pub(crate) fn is_open(&self, file: usize) -> bool {
        self.closed.get(file).is_some_and(|closed| !closed)
    }

    /// Return the owner and whether closing this tab removes its final alias.
    pub(crate) fn close(&mut self, file: usize) -> Option<(usize, bool)> {
        *self.closed.get_mut(file)? = true;
        let owner = self.owner(file);
        let last_alias = !self
            .owners
            .iter()
            .enumerate()
            .any(|(other, &candidate)| candidate == owner && !self.closed[other]);
        Some((owner, last_alias))
    }
}

/// Application-facing handle for session requests, updates and background cancellation.
pub struct Worker {
    /// Generic transport and thread lifetime for the domain-aware session.
    tasks: TaskWorker<ModuleRequest, ModuleUpdate>,
}

impl Worker {
    /// Construct the session and its domain backends on the background worker thread.
    pub fn start(paths: Vec<PathBuf>, cache: PathBuf) -> Self {
        Self {
            // The closure receives an update sender and the shared cancellation flag.
            // Requests travel in the opposite direction through TaskWorker's request channel.
            tasks: TaskWorker::spawn(ModuleRequest::Quit, move |updates, cancel| {
                Session::new(paths, cache, updates, cancel)
            }),
        }
    }

    /// Forward a routed request to the generic task worker without waiting for a reply.
    pub(crate) fn send_module(&self, request: ModuleRequest) {
        self.tasks.send(request);
    }

    /// Keep the sequence request entry point for existing library callers.
    pub fn send(&self, request: sequence::protocol::Request) {
        self.send_module(ModuleRequest::sequence(request));
    }
}

impl Deref for Worker {
    /// Generic task transport exposed through the session handle.
    type Target = TaskWorker<ModuleRequest, ModuleUpdate>;

    /// Expose the underlying worker channels and cancellation handle to existing callers.
    fn deref(&self) -> &Self::Target {
        &self.tasks
    }
}

/// One runner owns both domain backends; only the active domain advances I/O.
struct Session {
    /// Session-level tab and canonical-path ownership state.
    files: SessionFiles,
    /// Root directory used for source probing and backend indexes.
    cache: PathBuf,
    /// Detected module per dataset owner; aliases resolve through `files`.
    modules: Vec<Option<ModuleKind>>,
    /// Domain whose incremental I/O may advance on the next tick.
    active: Option<ModuleKind>,
    /// Sequence backend and its owned datasets.
    sequence: sequence::backend::Backend,
    /// VCF backend and its owned datasets.
    vcf: vcf::backend::Backend,
    /// Sequence replies waiting to be wrapped and forwarded.
    sequence_updates: mpsc::Receiver<sequence::protocol::Update>,
    /// VCF replies waiting to be wrapped and forwarded.
    vcf_updates: mpsc::Receiver<vcf::protocol::Update>,
    /// Application-facing channel for probe results and domain replies.
    updates: mpsc::Sender<ModuleUpdate>,
}

/// Coordinate format detection, request routing and incremental work across both domains.
impl Session {
    /// Create both backends and update channels with matching, independently owned file maps.
    fn new(
        paths: Vec<PathBuf>,
        cache: PathBuf,
        updates: mpsc::Sender<ModuleUpdate>,
        cancel: Arc<AtomicBool>,
    ) -> Self {
        let (sequence_send, sequence_updates) = mpsc::channel();
        let (vcf_send, vcf_updates) = mpsc::channel();
        Self {
            modules: vec![None; paths.len()],
            active: None,
            files: SessionFiles::new(paths.clone()),
            sequence: sequence::backend::Backend::new(
                SessionFiles::new(paths.clone()),
                cache.clone(),
                sequence_send,
                cancel,
            ),
            vcf: vcf::backend::Backend::new(SessionFiles::new(paths), cache.clone(), vcf_send),
            cache,
            sequence_updates,
            vcf_updates,
            updates,
        }
    }

    /// Pause the active scan, probe up to 8192 decoded bytes and report the detected domain.
    fn open(&mut self, file: usize) {
        if !self.files.is_open(file) {
            return;
        }
        self.active = None;
        let owner = self.files.owner(file);
        let detected: io::Result<ModuleKind> = self.modules[owner].map(Ok).unwrap_or_else(|| {
            std::fs::create_dir_all(&self.cache)?;
            let source = Source::open(self.files.path(file), &self.cache)?;
            let mut prefix = Vec::with_capacity(8192);
            source.take(8192).read_to_end(&mut prefix)?;
            let module = registry::probe(&prefix, self.files.path(file)).ok_or_else(|| {
                io::Error::new(
                    io::ErrorKind::InvalidData,
                    "Input is not recognised as FASTA, FASTQ, SAM, BAM or VCF",
                )
            })?;
            Ok(module.kind)
        });
        let update = match detected {
            Ok(module) => {
                self.modules[owner] = Some(module);
                ModuleUpdate::Opened { file, module }
            }
            Err(error) => ModuleUpdate::OpenFailed {
                file,
                text: error.to_string(),
            },
        };
        let _ = self.updates.send(update);
    }

    /// Drain queued sequence requests, then VCF requests, stopping if either backend exits.
    fn flush(
        &mut self,
        sequence: &mut Vec<sequence::protocol::Request>,
        vcf: &mut Vec<vcf::protocol::Request>,
    ) -> bool {
        self.sequence.handle_requests(sequence.drain(..)) && self.vcf.handle_requests(vcf.drain(..))
    }
}

impl Driver for Session {
    /// Routed lifecycle and domain messages accepted by this session.
    type Request = ModuleRequest;

    /// Use the active backend's polling interval, or an idle minute with no active domain.
    fn timeout(&self) -> Duration {
        match self.active {
            Some(ModuleKind::Sequence) => self.sequence.timeout(),
            Some(ModuleKind::Vcf) => self.vcf.timeout(),
            None => Duration::from_secs(60),
        }
    }

    /// Route requests in order, flush at domain/lifecycle boundaries and ignore closed-tab views.
    fn handle_requests(&mut self, requests: impl Iterator<Item = ModuleRequest>) -> bool {
        let mut sequence = Vec::new();
        let mut vcf = Vec::new();
        for request in requests {
            match request {
                ModuleRequest::Sequence(envelope) => {
                    if !vcf.is_empty() && !self.flush(&mut sequence, &mut vcf) {
                        return false;
                    }
                    if let sequence::protocol::Request::View(view) = &envelope.payload {
                        if !self.files.is_open(view.file) {
                            continue;
                        }
                        self.active = Some(ModuleKind::Sequence);
                    }
                    sequence.push(envelope.payload);
                }
                ModuleRequest::Vcf(envelope) => {
                    if !sequence.is_empty() && !self.flush(&mut sequence, &mut vcf) {
                        return false;
                    }
                    if let vcf::protocol::Request::View(view) = &envelope.payload {
                        if !self.files.is_open(view.file) {
                            continue;
                        }
                        self.active = Some(ModuleKind::Vcf);
                    }
                    vcf.push(envelope.payload);
                }
                request => {
                    if !self.flush(&mut sequence, &mut vcf) {
                        return false;
                    }
                    match request {
                        ModuleRequest::Open { file } => self.open(file),
                        ModuleRequest::Close { file } => {
                            if self.files.close(file).is_some() {
                                self.sequence.handle_requests(std::iter::once(
                                    sequence::protocol::Request::Close(file),
                                ));
                                self.vcf.handle_requests(std::iter::once(
                                    vcf::protocol::Request::Close(file),
                                ));
                            }
                        }
                        ModuleRequest::Quit => {
                            self.sequence.handle_requests(std::iter::once(
                                sequence::protocol::Request::Quit,
                            ));
                            self.vcf
                                .handle_requests(std::iter::once(vcf::protocol::Request::Quit));
                            return false;
                        }
                        _ => unreachable!(),
                    }
                }
            }
        }
        self.flush(&mut sequence, &mut vcf)
    }

    /// Advance only the active backend and forward both domains' pending replies to the UI.
    fn tick(&mut self) -> bool {
        let keep_running = match self.active {
            Some(ModuleKind::Sequence) => self.sequence.tick(),
            Some(ModuleKind::Vcf) => self.vcf.tick(),
            None => true,
        };
        if !keep_running {
            return false;
        }
        for update in self.sequence_updates.try_iter() {
            if self.updates.send(ModuleUpdate::sequence(update)).is_err() {
                return false;
            }
        }
        for update in self.vcf_updates.try_iter() {
            if self.updates.send(ModuleUpdate::vcf(update)).is_err() {
                return false;
            }
        }
        true
    }
}

/// Session routing and foreground scan ownership regressions.
#[cfg(test)]
mod tests {
    use super::*;

    /// Verify a failed foreground probe leaves the previous domain paused until requested again.
    #[test]
    fn failed_foreground_open_stops_the_previous_module_scan() {
        let directory = tempfile::tempdir().unwrap();
        let vcf = directory.path().join("variants.vcf");
        let missing = directory.path().join("missing.vcf");
        let header = "##fileformat=VCFv4.3\n#CHROM\tPOS\tID\tREF\tALT\tQUAL\tFILTER\tINFO\n";
        std::fs::write(
            &vcf,
            format!("{header}{}", "chr1\t1\t.\tA\tT\t.\tPASS\t.\n".repeat(20000)),
        )
        .unwrap();
        let (send, receive) = mpsc::channel();
        let mut session = Session::new(
            vec![vcf, missing],
            directory.path().join("cache"),
            send,
            Arc::new(AtomicBool::new(false)),
        );
        let view = |token| {
            ModuleRequest::vcf(vcf::protocol::Request::View(vcf::protocol::ViewRequest {
                file: 0,
                token,
                contig: None,
                selected: 0,
                start: 0,
                count: 1,
            }))
        };
        assert!(session.handle_requests([view(1)].into_iter()));
        assert!(session.tick());
        let before = receive
            .try_iter()
            .find_map(|update| match update {
                ModuleUpdate::Vcf(envelope) => match envelope.payload {
                    vcf::protocol::Update::View(snapshot) => Some(snapshot.progress),
                    _ => None,
                },
                _ => None,
            })
            .unwrap();
        assert!(!before.complete);
        assert!(session.handle_requests([ModuleRequest::Open { file: 1 }].into_iter()));
        assert!(
            receive
                .try_iter()
                .any(|update| matches!(update, ModuleUpdate::OpenFailed { file: 1, .. }))
        );
        for _ in 0..10 {
            assert!(session.tick());
        }
        assert!(session.handle_requests([view(2)].into_iter()));
        assert!(session.tick());
        let after = receive
            .try_iter()
            .find_map(|update| match update {
                ModuleUpdate::Vcf(envelope) => match envelope.payload {
                    vcf::protocol::Update::View(snapshot) if snapshot.request.token == 2 => {
                        Some(snapshot.progress)
                    }
                    _ => None,
                },
                _ => None,
            })
            .unwrap();
        assert_eq!(after.scanned_bytes, before.scanned_bytes + 64 * 1024);
    }
}
