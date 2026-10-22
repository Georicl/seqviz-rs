//! One VCF document: variant navigation and lazily materialized detail pages.
mod events;
mod layout;
mod render;
#[cfg(test)]
mod render_tests;
#[cfg(test)]
mod tests;
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

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Detail {
    Info,
    Samples,
    Raw,
}
impl Detail {
    fn index(self) -> usize {
        match self {
            Self::Info => 0,
            Self::Samples => 1,
            Self::Raw => 2,
        }
    }
    fn at(index: usize) -> Self {
        [Self::Info, Self::Samples, Self::Raw][index % 3]
    }
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Focus {
    Table,
    Detail,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Overlay {
    Help,
    File,
    Header,
}
enum Input {
    Position(String),
    Contig(String),
}
struct CopyOperation {
    id: u64,
    cancel: Arc<AtomicBool>,
}

pub(crate) struct VcfView {
    path: PathBuf,
    file: usize,
    config: VcfConfig,
    theme: Theme,
    clock: Arc<AtomicU64>,
    enabled: bool,
    snapshot: Option<Box<Snapshot>>,
    load_error: Option<String>,
    contig: Option<String>,
    selected: u64,
    start: u64,
    token: u64,
    locating: Option<u64>,
    focus: Focus,
    detail: Detail,
    detail_row: usize,
    detail_start: usize,
    horizontal: usize,
    selection: Option<((usize, usize), (usize, usize))>,
    dragging: bool,
    drag_anchor: Option<(usize, usize)>,
    overlay: Option<Overlay>,
    input: Option<Input>,
    picker_row: usize,
    info: text_panel::State,
    layout: layout::Layout,
    dirty: bool,
    redraw: bool,
    message: String,
    pending_copy: Option<CopyOperation>,
    actions: Vec<Action>,
}

impl VcfView {
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
    fn send(&mut self, request: Request) {
        self.actions
            .push(Action::Request(ModuleRequest::vcf(request)));
    }
    fn entry(&self) -> Option<&VcfEntry> {
        let snapshot = self.snapshot.as_ref()?;
        if snapshot.request.selected != self.selected || snapshot.request.contig != self.contig {
            return None;
        }
        snapshot.selected.as_deref()
    }
    fn cancel_copy(&mut self) {
        if let Some(copy) = self.pending_copy.take() {
            copy.cancel.store(true, Ordering::Relaxed);
        }
    }
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
    fn open_overlay(&mut self, overlay: Overlay) {
        self.cancel_copy();
        self.dragging = false;
        self.drag_anchor = None;
        self.overlay = Some(overlay);
        self.info = text_panel::State::default();
        self.redraw = true;
        self.refresh_overlay();
    }
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
    fn draw(&self, frame: &mut Frame) {
        render::draw(frame, self);
    }
    fn key(&mut self, event: crossterm::event::KeyEvent) {
        self.key_event(event);
    }
    fn mouse(&mut self, event: crossterm::event::MouseEvent) {
        self.mouse_event(event);
    }
    fn tick(&mut self) {}
    fn captures_input(&self) -> bool {
        self.input.is_some() || self.overlay.is_some()
    }
    fn captures_tab_mouse(&self) -> bool {
        self.input.is_some() || self.overlay.is_some()
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
        self.request();
    }
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
    fn actions(&mut self) -> Vec<Action> {
        std::mem::take(&mut self.actions)
    }
    fn activate(&mut self) {
        self.enabled = true;
        self.dirty = true;
        self.redraw = true;
        self.actions.push(Action::BackgroundCancelled(false));
    }
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
    fn can_copy(&self) -> bool {
        self.entry().is_some() || self.overlay.is_some()
    }
    fn accepts_copy(&self, id: u64) -> bool {
        self.enabled
            && self
                .pending_copy
                .as_ref()
                .is_some_and(|op| op.id == id && !op.cancel.load(Ordering::Relaxed))
    }
    fn copy_finished(&mut self, id: u64, message: String) {
        if self.accepts_copy(id) {
            self.pending_copy = None;
            self.message = message;
            self.redraw = true;
        }
    }
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
    fn drop(&mut self) {
        self.cancel_copy();
    }
}
