//! One VCF document: variant navigation and lazily materialized detail pages.
/// Translate keyboard and pointer input into VCF navigation and selection changes.
mod events;
/// Allocate terminal rectangles for the table and detail panels.
mod layout;
/// Draw bounded table cells, original-text details and overlays.
mod render;
/// Check terminal-cell output across narrow layouts and Unicode content.
#[cfg(test)]
mod render_tests;
/// Exercise document navigation, clipboard actions and asynchronous update handling.
#[cfg(test)]
mod tests;
/// Share tab expansion and grapheme geometry between rendering and selection.
mod text;

use super::{
    config::VcfConfig,
    model::VcfEntry,
    protocol::{Request, Snapshot, Update, ViewRequest},
};
use crate::modules::{
    protocol::{ModuleRequest, ModuleUpdate},
    view::{Action, DocumentView, InputPrompt, Status},
};
use crate::widgets::{text_panel, theme::Theme};
use ratatui::{Frame, layout::Rect};
use std::{
    path::PathBuf,
    sync::{
        Arc,
        atomic::{AtomicBool, AtomicU64, Ordering},
    },
};

/// The selected representation of the current variant record.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Detail {
    /// INFO keys and values from the selected record.
    Info,
    /// Sample values in FORMAT order.
    Samples,
    /// The preserved original record text.
    Raw,
}
impl Detail {
    /// Return the zero-based detail-tab position.
    fn index(self) -> usize {
        match self {
            Self::Info => 0,
            Self::Samples => 1,
            Self::Raw => 2,
        }
    }
    /// Map a cyclic tab index to INFO, Samples or Raw.
    fn at(index: usize) -> Self {
        [Self::Info, Self::Samples, Self::Raw][index % 3]
    }
}
/// The panel receiving record or detail-row navigation commands.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Focus {
    /// Navigate variant record ordinals.
    Table,
    /// Navigate logical rows within the active detail tab.
    Detail,
}
/// Full-document informational panels layered over the variant browser.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Overlay {
    /// Keyboard/mouse usage and semantic colour explanations.
    Help,
    /// Source path, compression and index progress.
    File,
    /// Original header metadata displayed as reflowed text.
    Header,
}
/// Editable text for a position jump or contig search.
enum Input {
    /// Editable positive VCF position to locate.
    Position(String),
    /// Search text used to filter the contig picker.
    Contig(String),
}
/// A clipboard operation identity and its shared cancellation flag.
struct CopyOperation {
    /// Application-wide clipboard operation ID.
    id: u64,
    /// Flag shared with the clipboard service to stop obsolete work.
    cancel: Arc<AtomicBool>,
}

/// Per-tab VCF navigation, selection and rendering state.
/// Request tokens reject stale snapshots while the backend owns shared source data.
pub(crate) struct VcfView {
    /// Original path displayed in file information.
    path: PathBuf,
    /// Tab index attached to all requests and clipboard operations.
    file: usize,
    /// VCF layout and semantic colour settings.
    config: VcfConfig,
    /// Application-wide UI colours.
    theme: Theme,
    /// Shared monotonic ID source for clipboard operations.
    clock: Arc<AtomicU64>,
    /// Whether this tab is active and may accept request-bound updates.
    enabled: bool,
    /// Most recently accepted backend viewport and selected record.
    snapshot: Option<Box<Snapshot>>,
    /// Initial backend error retained separately from transient status messages.
    load_error: Option<String>,
    /// Current contig filter; None displays all source records.
    contig: Option<String>,
    /// Zero-based selected record ordinal within the active filter.
    selected: u64,
    /// Zero-based first visible table ordinal within the active filter.
    start: u64,
    /// Current viewport or position-lookup generation.
    token: u64,
    /// Token of an outstanding position lookup that suppresses normal viewport requests.
    locating: Option<u64>,
    /// Panel receiving row navigation.
    focus: Focus,
    /// Current INFO, Samples or Raw detail tab.
    detail: Detail,
    /// Zero-based selected logical detail row.
    detail_row: usize,
    /// Zero-based first visible logical detail row.
    detail_start: usize,
    /// Horizontal scroll offset measured in terminal display columns.
    horizontal: usize,
    /// Inclusive selection endpoints as logical row and original character indices.
    selection: Option<((usize, usize), (usize, usize))>,
    /// Whether a left-button text-selection drag is active.
    dragging: bool,
    /// Initial logical text position of the current drag.
    drag_anchor: Option<(usize, usize)>,
    /// Active help, file-information or header panel.
    overlay: Option<Overlay>,
    /// Current position-jump or contig-search editor.
    input: Option<Input>,
    /// Selected index in the filtered contig choices.
    picker_row: usize,
    /// Reflowed overlay text, scroll position and selection.
    info: text_panel::State,
    /// Prepared rectangles shared by rendering and event handling.
    layout: layout::Layout,
    /// Whether the backend viewport must be refreshed.
    dirty: bool,
    /// Whether the application should draw another frame.
    redraw: bool,
    /// Transient status or current error text.
    message: String,
    /// Current clipboard job whose completion may be accepted.
    pending_copy: Option<CopyOperation>,
    /// Requests and application-level actions waiting to be drained.
    actions: Vec<Action>,
}

