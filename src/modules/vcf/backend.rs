//! Incremental indexing and position navigation on the session's I/O thread.
use super::data::Dataset;
use super::protocol::{Request, Snapshot, Update, ViewRequest};
use crate::core::tasks::Driver;
use crate::modules::session::SessionFiles;
use std::{
    io,
    path::PathBuf,
    sync::mpsc,
    time::{Duration, Instant},
};

/// Throttle scan-progress snapshots; explicit view or cancel requests can publish immediately.
const PUBLISH_INTERVAL: Duration = Duration::from_millis(100);

/// One shared dataset and any nonpersistent failure that stopped its scan.
struct Slot {
    /// The source/cache pair shared by tabs with the same canonical owner.
    dataset: Dataset,
    /// A storage or source failure that stops work for this open dataset.
    failure: Option<String>,
}

/// An outstanding genomic-position search tied to a tab and request token.
struct Locate {
    /// Tab that initiated the lookup.
    file: usize,
    /// Lookup generation echoed in success and failure updates.
    token: u64,
    /// Exact contig identifier to search.
    contig: String,
    /// Requested genomic coordinate in VCF POS units.
    position: u64,
}

/// Own shared VCF datasets and advance work for the current viewport.
pub(crate) struct Backend {
    /// Canonical dataset ownership and per-tab open state.
    files: SessionFiles,
    /// Parent directory for VCF source caches.
    cache: PathBuf,
    /// Lazily opened datasets stored at their canonical owner indices.
    slots: Vec<Option<Slot>>,
    /// Per-tab background indexing pause flags.
    paused: Vec<bool>,
    /// Most recent viewport to serve.
    view: Option<ViewRequest>,
    /// Outstanding position lookup, if any.
    locate: Option<Locate>,
    /// Whether navigation may scan enough records to fill the requested viewport.
    foreground_read: bool,
    /// Whether the current snapshot needs to be published.
    dirty: bool,
    /// Whether another scan step can make progress without waiting for a request.
    working: bool,
    /// Timestamp used to throttle viewport snapshots.
    last_publish: Instant,
    /// Channel carrying backend responses to the session.
    updates: mpsc::Sender<Update>,
}

impl Backend {
    /// Create empty dataset slots and per-tab pause state without opening sources.
    pub(crate) fn new(files: SessionFiles, cache: PathBuf, updates: mpsc::Sender<Update>) -> Self {
        let count = files.len();
        Self {
            files,
            cache,
            slots: (0..count).map(|_| None).collect(),
            paused: vec![false; count],
            view: None,
            locate: None,
            foreground_read: false,
            dirty: false,
            working: false,
            last_publish: Instant::now() - PUBLISH_INTERVAL,
            updates,
        }
    }

    /// Open the shared dataset on demand and schedule a fresh viewport snapshot.
    /// A changed position may read the required prefix even while background scanning is paused.
    fn set_view(&mut self, next: ViewRequest) {
        if !self.files.is_open(next.file) {
            return;
        }
        let owner = self.files.owner(next.file);
        if self.slots[owner].is_none() {
            match Dataset::open(self.files.path(next.file), &self.cache) {
                Ok(dataset) => {
                    self.slots[owner] = Some(Slot {
                        dataset,
                        failure: None,
                    })
                }
                Err(error) => {
                    self.message(next.file, error.to_string());
                    self.view = None;
                    self.locate = None;
                    self.foreground_read = false;
                    self.working = false;
                    self.dirty = false;
                    return;
                }
            }
        }
        let moved = self.view.as_ref().is_none_or(|old| {
            old.file != next.file
                || old.contig != next.contig
                || old.selected != next.selected
                || old.start != next.start
        });
        if self
            .locate
            .as_ref()
            .is_some_and(|job| job.file != next.file || job.token != next.token)
        {
            self.locate = None;
        }
        // An unchanged refresh after Esc does not restart indexing. Moving may read its prefix.
        self.foreground_read |= moved;
        self.view = Some(next);
        self.dirty = true;
        self.last_publish = Instant::now() - PUBLISH_INTERVAL;
    }

    /// Send a status or I/O error message to the specified tab.
    fn message(&self, file: usize, text: String) {
        let _ = self.updates.send(Update::Message { file, text });
    }

    /// Apply one navigation, pause or close request; return false on shutdown.
    fn request(&mut self, request: Request) -> bool {
        match request {
            Request::Quit => return false,
            Request::View(view) => self.set_view(view),
            Request::Locate {
                file,
                token,
                contig,
                position,
            } => {
                if self
                    .view
                    .as_ref()
                    .is_some_and(|view| view.file == file && token >= view.token)
                {
                    self.locate = Some(Locate {
                        file,
                        token,
                        contig,
                        position,
                    });
                    self.paused[file] = false;
                    self.dirty = true;
                    self.working = true;
                }
            }
            Request::Cancel(file) => {
                if let Some(paused) = self.paused.get_mut(file) {
                    *paused = true;
                }
                if self.view.as_ref().is_some_and(|view| view.file == file) {
                    self.locate = None;
                    self.foreground_read = false;
                    self.working = false;
                    self.dirty = true;
                    self.last_publish = Instant::now() - PUBLISH_INTERVAL;
                }
                self.message(file, "Background work paused".into());
            }
            Request::Close(file) => {
                let Some((owner, last_alias)) = self.files.close(file) else {
                    return true;
                };
                if last_alias {
                    self.slots[owner] = None;
                }
                if self.view.as_ref().is_some_and(|view| view.file == file) {
                    self.view = None;
                    self.locate = None;
                    self.foreground_read = false;
                    self.working = false;
                    self.dirty = false;
                }
            }
        }
        true
    }

