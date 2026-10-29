//! State and interaction controller for one sequence document page.
/// Keyboard, pointer and edge-scroll interaction for sequence documents.
mod events;
/// Terminal pane geometry and zero-based sequence hit testing.
mod layout;
/// Selectable help, metadata and statistics page content.
mod panels;
/// Sequence-pane rendering and status-line construction.
mod render;
/// Sequence interaction, rendering and operation-lifetime regressions.
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

/// Pane that receives sequence-navigation keyboard commands.
#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) enum Focus {
    /// Navigate the list of indexed records.
    Records,
    /// Navigate sequence characters and their selection.
    Sequence,
}
/// Information-page overlays available within a sequence document.
#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) enum Page {
    /// Keyboard controls and interpretation of displayed sequence metrics.
    Help,
    /// Source path, format, compression and index progress.
    File,
    /// Current record header, parsed properties and sequence metrics.
    Record,
    /// Dataset-wide metrics requested from the backend.
    Statistics,
}
impl Page {
    /// Cycle through information pages in either direction, wrapping at the ends.
    pub(crate) fn next(self, backwards: bool) -> Self {
        let pages = [Self::Help, Self::File, Self::Record, Self::Statistics];
        let i = pages.iter().position(|p| *p == self).unwrap();
        pages[(i + if backwards { 3 } else { 1 }) % 4]
    }
}
/// Editable search text or one-based position entered in the bottom prompt.
pub(crate) enum Input {
    /// Literal record-name query being edited.
    Search(String),
    /// One-based sequence position being entered as text.
    Position(String),
}

/// Navigation and cached data retained independently for each document tab.
pub(crate) struct SequenceState {
    /// Original source path shown in the file-information page.
    pub(crate) path: PathBuf,
    /// Zero-based selected record ID.
    pub(crate) record: u64,
    /// Zero-based sequence offset at the top of the target viewport.
    pub(crate) start: u64,
    /// Zero-based sequence coordinate of the keyboard cursor.
    pub(crate) cursor: u64,
    /// Inclusive zero-based sequence endpoints, retaining drag direction.
    pub(crate) selection: Option<(u64, u64)>,
    /// Zero-based record ID at the top of the record list.
    pub(crate) list_start: u64,
    /// Most recent accepted viewport snapshot for this document.
    pub(crate) snapshot: Option<Box<Snapshot>>,
    /// Current operation, error or status message shown in the status bar.
    pub(crate) message: String,
    /// Last nonempty name query, reused by the next-match command.
    pub(crate) last_search: String,
    /// Whether an End command is waiting for indexing to establish the final position.
    pub(crate) pending_end: bool,
}

/// Focus and quality-row preferences carried between sequence tabs.
pub(crate) struct SharedInteraction {
    /// Record or sequence pane selected for navigation.
    pub(crate) focus: Focus,
    /// Whether aligned quality rows should remain visible across tab switches.
    pub(crate) show_quality: bool,
}