impl VcfView {
    /// Create a dirty document view, enabling the first tab for initial loading.
    pub(crate) fn new(
        path: PathBuf,
        file: usize,
        config: VcfConfig,
        theme: Theme,
        clock: Arc<AtomicU64>,
    ) -> Self {
        Self {
            path,
            file,
            config,
            theme,
            clock,
            enabled: file == 0,
            snapshot: None,
            load_error: None,
            contig: None,
            selected: 0,
            start: 0,
            token: 0,
            locating: None,
            focus: Focus::Table,
            detail: Detail::Info,
            detail_row: 0,
            detail_start: 0,
            horizontal: 0,
            selection: None,
            dragging: false,
            drag_anchor: None,
            overlay: None,
            input: None,
            picker_row: 0,
            info: text_panel::State::default(),
            layout: layout::Layout::default(),
            dirty: true,
            redraw: true,
            message: "Opening…".into(),
            pending_copy: None,
            actions: Vec::new(),
        }
    }
    /// Queue a VCF request for the application to forward to its session worker.
    fn send(&mut self, request: Request) {
        self.actions
            .push(Action::Request(ModuleRequest::vcf(request)));
    }
    /// Borrow the selected record only when its snapshot matches the current filter and ordinal.
    fn entry(&self) -> Option<&VcfEntry> {
        let snapshot = self.snapshot.as_ref()?;
        if snapshot.request.selected != self.selected || snapshot.request.contig != self.contig {
            return None;
        }
        snapshot.selected.as_deref()
    }
    /// Cancel the pending clipboard operation and discard its completion identity.
    fn cancel_copy(&mut self) {
        if let Some(copy) = self.pending_copy.take() {
            copy.cancel.store(true, Ordering::Relaxed);
        }
    }
    /// Queue nonempty text with a fresh clipboard operation ID, cancelling the previous copy.
    fn copy_text(&mut self, text: String) {
        self.cancel_copy();
        if text.is_empty() {
            self.message = "Nothing to copy".into();
            self.redraw = true;
            return;
        }
        let id = self.clock.fetch_add(1, Ordering::Relaxed) + 1;
        let cancel = Arc::new(AtomicBool::new(false));
        self.pending_copy = Some(CopyOperation {
            id,
            cancel: Arc::clone(&cancel),
        });
        self.actions.push(Action::Clipboard {
            operation_id: id,
            file: self.file,
            text,
            allowed: true,
            cancel,
        });
        self.message = "Copying…".into();
        self.redraw = true;
    }
    /// Select a filtered ordinal, keep it visible and invalidate record-specific detail state.
    fn change_record(&mut self, selected: u64) {
        self.locating = None;
        let selected = self
            .snapshot
            .as_ref()
            .filter(|s| s.progress.complete && s.request.contig == self.contig)
            .map_or(selected, |s| selected.min(s.total.saturating_sub(1)));
        if selected != self.selected {
            self.cancel_copy();
            self.selected = selected;
            self.detail_row = 0;
            self.detail_start = 0;
            self.horizontal = 0;
            self.selection = None;
            self.dragging = false;
            self.drag_anchor = None;
        }
        let count = self.layout.rows.height.max(1) as u64;
        if self.selected < self.start {
            self.start = self.selected;
        }
        if self.selected >= self.start.saturating_add(count) {
            self.start = self.selected.saturating_add(1).saturating_sub(count);
        }
        self.dirty = true;
        self.redraw = true;
    }
    /// Switch detail tabs and clear selection, scrolling and pending clipboard state.
    fn change_detail(&mut self, detail: Detail) {
        self.cancel_copy();
        self.detail = detail;
        self.detail_row = 0;
        self.detail_start = 0;
        self.horizontal = 0;
        self.selection = None;
        self.dragging = false;
        self.drag_anchor = None;
        self.redraw = true;
    }
    /// Count rows in the active INFO, sample or raw-record detail page.
    fn detail_len(&self) -> usize {
        let Some(entry) = self.entry() else {
            return 0;
        };
        match self.detail {
            Detail::Info => entry.record.info_fields().len(),
            Detail::Samples => self.snapshot.as_ref().map_or(0, |s| s.header.samples.len()),
            Detail::Raw => 1,
        }
    }
    /// Untruncated logical rows; TABs remain in these strings for copying.
    fn detail_rows(&self, start: usize, count: usize) -> Vec<String> {
        let Some(entry) = self.entry() else {
            return Vec::new();
        };
        match self.detail {
            Detail::Info => entry
                .record
                .info_fields()
                .into_iter()
                .skip(start)
                .take(count)
                .map(|(key, value)| {
                    if value.is_empty() {
                        key
                    } else {
                        format!("{key}\t{value}")
                    }
                })
                .collect(),
            Detail::Samples => {
                let samples = &self.snapshot.as_ref().unwrap().header.samples;
                entry
                    .record
                    .sample_values(start, count)
                    .into_iter()
                    .map(|sample| {
                        let name = samples.get(sample.index).map(String::as_str).unwrap_or(".");
                        format!("{name}\t{}", sample.values.join("\t"))
                    })
                    .collect()
            }
            Detail::Raw if start == 0 && count > 0 => vec![entry.record.raw.clone()],
            Detail::Raw => Vec::new(),
        }
    }
    /// Copy the active record, detail row, selected text or overlay content.
    /// Include the original VCF header when requested from the table or raw-record panel.
    fn copy(&mut self, with_header: bool) {
        if let Some(overlay) = self.overlay {
            let text = if overlay == Overlay::Header && self.info.selection.is_none() {
                self.snapshot
                    .as_ref()
                    .map(|s| s.header.raw.clone())
                    .unwrap_or_default()
            } else {
                self.info.selected_text()
            };
            self.copy_text(text);
            return;
        }
        let Some(entry) = self.entry() else {
            self.message = if self.snapshot.as_ref().is_some_and(|snapshot| {
                snapshot.request.contig == self.contig
                    && snapshot.progress.complete
                    && snapshot.total == 0
            }) {
                "No record selected".into()
            } else {
                "Reading selected record…".into()
            };
            return;
        };
        let text = if with_header && (self.focus == Focus::Table || self.detail == Detail::Raw) {
            format!(
                "{}{}\n",
                self.snapshot.as_ref().unwrap().header.raw,
                entry.record.raw
            )
        } else if self.focus == Focus::Table
            || (self.detail == Detail::Raw && self.selection.is_none())
        {
            entry.record.raw.clone()
        } else if let Some((a, b)) = self.selection {
            let (start, end) = if a <= b { (a, b) } else { (b, a) };
            self.detail_rows(start.0, end.0.saturating_sub(start.0) + 1)
                .into_iter()
                .enumerate()
                .map(|(offset, line)| {
                    let row = start.0 + offset;
                    let first = if row == start.0 { start.1 } else { 0 };
                    let last = (row == end.0).then_some(end.1);
                    let mut column = 0;
                    let mut copied = String::new();
                    text::walk_graphemes(&line, |symbol| {
                        let begin = column;
                        column += symbol.chars().count();
                        if column > first && last.is_none_or(|last| begin <= last) {
                            copied.push_str(symbol);
                        }
                        last.is_none_or(|last| column <= last)
                    });
                    copied
                })
                .collect::<Vec<_>>()
                .join("\n")
        } else {
            match self.detail {
                Detail::Info => entry
                    .record
                    .info_fields()
                    .get(self.detail_row)
                    .map(|(key, value)| {
                        if value.is_empty() {
                            key.clone()
                        } else {
                            format!("{key}={value}")
                        }
                    })
                    .unwrap_or_default(),
                Detail::Samples => {
                    let header = format!("SAMPLE\t{}", entry.record.format.join("\t"));
                    self.detail_rows(self.detail_row, 1)
                        .first()
                        .map(|line| format!("{header}\n{line}"))
                        .unwrap_or_default()
                }
                Detail::Raw => entry.record.raw.clone(),
            }
        };
        self.copy_text(text);
    }
    /// Filter contig choices, ranking exact matches first and retaining All records.
    fn picker_options(&self) -> Vec<(Option<String>, u64)> {
        let Some(snapshot) = &self.snapshot else {
            return vec![(None, 0)];
        };
        let query = match &self.input {
            Some(Input::Contig(query)) => query.as_str(),
            _ => "",
        };
        let lower = query.to_lowercase();
        let mut options: Vec<_> = snapshot
            .contigs
            .iter()
            .filter(|c| c.name.to_lowercase().contains(&lower))
            .map(|c| (Some(c.name.clone()), c.records))
            .collect();
        for name in &snapshot.header.contigs {
            if name.to_lowercase().contains(&lower)
                && !options
                    .iter()
                    .any(|(candidate, _)| candidate.as_ref() == Some(name))
            {
                options.push((Some(name.clone()), 0));
            }
        }
        if !query.is_empty() {
            options.sort_by_key(|(name, _)| match name {
                Some(name) if name == query => 0,
                Some(name) if name.eq_ignore_ascii_case(query) => 1,
                _ => 2,
            });
            options.push((None, snapshot.progress.records));
        } else {
            options.insert(0, (None, snapshot.progress.records));
        }
        options
    }
    /// Return the centered contig popup, list rectangle and first visible option.
    fn picker_layout(&self) -> (Rect, Rect, usize) {
        let area = self.layout.area;
        let width = area.width.min(64);
        let height = area.height.min(16);
        let popup = Rect::new(
            area.x + area.width.saturating_sub(width) / 2,
            area.y + area.height.saturating_sub(height) / 2,
            width,
            height,
        );
        let inner = layout::inset(popup);
        let list = Rect::new(
            inner.x,
            inner.y.saturating_add(inner.height.min(1)),
            inner.width,
            inner.height.saturating_sub(1),
        );
        let start = self
            .picker_row
            .saturating_add(1)
            .saturating_sub(list.height.max(1) as usize);
        (popup, list, start)
    }
    /// Open an informational panel with fresh text-selection and scroll state.
    fn open_overlay(&mut self, overlay: Overlay) {
        self.cancel_copy();
        self.dragging = false;
        self.drag_anchor = None;
        self.overlay = Some(overlay);
        self.info = text_panel::State::default();
        self.redraw = true;
        self.refresh_overlay();
    }
    /// Rebuild and reflow help, file information or original-header display lines.
    fn refresh_overlay(&mut self) {
        let Some(overlay) = self.overlay else {
            return;
        };
        let lines = match overlay {
            Overlay::Help => vec![
                "VCF BROWSER".into(),
                "↑/↓ or j/k  Select variant / detail row".into(),
                "Enter      Focus details; Esc returns to variants".into(),
                "1 / 2 / 3  INFO / Samples / Raw".into(),
                "[ / ]      Previous / next detail page".into(),
                "h / l      Scroll detail text horizontally".into(),
                "c          Search and select contig; includes All records".into(),
                ":          Jump to a 1-based position in current contig".into(),
                "y          Copy raw row / current detail / selected text".into(),
                "Y          Copy original VCF header + selected raw record".into(),
                "m / F3     Original header and metadata".into(),
                "F2         File information".into(),
                "Tab        Switch file; q quits outside this panel".into(),
                "Mouse      Select variant, detail tab, or detail text".into(),
                "".into(),
                "GT colours identify allele numbers; 0 is reference.".into(),
                "AD bar segments keep the same allele colours; zero depths add no segment.".into(),
                "QUAL and GQ are displayed as ordinary values.".into(),
            ],
            Overlay::File => {
                let mut lines = vec![format!("Path: {}", self.path.display())];
                if let Some(s) = &self.snapshot {
                    lines.extend([
                        format!("Format: {}", s.header.version),
                        format!("Compression: {}", s.compression),
                        format!("Samples: {}", s.header.samples.len()),
                        format!("Discovered records: {}", s.progress.records),
                        format!("Scanned bytes: {}", s.progress.scanned_bytes),
                        format!("Index complete: {}", s.progress.complete),
                        format!("Errors: {}", s.progress.errors),
                    ]);
                }
                lines
            }
            Overlay::Header => self
                .snapshot
                .as_ref()
                .map(|s| {
                    s.header
                        .raw
                        .lines()
                        .map(|line| {
                            let mut column = 0;
                            line.chars()
                                .map(|ch| {
                                    let width = text::character_width(ch, column, &[]);
                                    column += width;
                                    if ch == '\t' {
                                        " ".repeat(width)
                                    } else {
                                        ch.to_string()
                                    }
                                })
                                .collect()
                        })
                        .collect()
                })
                .unwrap_or_else(|| vec!["Reading header…".into()]),
        };
        let area = layout::inset(self.layout.area);
        self.info.reflow(lines, area.width, area.height);
    }
    /// Queue a new viewport generation unless a position lookup is still pending.
    fn request(&mut self) {
        if self.locating.is_some() {
            self.dirty = false;
            return;
        }
        self.token += 1;
        self.actions.push(Action::BackgroundCancelled(false));
        self.send(Request::View(ViewRequest {
            file: self.file,
            token: self.token,
            contig: self.contig.clone(),
            selected: self.selected,
            start: self.start,
            count: self.layout.rows.height.max(1) as usize,
        }));
        self.dirty = false;
    }
}

