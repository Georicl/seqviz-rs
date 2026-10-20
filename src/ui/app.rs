//! File/tab state, operation lifetimes and worker/clipboard results.
use super::{
    layout::{self, Layout, SequenceLayout},
    panels,
};
use crate::clipboard::{self, ClipboardOutput, ClipboardService};
use crate::config::Config;
use crate::worker::{Request, Snapshot, Update, ViewRequest, Worker};
use ratatui::layout::Rect;
use std::{
    path::PathBuf,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
};

#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) enum Focus {
    Records,
    Sequence,
}
#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) enum Page {
    Help,
    File,
    Record,
    Statistics,
}
impl Page {
    pub(super) fn next(self, backwards: bool) -> Self {
        let pages = [Self::Help, Self::File, Self::Record, Self::Statistics];
        let i = pages.iter().position(|p| *p == self).unwrap();
        pages[(i + if backwards { 3 } else { 1 }) % 4]
    }
}
pub(super) enum Input {
    Search(String),
    Position(String),
}

pub(super) struct Tab {
    pub(super) path: PathBuf,
    pub(super) closed: bool,
    pub(super) record: u64,
    pub(super) start: u64,
    pub(super) cursor: u64,
    pub(super) selection: Option<(u64, u64)>,
    pub(super) list_start: u64,
    pub(super) snapshot: Option<Box<Snapshot>>,
    pub(super) message: String,
    pub(super) last_search: String,
    pub(super) pending_end: bool,
}

pub(super) struct App {
    pub(super) tabs: Vec<Tab>,
    pub(super) active: usize,
    pub(super) focus: Focus,
    pub(super) page: Option<Page>,
    pub(super) input: Option<Input>,
    pub(super) config: Config,
    pub(super) hits: Layout,
    pub(super) token: u64,
    pub(super) dirty: bool,
    pub(super) redraw: bool,
    pub(super) next_operation_id: u64,
    pub(super) pending_operation: Option<PendingOperation>,
    pub(super) clipboard: ClipboardService,
    pub(super) quit: bool,
    pub(super) drag: Option<(u16, u16)>,
    pub(super) info_scroll: usize,
    pub(super) info_lines: Vec<String>,
    pub(super) info_selection: Option<((usize, usize), (usize, usize))>,
    pub(super) worker: Worker,
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) enum OperationKind {
    Search,
    Copy,
}

pub(super) struct PendingOperation {
    pub(super) id: u64,
    pub(super) file: usize,
    pub(super) kind: OperationKind,
    pub(super) cancel: Arc<AtomicBool>,
}

impl App {
    pub(super) fn new(paths: Vec<PathBuf>, config: Config, worker: Worker) -> Self {
        Self {
            tabs: paths
                .into_iter()
                .map(|path| Tab {
                    path,
                    closed: false,
                    record: 0,
                    start: 0,
                    cursor: 0,
                    selection: None,
                    list_start: 0,
                    snapshot: None,
                    message: "Opening…".into(),
                    last_search: String::new(),
                    pending_end: false,
                })
                .collect(),
            active: 0,
            focus: Focus::Records,
            page: None,
            input: None,
            config,
            hits: Layout::default(),
            token: 0,
            dirty: true,
            redraw: true,
            next_operation_id: 0,
            pending_operation: None,
            clipboard: ClipboardService::start(),
            quit: false,
            drag: None,
            info_scroll: 0,
            info_lines: Vec::new(),
            info_selection: None,
            worker,
        }
    }
    pub(super) fn tab(&self) -> &Tab {
        &self.tabs[self.active]
    }
    pub(super) fn tab_mut(&mut self) -> &mut Tab {
        &mut self.tabs[self.active]
    }
    pub(super) fn can_copy(&self) -> bool {
        self.tab()
            .snapshot
            .as_ref()
            .is_some_and(|s| s.format.can_copy())
    }
    pub(super) fn count(&self) -> usize {
        (self.hits.bases_per_line.max(1) * self.hits.sequence_rows.max(1)).min(128 * 1024)
    }

    pub(super) fn request_view(&mut self) {
        self.token += 1;
        let tab = self.tab();
        self.worker.send(Request::View(ViewRequest {
            token: self.token,
            file: self.active,
            record: tab.record,
            start: tab.start,
            count: self.count(),
            list_start: tab.list_start,
            list_count: self.hits.list.height.max(1) as usize,
        }));
        self.dirty = false;
    }

