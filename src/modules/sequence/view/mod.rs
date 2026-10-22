//! State and interaction controller for one sequence document page.
mod events;
mod layout;
mod panels;
mod render;
#[cfg(test)]
mod tests;

use self::layout::{Layout, SequenceLayout};
use super::config::SequenceConfig;
use super::protocol::{Request, Snapshot, Update, ViewRequest};
use crate::modules::{
    protocol::{ModuleRequest, ModuleUpdate},
    view::{Action, DocumentView, InputPrompt, Status},
};
use crate::widgets::{text_panel, theme::Theme};
use ratatui::{Frame, layout::Rect};
use std::{
    cell::RefCell,
    path::PathBuf,
    rc::Rc,
    sync::{
        Arc,
        atomic::{AtomicBool, AtomicU64, Ordering},
    },
};

#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) enum Focus {
    Records,
    Sequence,
}
#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) enum Page {
    Help,
    File,
    Record,
    Statistics,
}
impl Page {
    pub(crate) fn next(self, backwards: bool) -> Self {
        let pages = [Self::Help, Self::File, Self::Record, Self::Statistics];
        let i = pages.iter().position(|p| *p == self).unwrap();
        pages[(i + if backwards { 3 } else { 1 }) % 4]
    }
}
pub(crate) enum Input {
    Search(String),
    Position(String),
}

pub(crate) struct SequenceState {
    pub(crate) path: PathBuf,
    pub(crate) record: u64,
    pub(crate) start: u64,
    pub(crate) cursor: u64,
    pub(crate) selection: Option<(u64, u64)>,
    pub(crate) list_start: u64,
    pub(crate) snapshot: Option<Box<Snapshot>>,
    pub(crate) message: String,
    pub(crate) last_search: String,
    pub(crate) pending_end: bool,
}

pub(crate) struct SharedInteraction {
    pub(crate) focus: Focus,
    pub(crate) show_quality: bool,
}

pub(crate) struct SequenceView {
    pub(crate) document: SequenceState,
    pub(crate) file: usize,
    enabled: bool,
    shared: Rc<RefCell<SharedInteraction>>,
    pub(crate) focus: Focus,
    pub(crate) page: Option<Page>,
    pub(crate) input: Option<Input>,
    pub(crate) config: SequenceConfig,
    pub(crate) theme: Theme,
    pub(crate) hits: Layout,
    pub(crate) token: u64,
    pub(crate) dirty: bool,
    pub(crate) redraw: bool,
    operation_ids: Arc<AtomicU64>,
    pub(crate) pending_operation: Option<PendingOperation>,
    pub(crate) drag: Option<(u16, u16)>,
    pub(crate) info: text_panel::State,
    actions: Vec<Action>,
}
#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) enum OperationKind {
    Search,
    Copy,
}

pub(crate) struct PendingOperation {
    pub(crate) id: u64,
    pub(crate) file: usize,
    pub(crate) kind: OperationKind,
    pub(crate) cancel: Arc<AtomicBool>,
}