/// Sequence document controller that queues shell actions and consumes backend snapshots.
pub(crate) struct SequenceView {
    /// Per-tab navigation state and its latest accepted data snapshot.
    pub(crate) document: SequenceState,
    /// Stable tab index used to route requests and replies.
    pub(crate) file: usize,
    /// Whether this view is active and may accept operation results.
    enabled: bool,
    /// Cross-tab focus and quality-row preferences handed off on activation.
    shared: Rc<RefCell<SharedInteraction>>,
    /// Record or sequence pane selected for navigation.
    pub(crate) focus: Focus,
    /// Currently open information overlay, if any.
    pub(crate) page: Option<Page>,
    /// Prompt currently capturing typed input, if any.
    pub(crate) input: Option<Input>,
    /// Sequence display settings, including quality visibility and residue colours.
    pub(crate) config: SequenceConfig,
    /// Shared application colours for focus, borders and selection.
    pub(crate) theme: Theme,
    /// Prepared rendering geometry, also used for pointer hit testing.
    pub(crate) hits: Layout,
    /// Viewport generation used to reject snapshots superseded by navigation or resizing.
    pub(crate) token: u64,
    /// Whether the current viewport needs a new backend request.
    pub(crate) dirty: bool,
    /// Whether visible state needs another terminal frame.
    pub(crate) redraw: bool,
    /// Application-wide ID source shared by operations across document tabs.
    operation_ids: Arc<AtomicU64>,
    /// Active search/copy identity, independent of the viewport token.
    pub(crate) pending_operation: Option<PendingOperation>,
    /// Last pointer position while sequence selection is being dragged.
    pub(crate) drag: Option<(u16, u16)>,
    /// Reflowed information-page text with independent scrolling and selection.
    pub(crate) info: text_panel::State,
    /// Requests and clipboard work queued for the shell to drain.
    actions: Vec<Action>,
}
/// User operation whose result must match the pending operation identity.
#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) enum OperationKind {
    /// Record-name search that selects its resulting record.
    Search,
    /// Sequence or information-text copy delivered through the clipboard service.
    Copy,
}

/// One active user operation with a cancellation flag distinct from viewport generations.
pub(crate) struct PendingOperation {
    /// Globally allocated identity echoed by backend and clipboard results.
    pub(crate) id: u64,
    /// Stable tab index used to route requests and replies.
    pub(crate) file: usize,
    /// Expected operation result type.
    pub(crate) kind: OperationKind,
    /// Operation-owned flag that remains set once the operation is cancelled.
    pub(crate) cancel: Arc<AtomicBool>,
}

impl SequenceView {
    /// Create one document view with shared operation IDs and cross-tab interaction preferences.
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
    /// Borrow the per-document navigation state and latest snapshot.
    pub(crate) fn tab(&self) -> &SequenceState {
        &self.document
    }
    /// Mutably borrow the per-document navigation state and latest snapshot.
    pub(crate) fn tab_mut(&mut self) -> &mut SequenceState {
        &mut self.document
    }
    /// Queue a sequence request for the application shell to forward to the worker.
    fn send(&mut self, request: Request) {
        self.actions
            .push(Action::Request(ModuleRequest::sequence(request)));
    }
    /// Queue an update to the shared background-cancellation flag.
    fn set_background_cancelled(&mut self, cancelled: bool) {
        self.actions.push(Action::BackgroundCancelled(cancelled));
    }
    /// Allow copying only after a snapshot identifies a supported non-alignment format.
    pub(crate) fn can_copy(&self) -> bool {
        self.tab()
            .snapshot
            .as_ref()
            .is_some_and(|s| s.format.can_copy())
    }
    /// Return the viewport capacity in sequence characters, capped at 128 KiB.
    pub(crate) fn count(&self) -> usize {
        (self.hits.bases_per_line.max(1) * self.hits.sequence_rows.max(1)).min(128 * 1024)
    }
    /// Advance the viewport generation and queue the current record/list window request.
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
    /// Cancel the active operation, select a record and reset sequence navigation while keeping it visible.
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
    /// Open an information page, reset its scroll/selection and request statistics when needed.
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
    /// Queue a full-record copy or convert inclusive sequence selection endpoints to a half-open range.
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
    /// Start a copy operation for selected information text, or the full page when unselected.
    pub(crate) fn copy_info(&mut self) {
        if !self.can_copy() {
            self.tab_mut().message = "Copy is disabled for SAM/BAM".into();
            return;
        }
        let text = self.info.selected_text();
        self.begin_operation(OperationKind::Copy);
        self.submit_clipboard(text);
    }
    /// Cancel and remove the current operation without changing any newer operation flag.
    pub(crate) fn cancel_operation(&mut self) {
        if let Some(operation) = self.pending_operation.take() {
            operation.cancel.store(true, Ordering::Relaxed);
            if self.enabled {
                self.document.message = "Operation cancelled".into();
            }
        }
    }
    /// Accept only the enabled view's matching file, ID, kind and uncancelled operation.
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
    /// Apply matching viewport generations and operation results, completing deferred end navigation.
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

