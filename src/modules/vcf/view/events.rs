use super::{Detail, Focus, Input, Overlay, VcfView, layout};
use crate::modules::vcf::protocol::Request;
use crate::modules::view::Action;
use crossterm::event::{KeyCode, KeyEvent, MouseButton, MouseEvent, MouseEventKind};

impl VcfView {
    pub(super) fn key_event(&mut self, event: KeyEvent) {
        self.redraw = true;
        if self.input.is_some() {
            self.input_event(event);
            return;
        }
        if self.overlay.is_some() {
            match event.code {
                KeyCode::Esc | KeyCode::Char('q') => {
                    self.cancel_copy();
                    self.overlay = None;
                    self.info.selection = None;
                    self.dragging = false;
                    self.drag_anchor = None;
                }
                KeyCode::Down | KeyCode::Char('j') => self.info.scroll += 1,
                KeyCode::Up | KeyCode::Char('k') => {
                    self.info.scroll = self.info.scroll.saturating_sub(1)
                }
                KeyCode::PageDown => {
                    self.info.scroll += self.layout.area.height.saturating_sub(2).max(1) as usize
                }
                KeyCode::PageUp => {
                    self.info.scroll = self
                        .info
                        .scroll
                        .saturating_sub(self.layout.area.height.saturating_sub(2).max(1) as usize)
                }
                KeyCode::Home => self.info.scroll = 0,
                KeyCode::End => {
                    self.info.scroll = self
                        .info
                        .lines
                        .len()
                        .saturating_sub(self.layout.area.height.saturating_sub(2) as usize)
                }
                KeyCode::Char('y') | KeyCode::Char('Y') => self.copy(false),
                KeyCode::F(2) => self.open_overlay(Overlay::File),
                KeyCode::F(3) => self.open_overlay(Overlay::Header),
                KeyCode::F(1) | KeyCode::Char('?') => self.open_overlay(Overlay::Help),
                _ => {}
            }
            return;
        }
        match event.code {
            KeyCode::Down | KeyCode::Char('j') => self.move_row(1),
            KeyCode::Up | KeyCode::Char('k') => self.move_row(-1),
            KeyCode::PageDown => self.move_row(if self.focus == Focus::Table {
                self.layout.rows.height.max(1)
            } else {
                self.layout.detail_body.height.max(1)
            } as i64),
            KeyCode::PageUp => self.move_row(
                -(if self.focus == Focus::Table {
                    self.layout.rows.height.max(1)
                } else {
                    self.layout.detail_body.height.max(1)
                } as i64),
            ),
            KeyCode::Home if self.focus == Focus::Table => self.change_record(0),
            KeyCode::End if self.focus == Focus::Table => {
                if let Some(snapshot) = &self.snapshot {
                    self.change_record(snapshot.total.saturating_sub(1));
                }
            }
            KeyCode::Home => {
                self.select_detail_row(0);
                self.horizontal = 0;
            }
            KeyCode::End => {
                self.select_detail_row(self.detail_len().saturating_sub(1));
            }
            KeyCode::Enter if self.layout.detail_body.height > 0 => {
                self.focus = Focus::Detail;
                self.selection = None;
            }
            KeyCode::Esc => {
                self.cancel_copy();
                self.selection = None;
                self.dragging = false;
                self.drag_anchor = None;
                let was_locating = self.locating.take().is_some();
                if self.focus == Focus::Detail && !was_locating {
                    self.focus = Focus::Table;
                } else {
                    self.focus = Focus::Table;
                    self.actions.push(Action::BackgroundCancelled(true));
                    self.send(Request::Cancel(self.file));
                    self.message = "Background work paused".into();
                    self.dirty |= was_locating;
                }
            }
            KeyCode::Char('1') => self.change_detail(Detail::Info),
            KeyCode::Char('2') => self.change_detail(Detail::Samples),
            KeyCode::Char('3') => self.change_detail(Detail::Raw),
            KeyCode::Char('[') => self.change_detail(Detail::at(self.detail.index() + 2)),
            KeyCode::Char(']') => self.change_detail(Detail::at(self.detail.index() + 1)),
            KeyCode::Left | KeyCode::Char('h') => {
                self.horizontal = self.horizontal.saturating_sub(4)
            }
            KeyCode::Right | KeyCode::Char('l') => {
                self.horizontal = self.horizontal.saturating_add(4)
            }
            KeyCode::Char('c') => {
                self.cancel_copy();
                self.input = Some(Input::Contig(String::new()));
                self.picker_row = 0;
            }
            KeyCode::Char(':') => {
                self.cancel_copy();
                self.input = Some(Input::Position(String::new()));
            }
            KeyCode::Char('y') => self.copy(false),
            KeyCode::Char('Y') => self.copy(true),
            KeyCode::F(1) | KeyCode::Char('?') => self.open_overlay(Overlay::Help),
            KeyCode::F(2) => self.open_overlay(Overlay::File),
            KeyCode::F(3) | KeyCode::Char('m') => self.open_overlay(Overlay::Header),
            _ => {}
        }
    }
    fn input_event(&mut self, event: KeyEvent) {
        let picker = matches!(self.input, Some(Input::Contig(_)));
        match event.code {
            KeyCode::Esc => {
                self.cancel_copy();
                self.input = None;
            }
            KeyCode::Down if picker => {
                self.picker_row =
                    (self.picker_row + 1).min(self.picker_options().len().saturating_sub(1))
            }
            KeyCode::Up if picker => self.picker_row = self.picker_row.saturating_sub(1),
            KeyCode::PageDown if picker => {
                self.picker_row =
                    (self.picker_row + 10).min(self.picker_options().len().saturating_sub(1))
            }
            KeyCode::PageUp if picker => self.picker_row = self.picker_row.saturating_sub(10),
            KeyCode::Enter if picker => self.choose_contig(),
            KeyCode::Enter => {
                let Some(Input::Position(text)) = self.input.take() else {
                    return;
                };
                let Some(position) = text.parse::<u64>().ok().filter(|&n| n > 0) else {
                    self.message = "Enter a positive 1-based position".into();
                    return;
                };
                let Some(contig) = self
                    .contig
                    .clone()
                    .or_else(|| self.entry().map(|e| e.record.chrom.clone()))
                else {
                    self.message = "Choose a contig first".into();
                    return;
                };
                self.contig = Some(contig.clone());
                self.token += 1;
                self.locating = Some(self.token);
                self.dirty = false;
                self.send(Request::Locate {
                    file: self.file,
                    token: self.token,
                    contig,
                    position,
                });
                self.message = "Locating position…".into();
            }
            _ => {
                if let Some(Input::Position(text) | Input::Contig(text)) = self.input.as_mut() {
                    crate::widgets::input::edit(text, event);
                }
                if picker {
                    self.picker_row = 0;
                }
            }
        }
    }
    fn choose_contig(&mut self) {
        let Some((contig, _)) = self.picker_options().get(self.picker_row).cloned() else {
            return;
        };
        self.cancel_copy();
        self.locating = None;
        self.contig = contig;
        self.selected = 0;
        self.start = 0;
        self.detail_start = 0;
        self.detail_row = 0;
        self.horizontal = 0;
        self.selection = None;
        self.dragging = false;
        self.drag_anchor = None;
        self.input = None;
        self.dirty = true;
        self.redraw = true;
        self.message.clear();
    }
    fn move_row(&mut self, delta: i64) {
        if self.focus == Focus::Table {
            self.change_record(self.selected.saturating_add_signed(delta));
            return;
        }
        self.select_detail_row(self.detail_row.saturating_add_signed(delta as isize));
    }
    fn select_detail_row(&mut self, row: usize) {
        self.selection = None;
        self.dragging = false;
        self.drag_anchor = None;
        self.detail_row = row.min(self.detail_len().saturating_sub(1));
        let height = self.layout.detail_body.height.max(1) as usize;
        if self.detail_row < self.detail_start {
            self.detail_start = self.detail_row;
        }
        if self.detail_row >= self.detail_start.saturating_add(height) {
            self.detail_start = self.detail_row.saturating_add(1).saturating_sub(height);
        }
    }
    pub(super) fn detail_position(&self, x: u16, y: u16) -> (usize, usize) {
        let area = self.layout.detail_body;
        let row = (self.detail_start
            + y.saturating_sub(area.y).min(area.height.saturating_sub(1)) as usize)
            .min(self.detail_len().saturating_sub(1));
        let column = x.saturating_sub(area.x) as usize + self.horizontal;
        let line = self
            .detail_rows(row, 1)
            .into_iter()
            .next()
            .unwrap_or_default();
        let mut used = 0;
        let mut index = 0;
        let stops = super::text::tab_stops(self);
        super::text::walk_graphemes(&line, |symbol| {
            let width = super::text::grapheme_width(symbol, used, &stops);
            if used + width > column {
                return false;
            }
            used += width;
            index += symbol.chars().count();
            true
        });
        (row, index)
    }
    pub(super) fn mouse_event(&mut self, event: MouseEvent) {
        self.redraw = true;
        let (x, y) = (event.column, event.row);
        if matches!(self.input, Some(Input::Contig(_))) {
            let (_, list, start) = self.picker_layout();
            match event.kind {
                MouseEventKind::Down(MouseButton::Left) if layout::contains(list, x, y) => {
                    self.picker_row = start + (y - list.y) as usize;
                    self.choose_contig();
                }
                MouseEventKind::ScrollDown => {
                    self.picker_row =
                        (self.picker_row + 3).min(self.picker_options().len().saturating_sub(1))
                }
                MouseEventKind::ScrollUp => self.picker_row = self.picker_row.saturating_sub(3),
                _ => {}
            }
            return;
        }
        if self.input.is_some() {
            return;
        }
        if self.overlay.is_some() {
            let inner = layout::inset(self.layout.area);
            match event.kind {
                MouseEventKind::ScrollDown => self.info.scroll += 3,
                MouseEventKind::ScrollUp => self.info.scroll = self.info.scroll.saturating_sub(3),
                MouseEventKind::Down(MouseButton::Left) if layout::contains(inner, x, y) => {
                    let point = self.info.position(inner, x, y);
                    self.info.selection = None;
                    self.drag_anchor = Some(point);
                    self.dragging = true;
                }
                MouseEventKind::Drag(MouseButton::Left) if self.dragging => {
                    let point = self.info.position(inner, x, y);
                    if let Some(anchor) = self.drag_anchor
                        && (point != anchor || self.info.selection.is_some())
                    {
                        self.info.selection = Some((anchor, point));
                    }
                }
                MouseEventKind::Up(MouseButton::Left) => {
                    self.dragging = false;
                    self.drag_anchor = None;
                }
                _ => {}
            }
            return;
        }
        match event.kind {
            MouseEventKind::Down(MouseButton::Left)
                if layout::contains(self.layout.header, x, y) =>
            {
                if y == self.layout.header.y {
                    self.open_overlay(Overlay::Header);
                } else {
                    self.input = Some(Input::Contig(String::new()));
                    self.picker_row = 0;
                }
            }
            MouseEventKind::Down(MouseButton::Left) if layout::contains(self.layout.rows, x, y) => {
                if let Some(ordinal) = self
                    .snapshot
                    .as_ref()
                    .filter(|s| s.request.start == self.start && s.request.contig == self.contig)
                    .and_then(|s| s.rows.get((y - self.layout.rows.y) as usize))
                    .map(|r| r.ordinal)
                {
                    self.focus = Focus::Table;
                    self.change_record(ordinal);
                }
            }
            MouseEventKind::Down(MouseButton::Left)
                if layout::contains(self.layout.detail_tabs, x, y) =>
            {
                self.change_detail(Detail::at(
                    ((x - self.layout.detail_tabs.x) / 12).min(2) as usize
                ));
                self.focus = Focus::Detail;
            }
            MouseEventKind::Down(MouseButton::Left)
                if layout::contains(self.layout.detail_body, x, y) =>
            {
                self.focus = Focus::Detail;
                let point = self.detail_position(x, y);
                self.detail_row = point.0;
                self.selection = None;
                self.drag_anchor = Some(point);
                self.dragging = true;
            }
            MouseEventKind::Drag(MouseButton::Left) if self.dragging => {
                if y >= self.layout.detail_body.bottom() {
                    self.detail_start = self
                        .detail_start
                        .saturating_add(1)
                        .min(self.detail_len().saturating_sub(1));
                } else if y < self.layout.detail_body.y {
                    self.detail_start = self.detail_start.saturating_sub(1);
                }
                let point = self.detail_position(x, y);
                if let Some(anchor) = self.drag_anchor
                    && (point != anchor || self.selection.is_some())
                {
                    self.selection = Some((anchor, point));
                }
            }
            MouseEventKind::Up(MouseButton::Left) => {
                self.dragging = false;
                self.drag_anchor = None;
            }
            MouseEventKind::ScrollDown | MouseEventKind::ScrollUp => {
                self.focus = if layout::contains(self.layout.table, x, y) {
                    Focus::Table
                } else {
                    Focus::Detail
                };
                self.move_row(if event.kind == MouseEventKind::ScrollDown {
                    3
                } else {
                    -3
                });
            }
            _ => {}
        }
    }
}