impl SequenceView {
    pub(crate) fn new(
        path: PathBuf,
        file: usize,
        config: SequenceConfig,
        theme: Theme,
        operation_ids: Arc<AtomicU64>,
        shared: Rc<RefCell<SharedInteraction>>,
    ) -> Self {
        Self {
            document: SequenceState {
                path,
                record: 0,
                start: 0,
                cursor: 0,
                selection: None,
                list_start: 0,
                snapshot: None,
                message: "Opening…".into(),
                last_search: String::new(),
                pending_end: false,
            },
            file,
            enabled: file == 0,
            shared,
            focus: Focus::Records,
            page: None,
            input: None,
            config,
            theme,
            hits: Layout::default(),
            token: 0,
            dirty: true,
            redraw: true,
            operation_ids,
            pending_operation: None,
            drag: None,
            info: text_panel::State::default(),
            actions: Vec::new(),
        }
    }
    pub(crate) fn tab(&self) -> &SequenceState {
        &self.document
    }
    pub(crate) fn tab_mut(&mut self) -> &mut SequenceState {
        &mut self.document
    }
    fn send(&mut self, request: Request) {
        self.actions
            .push(Action::Request(ModuleRequest::sequence(request)));
    }
    fn set_background_cancelled(&mut self, cancelled: bool) {
        self.actions.push(Action::BackgroundCancelled(cancelled));
    }
    pub(crate) fn can_copy(&self) -> bool {
        self.tab()
            .snapshot
            .as_ref()
            .is_some_and(|s| s.format.can_copy())
    }
    pub(crate) fn count(&self) -> usize {
        (self.hits.bases_per_line.max(1) * self.hits.sequence_rows.max(1)).min(128 * 1024)
    }
    pub(crate) fn request_view(&mut self) {
        self.token += 1;
        let tab = self.tab();
        self.send(Request::View(ViewRequest {
            token: self.token,
            file: self.file,
            record: tab.record,
            start: tab.start,
            count: self.count(),
            list_start: tab.list_start,
            list_count: self.hits.list.height.max(1) as usize,
        }));
        self.dirty = false;
    }
    pub(crate) fn choose_record(&mut self, record: u64) {
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
    pub(crate) fn open_page(&mut self, page: Page) {
        self.redraw = true;
        self.page = Some(page);
        self.info.scroll = 0;
        self.info.selection = None;
        if page == Page::Statistics {
            self.set_background_cancelled(false);
            self.send(Request::Statistics { file: self.file });
        }
    }
    pub(crate) fn copy(&mut self) {
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
        self.send(Request::Copy {
            operation_id,
            cancel,
            file: self.file,
            record: self.tab().record,
            range,
        });
    }
    pub(crate) fn copy_info(&mut self) {
        if !self.can_copy() {
            self.tab_mut().message = "Copy is disabled for SAM/BAM".into();
            return;
        }
        let text = self.info.selected_text();
        self.begin_operation(OperationKind::Copy);
        self.submit_clipboard(text);
    }
    pub(crate) fn cancel_operation(&mut self) {
        if let Some(operation) = self.pending_operation.take() {
            operation.cancel.store(true, Ordering::Relaxed);
            if self.enabled {
                self.document.message = "Operation cancelled".into();
            }
        }
    }
    pub(crate) fn matches_operation(&self, file: usize, id: u64, kind: OperationKind) -> bool {
        self.enabled
            && file == self.file
            && self.pending_operation.as_ref().is_some_and(|op| {
                op.id == id
                    && op.file == file
                    && op.kind == kind
                    && !op.cancel.load(Ordering::Relaxed)
            })
    }
    fn apply_update(&mut self, update: Update) {
        self.redraw = true;
        match update {
            Update::View(snapshot) => {
                let file = snapshot.request.file;
                if !self.enabled || file != self.file || snapshot.request.token != self.token {
                    return;
                }
                if self.document.pending_end
                    && (snapshot.progress.complete || snapshot.progress.error.is_some())
                {
                    self.document.pending_end = false;
                    if self.focus == Focus::Records {
                        self.choose_record(snapshot.progress.records.saturating_sub(1));
                    } else if let Some(meta) = &snapshot.record {
                        self.document.start = meta.length.saturating_sub(self.count() as u64);
                        self.document.cursor = meta.length.saturating_sub(1);
                        self.dirty = true;
                    }
                }
                if let Some(error) = &snapshot.progress.error {
                    self.document.message = error.clone();
                } else if self.document.message == "Opening…" {
                    self.document.message.clear();
                }
                self.document.snapshot = Some(snapshot);
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
                self.document.message = text;
            }
            Update::Message { file: _, text } => self.document.message = text,
            _ => {}
        }
    }

    pub(crate) fn submit_clipboard(&mut self, text: String) {
        if let Some(op) = self.pending_operation.as_ref() {
            self.actions.push(Action::Clipboard {
                operation_id: op.id,
                file: op.file,
                text,
                allowed: self.can_copy(),
                cancel: Arc::clone(&op.cancel),
            });
        }
    }
    pub(crate) fn begin_operation(&mut self, kind: OperationKind) -> (u64, Arc<AtomicBool>) {
        self.cancel_operation();
        let id = self.operation_ids.fetch_add(1, Ordering::Relaxed) + 1;
        let cancel = Arc::new(AtomicBool::new(false));
        self.pending_operation = Some(PendingOperation {
            id,
            file: self.file,
            kind,
            cancel: Arc::clone(&cancel),
        });
        (id, cancel)
    }
    pub(crate) fn prepare_frame(&mut self, area: Rect) {
        let meta = self
            .document
            .snapshot
            .as_ref()
            .and_then(|s| s.record.as_ref());
        let sequence = SequenceLayout {
            known_length: meta.map(|r| r.length),
            start: self.document.start,
            has_quality: meta.is_some_and(|r| r.has_quality),
            show_quality: self.config.show_quality,
        };
        let next = layout::calculate(area, sequence);
        if self.hits.area != area
            || self.hits.bases_per_line != next.bases_per_line
            || self.hits.sequence_rows != next.sequence_rows
            || self.hits.list.height != next.list.height
        {
            self.dirty = true;
            self.redraw = true;
        }
        self.hits = next;
        if self.page.is_some() && self.redraw {
            self.info.reflow(
                panels::text(self),
                self.hits.info.width.max(1),
                self.hits.info.height,
            );
        }
    }
}
impl Drop for SequenceView {
    fn drop(&mut self) {
        self.cancel_operation();
    }
}

impl DocumentView for SequenceView {
    fn prepare(&mut self, area: Rect) {
        self.prepare_frame(area);
    }
    fn draw(&self, frame: &mut Frame) {
        render::draw(frame, self);
    }
    fn key(&mut self, event: crossterm::event::KeyEvent) {
        self.key(event);
    }
    fn mouse(&mut self, event: crossterm::event::MouseEvent) {
        self.mouse(event);
    }
    fn tick(&mut self) {
        self.drag_scroll();
    }
    fn captures_input(&self) -> bool {
        self.input.is_some() || self.page.is_some()
    }
    fn captures_tab_mouse(&self) -> bool {
        self.page.is_some()
    }
    fn is_dirty(&self) -> bool {
        self.dirty
    }
    fn needs_redraw(&self) -> bool {
        self.redraw
    }
    fn did_draw(&mut self) {
        self.redraw = false;
    }
    fn request_view(&mut self) {
        self.request_view();
    }
    fn update(&mut self, update: ModuleUpdate) {
        let ModuleUpdate::Sequence(update) = update;
        self.apply_update(update.payload);
    }
    fn actions(&mut self) -> Vec<Action> {
        std::mem::take(&mut self.actions)
    }
    fn activate(&mut self) {
        let shared = self.shared.borrow();
        self.focus = shared.focus;
        self.config.show_quality = shared.show_quality;
        self.enabled = true;
        self.dirty = true;
        self.redraw = true;
    }
    fn deactivate(&mut self) {
        {
            let mut shared = self.shared.borrow_mut();
            shared.focus = self.focus;
            shared.show_quality = self.config.show_quality;
        }
        self.cancel_operation();
        self.set_background_cancelled(true);
        self.enabled = false;
        if let Some(snapshot) = self.document.snapshot.as_mut() {
            snapshot.window.sequence.clear();
            snapshot.window.quality.clear();
        }
        self.page = None;
        self.input = None;
        self.drag = None;
        self.info.selection = None;
        self.dirty = true;
        self.redraw = true;
    }
    fn status(&self) -> Status {
        render::status(self)
    }
    fn input_prompt(&self) -> Option<InputPrompt<'_>> {
        self.input.as_ref().map(|input| match input {
            Input::Search(text) => InputPrompt {
                label: "Name contains: ",
                text,
            },
            Input::Position(text) => InputPrompt {
                label: "Position (1-based): ",
                text,
            },
        })
    }
    fn can_copy(&self) -> bool {
        self.can_copy()
    }
    fn accepts_copy(&self, id: u64) -> bool {
        self.matches_operation(self.file, id, OperationKind::Copy)
    }
    fn copy_finished(&mut self, id: u64, message: String) {
        if self.accepts_copy(id) {
            self.pending_operation = None;
            self.document.message = message;
            self.redraw = true;
        }
    }
    fn resized(&mut self) {
        self.dirty = true;
        self.redraw = true;
        self.info.selection = None;
    }
}