    /// Queue clipboard delivery with the pending operation's identity and cancellation flag.
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
    /// Cancel the previous operation and allocate a new global ID with its own cancellation flag.
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
    /// Recalculate geometry, mark capacity changes for refresh and reflow visible information text.
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
    /// Cancel any operation that still belongs to the document being dropped.
    fn drop(&mut self) {
        self.cancel_operation();
    }
}

impl DocumentView for SequenceView {
    /// Prepare sequence geometry for the document body assigned by the application.
    fn prepare(&mut self, area: Rect) {
        self.prepare_frame(area);
    }
    /// Render the sequence document from its prepared geometry and cached snapshot.
    fn draw(&self, frame: &mut Frame) {
        render::draw(frame, self);
    }
    /// Forward a document key event to sequence-specific interaction handling.
    fn key(&mut self, event: crossterm::event::KeyEvent) {
        self.key(event);
    }
    /// Forward a document pointer event to sequence-specific interaction handling.
    fn mouse(&mut self, event: crossterm::event::MouseEvent) {
        self.mouse(event);
    }
    /// Continue edge-drag scrolling without requiring another pointer event.
    fn tick(&mut self) {
        self.drag_scroll();
    }
    /// Keep keyboard navigation inside an active prompt or information page.
    fn captures_input(&self) -> bool {
        self.input.is_some() || self.page.is_some()
    }
    /// Keep tab-area mouse events inside an open information-page interaction.
    fn captures_tab_mouse(&self) -> bool {
        self.page.is_some()
    }
    /// Report whether a fresh viewport request is needed.
    fn is_dirty(&self) -> bool {
        self.dirty
    }
    /// Report whether the visible state has changed since the last frame.
    fn needs_redraw(&self) -> bool {
        self.redraw
    }
    /// Clear the redraw flag after the application renders a frame.
    fn did_draw(&mut self) {
        self.redraw = false;
    }
    /// Queue a fresh sequence viewport through the document-view interface.
    fn request_view(&mut self) {
        self.request_view();
    }
    /// Accept sequence-domain updates and discard updates for other document kinds.
    fn update(&mut self, update: ModuleUpdate) {
        if let ModuleUpdate::Sequence(update) = update {
            self.apply_update(update.payload);
        }
    }
    /// Drain requests and clipboard actions queued for the application shell.
    fn actions(&mut self) -> Vec<Action> {
        std::mem::take(&mut self.actions)
    }
    /// Restore shared focus/quality preferences and refresh the newly active document.
    fn activate(&mut self) {
        let shared = self.shared.borrow();
        self.focus = shared.focus;
        self.config.show_quality = shared.show_quality;
        self.enabled = true;
        self.dirty = true;
        self.redraw = true;
    }
    /// Save shared preferences, cancel work and release window bytes while retaining document navigation.
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
    /// Build the document status line and keyboard hints.
    fn status(&self) -> Status {
        render::status(self)
    }
    /// Expose the active name-search or one-based position prompt to the application shell.
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
    /// Expose the sequence format's copy permission to the application shell.
    fn can_copy(&self) -> bool {
        self.can_copy()
    }
    /// Check whether a clipboard result still belongs to the active copy operation.
    fn accepts_copy(&self, id: u64) -> bool {
        self.matches_operation(self.file, id, OperationKind::Copy)
    }
    /// Clear a matching copy operation and display its clipboard completion message.
    fn copy_finished(&mut self, id: u64, message: String) {
        if self.accepts_copy(id) {
            self.pending_operation = None;
            self.document.message = message;
            self.redraw = true;
        }
    }
    /// Refresh layout/data and clear information-page selection, preserving the sequence selection.
    fn resized(&mut self) {
        self.dirty = true;
        self.redraw = true;
        self.info.selection = None;
    }
}