    pub(super) fn receive(&mut self) {
        while let Ok(update) = self.worker.updates.try_recv() {
            self.redraw = true;
            match update {
                Update::View(snapshot) => {
                    let file = snapshot.request.file;
                    if self.tabs[file].closed
                        || file != self.active
                        || snapshot.request.token != self.token
                    {
                        continue;
                    }
                    if self.tabs[file].pending_end
                        && (snapshot.progress.complete || snapshot.progress.error.is_some())
                    {
                        self.tabs[file].pending_end = false;
                        if self.focus == Focus::Records {
                            self.choose_record(snapshot.progress.records.saturating_sub(1));
                        } else if let Some(meta) = &snapshot.record {
                            self.tabs[file].start = meta.length.saturating_sub(self.count() as u64);
                            self.tabs[file].cursor = meta.length.saturating_sub(1);
                            self.dirty = true;
                        }
                    }
                    if let Some(error) = &snapshot.progress.error {
                        self.tabs[file].message = error.clone();
                    } else if self.tabs[file].message == "Opening…" {
                        self.tabs[file].message.clear();
                    }
                    self.tabs[file].snapshot = Some(snapshot);
                }
                Update::Selected {
                    file,
                    record,
                    operation_id,
                } if self.matches_operation(file, operation_id, OperationKind::Search) => {
                    self.pending_operation = None;
                    self.choose_record(record);
                    self.tab_mut().message = "Match found".into();
                }
                Update::Copied {
                    file,
                    text,
                    operation_id,
                } if self.matches_operation(file, operation_id, OperationKind::Copy) => {
                    self.submit_clipboard(text);
                }
                Update::OperationFailed {
                    file,
                    operation_id,
                    text,
                } if self
                    .pending_operation
                    .as_ref()
                    .is_some_and(|op| op.id == operation_id && op.file == file) =>
                {
                    self.pending_operation = None;
                    self.tabs[file].message = text;
                }
                Update::Message { file, text } if !self.tabs[file].closed => {
                    self.tabs[file].message = text
                }
                _ => {}
            }
        }
        while let Ok(update) = self.clipboard.updates.try_recv() {
            if !self.matches_operation(update.file, update.operation_id, OperationKind::Copy) {
                continue;
            }
            self.pending_operation = None;
            self.tab_mut().message = match update.result {
                Ok(ClipboardOutput::System) => "Copied to clipboard".into(),
                Ok(ClipboardOutput::Osc52(payload)) if self.can_copy() => {
                    match clipboard::write_osc52(&payload) {
                        Ok(()) => "Clipboard request sent to terminal (OSC 52)".into(),
                        Err(error) => error.to_string(),
                    }
                }
                Ok(ClipboardOutput::Osc52(_)) => "Copy is disabled for SAM/BAM".into(),
                Err(error) => error.to_string(),
            };
            self.redraw = true;
        }
    }

    pub(super) fn choose_record(&mut self, record: u64) {
        self.cancel_operation();
        self.redraw = true;
        let height = self.hits.list.height.max(1) as u64;
        let tab = self.tab_mut();
        if let Some(snapshot) = &tab.snapshot
            && snapshot.progress.complete
            && record >= snapshot.progress.records
        {
            return;
        }
        tab.record = record;
        tab.start = 0;
        tab.cursor = 0;
        tab.selection = None;
        if record < tab.list_start {
            tab.list_start = record;
        }
        if record >= tab.list_start + height {
            tab.list_start = record + 1 - height;
        }
        self.dirty = true;
    }

    pub(super) fn switch_tab(&mut self, next: usize) {
        self.cancel_operation();
        self.redraw = true;
        self.worker.cancel_background.store(true, Ordering::Relaxed);
        if let Some(snapshot) = &mut self.tab_mut().snapshot {
            snapshot.window.sequence.clear();
            snapshot.window.quality.clear();
        }
        self.active = next;
        self.page = None;
        self.input = None;
        self.drag = None;
        self.info_selection = None;
        self.dirty = true;
    }

    pub(super) fn cycle_tab(&mut self, backwards: bool) {
        let n = self.tabs.len();
        for step in 1..=n {
            let next = if backwards {
                (self.active + n - step % n) % n
            } else {
                (self.active + step) % n
            };
            if !self.tabs[next].closed {
                self.switch_tab(next);
                break;
            }
        }
    }

    pub(super) fn open_page(&mut self, page: Page) {
        self.redraw = true;
        self.page = Some(page);
        self.info_scroll = 0;
        self.info_selection = None;
        if page == Page::Statistics {
            self.worker
                .cancel_background
                .store(false, Ordering::Relaxed);
            self.worker.send(Request::Statistics { file: self.active });
        }
    }

