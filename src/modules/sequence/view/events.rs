//! Keyboard, pointer and scrolling actions applied to application state.
use super::{Focus, Input, OperationKind, Page, SequenceView};
use crate::modules::sequence::protocol::Request;
use crossterm::event::{KeyCode, KeyEvent, KeyModifiers, MouseButton, MouseEvent, MouseEventKind};
use ratatui::layout::Rect;

impl SequenceView {
    pub(crate) fn key(&mut self, key: KeyEvent) {
        self.redraw = true;
        if self.input.is_some() {
            self.input_key(key);
            return;
        }
        if let Some(page) = self.page {
            match key.code {
                KeyCode::Char('q') => {
                    self.page = None;
                    self.info.selection = None;
                }
                KeyCode::Esc => {
                    self.cancel_operation();
                    self.set_background_cancelled(true);
                    self.send(Request::Cancel(self.file));
                    self.page = None;
                    self.info.selection = None;
                    self.dirty = true;
                }
                KeyCode::Tab | KeyCode::Right => self.open_page(page.next(false)),
                KeyCode::BackTab | KeyCode::Left => self.open_page(page.next(true)),
                KeyCode::Down | KeyCode::Char('j') => self.info.scroll += 1,
                KeyCode::Up | KeyCode::Char('k') => {
                    self.info.scroll = self.info.scroll.saturating_sub(1)
                }
                KeyCode::PageDown => self.info.scroll += self.hits.info.height.max(1) as usize,
                KeyCode::PageUp => {
                    self.info.scroll = self
                        .info
                        .scroll
                        .saturating_sub(self.hits.info.height.max(1) as usize)
                }
                KeyCode::Home => self.info.scroll = 0,
                KeyCode::End => {
                    self.info.scroll = self
                        .info
                        .lines
                        .len()
                        .saturating_sub(self.hits.info.height as usize)
                }
                KeyCode::Char('y') => self.copy_info(),
                KeyCode::F(1) => self.open_page(Page::Help),
                KeyCode::F(2) => self.open_page(Page::File),
                KeyCode::F(3) => self.open_page(Page::Record),
                KeyCode::F(4) => self.open_page(Page::Statistics),
                _ => {}
            }
            return;
        }
        match key.code {
            KeyCode::Char('?') | KeyCode::F(1) => self.open_page(Page::Help),
            KeyCode::F(2) => self.open_page(Page::File),
            KeyCode::F(3) => self.open_page(Page::Record),
            KeyCode::F(4) => self.open_page(Page::Statistics),
            KeyCode::Char('/') => self.input = Some(Input::Search(String::new())),
            KeyCode::Char(':') => self.input = Some(Input::Position(String::new())),
            KeyCode::Char('n') => {
                if !self.tab().last_search.is_empty() {
                    self.request_view();
                    let (operation_id, cancel) = self.begin_operation(OperationKind::Search);
                    self.send(Request::Search {
                        operation_id,
                        cancel,
                        file: self.file,
                        query: self.tab().last_search.clone(),
                        after: self.tab().record + 1,
                    });
                    self.tab_mut().message = "Searching… Esc cancels".into();
                }
            }
            KeyCode::Char('Q') => {
                self.config.show_quality = !self.config.show_quality;
                self.dirty = true;
            }
            KeyCode::Enter | KeyCode::Right if self.focus == Focus::Records => {
                self.focus = Focus::Sequence
            }
            KeyCode::Left if self.focus == Focus::Records => {}
            KeyCode::Char('[') => self.choose_record(self.tab().record.saturating_sub(1)),
            KeyCode::Char(']') => self.choose_record(self.tab().record + 1),
            KeyCode::Char('h') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                self.focus = Focus::Records
            }
            KeyCode::Backspace => self.focus = Focus::Records,
            KeyCode::Char('l') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                self.focus = Focus::Sequence
            }
            KeyCode::Down | KeyCode::Char('j') => self.move_vertical(1),
            KeyCode::Up | KeyCode::Char('k') => self.move_vertical(-1),
            KeyCode::Left | KeyCode::Char('h') => self.move_cursor(-1),
            KeyCode::Right | KeyCode::Char('l') => self.move_cursor(1),
            KeyCode::PageDown => self.scroll(1),
            KeyCode::PageUp => self.scroll(-1),
            KeyCode::Home => {
                if self.focus == Focus::Records {
                    self.choose_record(0);
                } else {
                    self.tab_mut().start = 0;
                    self.tab_mut().cursor = 0;
                    self.dirty = true;
                }
            }
            KeyCode::End => self.end(),
            KeyCode::Char('v') if self.focus == Focus::Sequence && self.can_copy() => {
                let cursor = self.tab().cursor;
                self.tab_mut().selection = if self.tab().selection.is_some() {
                    None
                } else {
                    Some((cursor, cursor))
                };
            }
            KeyCode::Char('y') => self.copy(),
            KeyCode::Esc => {
                self.cancel_operation();
                self.set_background_cancelled(true);
                self.send(Request::Cancel(self.file));
                self.tab_mut().selection = None;
                self.tab_mut().pending_end = false;
                self.drag = None;
                self.dirty = true;
            }
            _ => {}
        }
    }

    pub(crate) fn input_key(&mut self, key: KeyEvent) {
        match key.code {
            KeyCode::Esc => self.input = None,
            KeyCode::Enter => match self.input.take().unwrap() {
                Input::Search(query) if !query.is_empty() => {
                    self.tab_mut().last_search = query.clone();
                    self.request_view();
                    let (operation_id, cancel) = self.begin_operation(OperationKind::Search);
                    self.send(Request::Search {
                        operation_id,
                        cancel,
                        file: self.file,
                        query,
                        after: 0,
                    });
                    self.tab_mut().message = "Searching… Esc cancels".into();
                }
                Input::Position(text) => match text.parse::<u64>() {
                    Ok(position) if position > 0 => {
                        if self
                            .tab()
                            .snapshot
                            .as_ref()
                            .and_then(|s| s.record.as_ref())
                            .is_some_and(|r| !r.has_sequence || (r.complete && position > r.length))
                        {
                            self.tab_mut().message = "Position is outside this record".into();
                        } else {
                            self.tab_mut().start = position - 1;
                            self.tab_mut().cursor = position - 1;
                            self.focus = Focus::Sequence;
                            self.dirty = true;
                        }
                    }
                    _ => self.tab_mut().message = "Enter a 1-based sequence position".into(),
                },
                _ => {}
            },
            _ => {
                if let Some(Input::Search(text) | Input::Position(text)) = self.input.as_mut() {
                    crate::widgets::input::edit(text, key);
                }
            }
        }
    }

    pub(crate) fn move_vertical(&mut self, delta: i64) {
        if self.focus == Focus::Records {
            self.choose_record(self.tab().record.saturating_add_signed(delta));
        } else {
            self.move_cursor(delta * self.hits.bases_per_line.max(1) as i64);
        }
    }

    pub(crate) fn move_cursor(&mut self, delta: i64) {
        let width = self.hits.bases_per_line.max(1) as u64;
        let count = self.count() as u64;
        let tab = self.tab_mut();
        let mut next = tab.cursor.saturating_add_signed(delta);
        if let Some(record) = tab.snapshot.as_ref().and_then(|s| s.record.as_ref())
            && record.complete
        {
            next = next.min(record.length.saturating_sub(1));
        }
        tab.cursor = next;
        if let Some((_, end)) = &mut tab.selection {
            *end = next;
        }
        if next < tab.start {
            tab.start = next;
            self.dirty = true;
        } else if next >= tab.start + count {
            tab.start = next.saturating_sub(count.saturating_sub(width));
            self.dirty = true;
        }
    }

    pub(crate) fn scroll(&mut self, delta: i64) {
        if self.focus == Focus::Records {
            self.choose_record(
                self.tab()
                    .record
                    .saturating_add_signed(delta * self.hits.list.height.max(1) as i64),
            );
        } else {
            let step = self.count() as i64;
            let tab = self.tab_mut();
            tab.start = tab.start.saturating_add_signed(delta * step);
            if let Some(record) = tab.snapshot.as_ref().and_then(|s| s.record.as_ref())
                && record.complete
            {
                tab.start = tab.start.min(record.length.saturating_sub(1));
            }
            tab.cursor = tab.start;
            self.dirty = true;
        }
    }

    pub(crate) fn end(&mut self) {
        if let Some(snapshot) = &self.tab().snapshot {
            if self.focus == Focus::Records && snapshot.progress.complete {
                self.choose_record(snapshot.progress.records.saturating_sub(1));
                return;
            }
            if self.focus == Focus::Sequence
                && let Some(record) = &snapshot.record
                && record.complete
            {
                let start = record.length.saturating_sub(self.count() as u64);
                let cursor = record.length.saturating_sub(1);
                self.tab_mut().start = start;
                self.tab_mut().cursor = cursor;
                self.dirty = true;
                return;
            }
        }
        self.tab_mut().pending_end = true;
        self.set_background_cancelled(false);
        self.send(Request::Statistics { file: self.file });
        self.tab_mut().message = "Finding the end… Esc cancels".into();
    }

    pub(crate) fn sequence_position(&self, x: u16, y: u16) -> Option<u64> {
        let record = self.tab().snapshot.as_ref()?.record.as_ref()?;
        self.hits
            .sequence_position(self.tab().start, record.length, x, y)
    }

    pub(crate) fn info_position(&self, x: u16, y: u16) -> (usize, usize) {
        self.info.position(self.hits.info, x, y)
    }

    pub(crate) fn mouse(&mut self, mouse: MouseEvent) {
        self.redraw = true;
        if self.hits.too_small {
            return;
        }
        let (x, y) = (mouse.column, mouse.row);
        if self.page.is_some() {
            match mouse.kind {
                MouseEventKind::ScrollDown => self.info.scroll += 3,
                MouseEventKind::ScrollUp => self.info.scroll = self.info.scroll.saturating_sub(3),
                MouseEventKind::Down(MouseButton::Left)
                    if self.can_copy() && contains(self.hits.info, x, y) =>
                {
                    let p = self.info_position(x, y);
                    self.info.selection = Some((p, p));
                }
                MouseEventKind::Drag(MouseButton::Left) if self.can_copy() => {
                    let p = self.info_position(x, y);
                    if let Some((_, end)) = &mut self.info.selection {
                        *end = p;
                    }
                    if y >= self.hits.info.bottom() {
                        self.info.scroll += 1;
                    } else if y < self.hits.info.y {
                        self.info.scroll = self.info.scroll.saturating_sub(1);
                    }
                }
                _ => {}
            }
            return;
        }
        match mouse.kind {
            MouseEventKind::Down(MouseButton::Left) => {
                if contains(self.hits.list, x, y) {
                    if self
                        .tab()
                        .snapshot
                        .as_ref()
                        .is_none_or(|s| s.request.list_start != self.tab().list_start)
                    {
                        return;
                    }
                    self.focus = Focus::Records;
                    self.choose_record(self.tab().list_start + u64::from(y - self.hits.list.y));
                } else if contains(self.hits.sequence, x, y) {
                    if self.tab().snapshot.as_ref().is_none_or(|s| {
                        s.request.record != self.tab().record || s.window.start != self.tab().start
                    }) {
                        return;
                    }
                    self.focus = Focus::Sequence;
                    if let Some(p) = self.sequence_position(x, y) {
                        self.tab_mut().cursor = p;
                        if self.can_copy() {
                            self.tab_mut().selection = Some((p, p));
                            self.drag = Some((x, y));
                        }
                    }
                }
            }
            MouseEventKind::Drag(MouseButton::Left) if self.drag.is_some() => {
                self.drag = Some((x, y));
                if let Some(p) = self.sequence_position(x, y)
                    && let Some((_, end)) = &mut self.tab_mut().selection
                {
                    *end = p;
                }
            }
            MouseEventKind::Up(MouseButton::Left) => self.drag = None,
            MouseEventKind::ScrollDown | MouseEventKind::ScrollUp => {
                let delta = if mouse.kind == MouseEventKind::ScrollDown {
                    3
                } else {
                    -3
                };
                if contains(self.hits.list, x, y) {
                    self.focus = Focus::Records;
                    self.move_vertical(delta);
                } else {
                    self.focus = Focus::Sequence;
                    let step = delta * self.hits.bases_per_line.max(1) as i64;
                    let mut next = self.tab().start.saturating_add_signed(step);
                    if let Some(record) =
                        self.tab().snapshot.as_ref().and_then(|s| s.record.as_ref())
                        && record.complete
                    {
                        next = next.min(record.length.saturating_sub(1));
                    }
                    self.tab_mut().start = next;
                    self.tab_mut().cursor = self.tab().start;
                    self.dirty = true;
                }
            }
            _ => {}
        }
    }

    pub(crate) fn drag_scroll(&mut self) {
        if self.hits.too_small || self.hits.sequence_rows == 0 {
            return;
        }
        let Some((x, y)) = self.drag else {
            return;
        };
        let delta = if y >= self.hits.sequence.bottom().saturating_sub(1) {
            1
        } else if y <= self.hits.sequence.y {
            -1
        } else {
            0
        };
        if delta == 0 {
            return;
        }
        let next = self
            .tab()
            .start
            .saturating_add_signed(delta * self.hits.bases_per_line.max(1) as i64);
        if self
            .tab()
            .snapshot
            .as_ref()
            .and_then(|s| s.record.as_ref())
            .is_some_and(|r| r.complete && next >= r.length)
        {
            return;
        }
        self.tab_mut().start = next;
        // The pointer maps through the target viewport even while its data is loading.
        if let Some(p) = self.sequence_position(x, y)
            && let Some((_, end)) = &mut self.tab_mut().selection
        {
            *end = p;
        }
        self.dirty = true;
        self.redraw = true;
    }
}

fn contains(rect: Rect, x: u16, y: u16) -> bool {
    x >= rect.x && x < rect.right() && y >= rect.y && y < rect.bottom()
}