    /// Run one bounded scan step, resolve a pending position search and publish due updates.
    /// Return false if the view-update receiver has disconnected.
    fn advance(&mut self) -> bool {
        self.working = false;
        let Some(view) = self.view.as_ref() else {
            return true;
        };
        let file = view.file;
        let Some(slot) = self.slots[self.files.owner(file)].as_mut() else {
            return true;
        };
        let before = slot.dataset.progress();
        let stopped = before.complete || before.error.is_some() || slot.failure.is_some();
        let needed = view
            .selected
            .saturating_add(1)
            .max(view.start.saturating_add(view.count.min(512) as u64));
        let needs_view = self.foreground_read
            && slot
                .dataset
                .total(view.contig.as_deref())
                .is_ok_and(|total| total < needed);
        if !needs_view {
            self.foreground_read = false;
        }
        let has_locate = self.locate.is_some();
        if !stopped && (!self.paused[file] || needs_view || has_locate) {
            match slot.dataset.step(64 * 1024) {
                Ok(after) => self.dirty |= before != after,
                Err(error) => {
                    slot.failure = Some(error.to_string());
                    let _ = self.updates.send(Update::Message {
                        file,
                        text: error.to_string(),
                    });
                    self.dirty = true;
                }
            }
        }
        let progress = slot.dataset.progress();
        let stopped = progress.complete || progress.error.is_some() || slot.failure.is_some();
        if let Some(job) = &self.locate {
            let result = slot.dataset.locate(&job.contig, job.position);
            let finished = match result {
                Ok(Some(selected)) => {
                    let _ = self.updates.send(Update::Located {
                        file: job.file,
                        token: job.token,
                        selected,
                    });
                    true
                }
                Ok(None) if stopped => {
                    let _ = self.updates.send(Update::LocateFailed {
                        file: job.file,
                        token: job.token,
                        text: progress
                            .error
                            .clone()
                            .or_else(|| slot.failure.clone())
                            .unwrap_or_else(|| format!("No indexed variants for {}", job.contig)),
                    });
                    true
                }
                Err(error) => {
                    let _ = self.updates.send(Update::LocateFailed {
                        file: job.file,
                        token: job.token,
                        text: error.to_string(),
                    });
                    true
                }
                _ => false,
            };
            if finished {
                self.locate = None;
            }
        }
        self.working = !stopped && (!self.paused[file] || needs_view || self.locate.is_some());
        if self.dirty && self.last_publish.elapsed() >= PUBLISH_INTERVAL {
            match snapshot(slot, view.clone(), self.paused[file]) {
                Ok(snapshot) => {
                    if self.updates.send(Update::View(Box::new(snapshot))).is_err() {
                        return false;
                    }
                }
                Err(error) => {
                    let _ = self.updates.send(Update::Message {
                        file,
                        text: error.to_string(),
                    });
                }
            }
            self.dirty = false;
            self.last_publish = Instant::now();
        }
        true
    }
}

/// Collect bounded table rows and one raw selected record for the request.
fn snapshot(slot: &mut Slot, request: ViewRequest, paused: bool) -> io::Result<Snapshot> {
    let mut progress = slot.dataset.progress();
    if let Some(error) = &slot.failure {
        progress.error = Some(error.clone());
    }
    Ok(Snapshot {
        header: slot.dataset.header(),
        rows: slot
            .dataset
            .rows(request.contig.as_deref(), request.start, request.count)?,
        selected: slot
            .dataset
            .entry(request.contig.as_deref(), request.selected)?,
        total: slot.dataset.total(request.contig.as_deref())?,
        contigs: slot.dataset.contigs()?,
        compression: slot.dataset.compression().to_owned(),
        request,
        progress,
        paused,
    })
}

impl Driver for Backend {
    type Request = Request;

    /// Poll immediately while scanning, wait for a due snapshot, or sleep while idle.
    fn timeout(&self) -> Duration {
        if self.working {
            Duration::ZERO
        } else if self.dirty {
            PUBLISH_INTERVAL.saturating_sub(self.last_publish.elapsed())
        } else {
            Duration::from_secs(60)
        }
    }

    /// Coalesce consecutive viewport requests while preserving operation ordering.
    /// Flush the latest viewport before each pause, locate, close or quit request.
    fn handle_requests(&mut self, incoming: impl Iterator<Item = Request>) -> bool {
        let mut pending = None;
        for request in incoming {
            if let Request::View(view) = request {
                pending = Some(view);
                continue;
            }
            if let Some(view) = pending.take() {
                self.set_view(view);
            }
            if !self.request(request) {
                return false;
            }
        }
        if let Some(view) = pending {
            self.set_view(view);
        }
        true
    }

    /// Advance indexing and publication once for the session task runner.
    fn tick(&mut self) -> bool {
        self.advance()
    }
}