    pub(super) fn copy(&mut self) {
        if !self.can_copy() {
            self.tab_mut().message = "Copy is disabled for SAM/BAM".into();
            return;
        }
        let range = if self.focus == Focus::Sequence {
            let Some((a, b)) = self.tab().selection else {
                self.tab_mut().message = "Drag or press v to select sequence".into();
                return;
            };
            Some((a.min(b), a.max(b).saturating_add(1)))
        } else {
            None
        };
        self.request_view();
        let (operation_id, cancel) = self.begin_operation(OperationKind::Copy);
        self.tab_mut().message = "Reading selection for clipboard… Esc cancels".into();
        self.worker.send(Request::Copy {
            operation_id,
            cancel,
            file: self.active,
            record: self.tab().record,
            range,
        });
    }

    pub(super) fn submit_clipboard(&mut self, text: String) {
        let Some(operation) = self.pending_operation.as_ref() else {
            return;
        };
        if let Err(error) = self.clipboard.submit(
            operation.id,
            operation.file,
            text,
            self.can_copy(),
            Arc::clone(&operation.cancel),
        ) {
            self.pending_operation = None;
            self.tab_mut().message = error.to_string();
        }
    }

    pub(super) fn copy_info(&mut self) {
        if !self.can_copy() {
            self.tab_mut().message = "Copy is disabled for SAM/BAM".into();
            return;
        }
        let text = if let Some((a, b)) = self.info_selection {
            let (start, end) = if a <= b { (a, b) } else { (b, a) };
            self.info_lines
                .iter()
                .enumerate()
                .filter(|(i, _)| *i >= start.0 && *i <= end.0)
                .map(|(i, line)| {
                    line.chars()
                        .skip(if i == start.0 { start.1 } else { 0 })
                        .take(if i == end.0 {
                            end.1 + 1 - if i == start.0 { start.1 } else { 0 }
                        } else {
                            usize::MAX
                        })
                        .collect::<String>()
                })
                .collect::<Vec<_>>()
                .join("\n")
        } else {
            self.info_lines.join("\n")
        };
        self.begin_operation(OperationKind::Copy);
        self.submit_clipboard(text);
    }

    pub(super) fn prepare_frame(&mut self, area: Rect) {
        let meta = self.tab().snapshot.as_ref().and_then(|s| s.record.as_ref());
        let sequence = SequenceLayout {
            known_length: meta.map(|r| r.length),
            start: self.tab().start,
            has_quality: meta.is_some_and(|r| r.has_quality),
            show_quality: self.config.show_quality,
        };
        let open: Vec<_> = self
            .tabs
            .iter()
            .enumerate()
            .filter(|(_, tab)| !tab.closed)
            .map(|(id, _)| id)
            .collect();
        let layout = layout::calculate(area, &open, self.active, sequence);
        if self.hits.area != area
            || self.hits.bases_per_line != layout.bases_per_line
            || self.hits.sequence_rows != layout.sequence_rows
            || self.hits.list.height != layout.list.height
        {
            self.dirty = true;
            self.redraw = true;
        }
        self.hits = layout;
        if self.page.is_some() && self.redraw {
            self.info_lines = panels::text(self)
                .into_iter()
                .flat_map(|line| panels::wrap_line(&line, self.hits.info.width.max(1) as usize))
                .collect();
            self.info_scroll = self.info_scroll.min(
                self.info_lines
                    .len()
                    .saturating_sub(self.hits.info.height as usize),
            );
        }
    }

    pub(super) fn cancel_operation(&mut self) {
        if let Some(operation) = self.pending_operation.take() {
            operation.cancel.store(true, Ordering::Relaxed);
            if !self.tabs[operation.file].closed {
                self.tabs[operation.file].message = "Operation cancelled".into();
            }
        }
    }

    pub(super) fn begin_operation(&mut self, kind: OperationKind) -> (u64, Arc<AtomicBool>) {
        self.cancel_operation();
        self.next_operation_id += 1;
        let cancel = Arc::new(AtomicBool::new(false));
        self.pending_operation = Some(PendingOperation {
            id: self.next_operation_id,
            file: self.active,
            kind,
            cancel: Arc::clone(&cancel),
        });
        (self.next_operation_id, cancel)
    }

    pub(super) fn matches_operation(&self, file: usize, id: u64, kind: OperationKind) -> bool {
        file == self.active
            && !self.tabs[file].closed
            && self.pending_operation.as_ref().is_some_and(|op| {
                op.id == id
                    && op.file == file
                    && op.kind == kind
                    && !op.cancel.load(Ordering::Relaxed)
            })
    }
}

impl Drop for App {
    fn drop(&mut self) {
        self.cancel_operation();
    }
}