impl DocumentView for VcfView {
    /// Recompute layout and keep selected table/detail rows visible after a resize.
    fn prepare(&mut self, area: Rect) {
        let next = layout::calculate(area, self.config.detail_rows);
        let layout_changed =
            self.layout.area != area || self.layout.rows.height != next.rows.height;
        if layout_changed {
            self.dirty = true;
            self.redraw = true;
            self.selection = None;
            self.dragging = false;
            self.drag_anchor = None;
        }
        self.layout = next;
        let count = self.layout.rows.height.max(1) as u64;
        if self.selected >= self.start.saturating_add(count) {
            self.start = self.selected.saturating_add(1).saturating_sub(count);
            self.dirty = true;
        }
        let detail_height = self.layout.detail_body.height as usize;
        if detail_height > 0 && layout_changed {
            self.detail_row = self.detail_row.min(self.detail_len().saturating_sub(1));
            if self.detail_row < self.detail_start {
                self.detail_start = self.detail_row;
            } else if self.detail_row >= self.detail_start.saturating_add(detail_height) {
                self.detail_start = self
                    .detail_row
                    .saturating_add(1)
                    .saturating_sub(detail_height);
            }
        } else if detail_height == 0 && self.focus == Focus::Detail {
            self.focus = Focus::Table;
        }
        if self.overlay.is_some() && self.redraw {
            self.refresh_overlay();
        }
    }
    /// Render this document using its prepared layout.
    fn draw(&self, frame: &mut Frame) {
        render::draw(frame, self);
    }
    /// Dispatch a keyboard event to VCF navigation and input handling.
    fn key(&mut self, event: crossterm::event::KeyEvent) {
        self.key_event(event);
    }
    /// Dispatch a pointer event to panel navigation and text selection.
    fn mouse(&mut self, event: crossterm::event::MouseEvent) {
        self.mouse_event(event);
    }
    /// Leave timer-driven view state unchanged; this view has no periodic UI work.
    fn tick(&mut self) {}
    /// Keep application shortcuts inside an active input prompt or overlay.
    fn captures_input(&self) -> bool {
        self.input.is_some() || self.overlay.is_some()
    }
    /// Keep tab-strip clicks captured while a modal prompt or overlay is open.
    fn captures_tab_mouse(&self) -> bool {
        self.input.is_some() || self.overlay.is_some()
    }
    /// Report whether the current navigation or layout needs a backend viewport request.
    fn is_dirty(&self) -> bool {
        self.dirty
    }
    /// Report whether visual state has changed since the last completed frame.
    fn needs_redraw(&self) -> bool {
        self.redraw
    }
    /// Clear the redraw flag after the application renders this view.
    fn did_draw(&mut self) {
        self.redraw = false;
    }
    /// Submit the current viewport through the document action queue.
    fn request_view(&mut self) {
        self.request();
    }
    /// Apply matching VCF snapshots and lookup results, ignoring stale request generations.
    fn update(&mut self, update: ModuleUpdate) {
        let ModuleUpdate::Vcf(update) = update else {
            return;
        };
        match update.payload {
            Update::View(snapshot)
                if self.enabled
                    && snapshot.request.file == self.file
                    && snapshot.request.token == self.token =>
            {
                if snapshot.total > 0
                    && self.selected >= snapshot.total
                    && snapshot.progress.complete
                {
                    self.selected = snapshot.total - 1;
                    self.start = self
                        .selected
                        .saturating_sub(self.layout.rows.height.saturating_sub(1) as u64);
                    self.dirty = true;
                }
                if self
                    .load_error
                    .as_ref()
                    .is_some_and(|error| *error == self.message)
                {
                    self.message.clear();
                }
                if let Some(error) = &snapshot.progress.error {
                    self.message = error.clone();
                } else if self.message == "Opening…" {
                    self.message.clear();
                }
                self.snapshot = Some(snapshot);
                self.load_error = None;
                self.redraw = true;
            }
            Update::Located {
                file,
                token,
                selected,
            } if self.enabled
                && file == self.file
                && self.locating == Some(token)
                && token == self.token =>
            {
                self.locating = None;
                self.change_record(selected);
                self.message = "Position found".into();
            }
            Update::LocateFailed { file, token, text }
                if self.enabled
                    && file == self.file
                    && self.locating == Some(token)
                    && token == self.token =>
            {
                self.locating = None;
                self.selected = 0;
                self.start = 0;
                self.detail_row = 0;
                self.detail_start = 0;
                self.selection = None;
                self.dirty = true;
                self.redraw = true;
                self.message = text;
            }
            Update::Message { file, text } if file == self.file => {
                if self.snapshot.is_none() && text != "Background work paused" {
                    self.load_error = Some(text.clone());
                }
                self.message = text;
                self.redraw = true;
            }
            _ => {}
        }
    }
    /// Drain queued requests, clipboard operations and background-cancellation changes.
    fn actions(&mut self) -> Vec<Action> {
        std::mem::take(&mut self.actions)
    }
    /// Enable updates for this tab and request a fresh snapshot and redraw.
    fn activate(&mut self) {
        self.enabled = true;
        self.dirty = true;
        self.redraw = true;
        self.actions.push(Action::BackgroundCancelled(false));
    }
    /// Cancel transient selection, input, lookup and clipboard state when leaving this tab.
    fn deactivate(&mut self) {
        self.cancel_copy();
        self.locating = None;
        self.enabled = false;
        self.input = None;
        self.overlay = None;
        self.selection = None;
        self.dragging = false;
        self.drag_anchor = None;
        self.actions.push(Action::BackgroundCancelled(true));
    }
    /// Describe current navigation and scan state for the application status bar.
    fn status(&self) -> Status {
        let message = if !self.message.is_empty() {
            self.message.clone()
        } else if let Some(s) = &self.snapshot {
            format!(
                "{} · {} · record {} / {}{} · {}",
                s.header.version,
                self.contig.as_deref().unwrap_or("All records"),
                if s.total == 0 { 0 } else { self.selected + 1 },
                s.total,
                if s.progress.complete { "" } else { "+" },
                if s.paused {
                    "Paused"
                } else if s.progress.complete {
                    "Indexed"
                } else {
                    "Indexing"
                }
            )
        } else {
            "Opening…".into()
        };
        Status { message, hints: "? help  c contig  : position  Enter details  1/2/3 detail tab  y copy  Tab file  q quit".into() }
    }
    /// Expose the current position or contig editor to the application prompt widget.
    fn input_prompt(&self) -> Option<InputPrompt<'_>> {
        match &self.input {
            Some(Input::Position(text)) => Some(InputPrompt {
                label: "Position (1-based): ",
                text,
            }),
            Some(Input::Contig(text)) => Some(InputPrompt {
                label: "Contig: ",
                text,
            }),
            None => None,
        }
    }
    /// Report whether a selected record or informational overlay is available to copy.
    fn can_copy(&self) -> bool {
        self.entry().is_some() || self.overlay.is_some()
    }
    /// Accept completion only for this active tab and its uncancelled copy operation.
    fn accepts_copy(&self, id: u64) -> bool {
        self.enabled
            && self
                .pending_copy
                .as_ref()
                .is_some_and(|op| op.id == id && !op.cancel.load(Ordering::Relaxed))
    }
    /// Show the matching clipboard completion message and request a redraw.
    fn copy_finished(&mut self, id: u64, message: String) {
        if self.accepts_copy(id) {
            self.pending_copy = None;
            self.message = message;
            self.redraw = true;
        }
    }
    /// Invalidate viewport geometry and drag selections while retaining navigation context.
    fn resized(&mut self) {
        self.dirty = true;
        self.redraw = true;
        self.selection = None;
        self.dragging = false;
        self.drag_anchor = None;
        self.info.selection = None;
    }
}
impl Drop for VcfView {
    /// Cancel an outstanding clipboard operation when the document is removed.
    fn drop(&mut self) {
        self.cancel_copy();
    }
}
