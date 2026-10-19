mod render;

use crate::clipboard::{self, CopyMethod};
use crate::config::Config;
use crate::worker::{self, Request, Snapshot, Update, ViewRequest, Worker};
use crossterm::event::{
    self, DisableMouseCapture, EnableMouseCapture, Event, KeyCode, KeyEvent, KeyEventKind,
    KeyModifiers, MouseButton, MouseEvent, MouseEventKind,
};
use crossterm::execute;
use crossterm::terminal::{
    EnterAlternateScreen, LeaveAlternateScreen, disable_raw_mode, enable_raw_mode,
};
use ratatui::{Terminal, backend::CrosstermBackend, layout::Rect};
use std::io;
use std::path::PathBuf;
use std::sync::atomic::Ordering;
use std::time::{Duration, Instant};

#[derive(Clone, Copy, PartialEq, Eq)]
enum Focus {
    Records,
    Sequence,
}
#[derive(Clone, Copy, PartialEq, Eq)]
enum Page {
    Help,
    File,
    Record,
    Statistics,
}
impl Page {
    fn next(self, backwards: bool) -> Self {
        let pages = [Self::Help, Self::File, Self::Record, Self::Statistics];
        let i = pages.iter().position(|p| *p == self).unwrap();
        pages[(i + if backwards { 3 } else { 1 }) % 4]
    }
}
enum Input {
    Search(String),
    Position(String),
}

struct Tab {
    path: PathBuf,
    closed: bool,
    record: u64,
    start: u64,
    cursor: u64,
    selection: Option<(u64, u64)>,
    list_start: u64,
    snapshot: Option<Box<Snapshot>>,
    message: String,
    last_search: String,
    pending_end: bool,
}

#[derive(Default)]
struct HitMap {
    list: Rect,
    sequence: Rect,
    bases_per_line: usize,
    sequence_rows: usize,
    stride: u16,
    gutter: u16,
    tabs: Vec<(usize, Rect)>,
    info: Rect,
}

struct App {
    tabs: Vec<Tab>,
    active: usize,
    focus: Focus,
    page: Option<Page>,
    input: Option<Input>,
    config: Config,
    hits: HitMap,
    token: u64,
    dirty: bool,
    quit: bool,
    drag: Option<(u16, u16)>,
    info_scroll: usize,
    info_lines: Vec<String>,
    info_selection: Option<((usize, usize), (usize, usize))>,
    worker: Worker,
}

struct TerminalGuard;
impl Drop for TerminalGuard {
    fn drop(&mut self) {
        let _ = disable_raw_mode();
        let _ = execute!(io::stdout(), DisableMouseCapture, LeaveAlternateScreen);
    }
}

pub fn run(paths: Vec<PathBuf>, config: Config) -> io::Result<()> {
    let worker = Worker::start(paths.clone(), worker::cache_directory());
    let mut app = App::new(paths, config, worker);
    enable_raw_mode()?;
    let _guard = TerminalGuard;
    execute!(io::stdout(), EnterAlternateScreen, EnableMouseCapture)?;
    let mut terminal = Terminal::new(CrosstermBackend::new(io::stdout()))?;
    let mut last_drag = Instant::now();
    while !app.quit {
        terminal.draw(|frame| render::draw(frame, &mut app))?;
        if app.dirty {
            app.request_view();
        }
        app.receive();
        if last_drag.elapsed() >= Duration::from_millis(60) {
            app.drag_scroll();
            last_drag = Instant::now();
        }
        if event::poll(Duration::from_millis(30))? {
            match event::read()? {
                Event::Key(key) if key.kind != KeyEventKind::Release => app.key(key),
                Event::Mouse(mouse) => app.mouse(mouse),
                Event::Resize(_, _) => {
                    app.dirty = true;
                    app.info_selection = None;
                }
                _ => {}
            }
        }
    }
    Ok(())
}

impl App {
    fn new(paths: Vec<PathBuf>, config: Config, worker: Worker) -> Self {
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
            hits: HitMap::default(),
            token: 0,
            dirty: true,
            quit: false,
            drag: None,
            info_scroll: 0,
            info_lines: Vec::new(),
            info_selection: None,
            worker,
        }
    }
    fn tab(&self) -> &Tab {
        &self.tabs[self.active]
    }
    fn tab_mut(&mut self) -> &mut Tab {
        &mut self.tabs[self.active]
    }
    fn can_copy(&self) -> bool {
        self.tab()
            .snapshot
            .as_ref()
            .is_some_and(|s| s.format.can_copy())
    }
    fn count(&self) -> usize {
        (self.hits.bases_per_line.max(1) * self.hits.sequence_rows.max(1)).min(128 * 1024)
    }

    fn request_view(&mut self) {
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

    fn receive(&mut self) {
        while let Ok(update) = self.worker.updates.try_recv() {
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
                    token,
                } if file == self.active && !self.tabs[file].closed && token == self.token => {
                    self.choose_record(record);
                    self.tab_mut().message = "Match found".into();
                }
                Update::Copied { file, text, token }
                    if file == self.active
                        && !self.tabs[file].closed
                        && token == self.token
                        && !self.worker.cancel_copy.load(Ordering::Relaxed) =>
                {
                    self.copy_text(&text);
                }
                Update::Message { file, text } if !self.tabs[file].closed => {
                    self.tabs[file].message = text
                }
                _ => {}
            }
        }
    }

    fn choose_record(&mut self, record: u64) {
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

    fn switch_tab(&mut self, next: usize) {
        self.worker.cancel_copy.store(true, Ordering::Relaxed);
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

    fn cycle_tab(&mut self, backwards: bool) {
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

    fn open_page(&mut self, page: Page) {
        self.page = Some(page);
        self.info_scroll = 0;
        self.info_selection = None;
        if page == Page::Statistics {
            self.worker.cancel_copy.store(false, Ordering::Relaxed);
            self.worker.send(Request::Statistics { file: self.active });
        }
    }

    fn key(&mut self, key: KeyEvent) {
        if key.code == KeyCode::Char('c') && key.modifiers.contains(KeyModifiers::CONTROL) {
            self.quit = true;
            return;
        }
        if self.input.is_some() {
            self.input_key(key);
            return;
        }
        if let Some(page) = self.page {
            match key.code {
                KeyCode::Char('q') => {
                    self.page = None;
                    self.info_selection = None;
                }
                KeyCode::Esc => {
                    self.worker.cancel_copy.store(true, Ordering::Relaxed);
                    self.worker.send(Request::Cancel(self.active));
                    self.page = None;
                    self.info_selection = None;
                    self.dirty = true;
                }
                KeyCode::Tab | KeyCode::Right => self.open_page(page.next(false)),
                KeyCode::BackTab | KeyCode::Left => self.open_page(page.next(true)),
                KeyCode::Down | KeyCode::Char('j') => self.info_scroll += 1,
                KeyCode::Up | KeyCode::Char('k') => {
                    self.info_scroll = self.info_scroll.saturating_sub(1)
                }
                KeyCode::PageDown => self.info_scroll += self.hits.info.height.max(1) as usize,
                KeyCode::PageUp => {
                    self.info_scroll = self
                        .info_scroll
                        .saturating_sub(self.hits.info.height.max(1) as usize)
                }
                KeyCode::Home => self.info_scroll = 0,
                KeyCode::End => {
                    self.info_scroll = self
                        .info_lines
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
            KeyCode::Char('q') => self.quit = true,
            KeyCode::Tab => self.cycle_tab(false),
            KeyCode::BackTab => self.cycle_tab(true),
            KeyCode::Char('x') => {
                self.worker.cancel_copy.store(true, Ordering::Relaxed);
                self.worker.send(Request::Close(self.active));
                self.tab_mut().closed = true;
                self.tab_mut().snapshot = None;
                if self.tabs.iter().all(|t| t.closed) {
                    self.quit = true;
                } else {
                    self.cycle_tab(false);
                }
            }
            KeyCode::Char('?') | KeyCode::F(1) => self.open_page(Page::Help),
            KeyCode::F(2) => self.open_page(Page::File),
            KeyCode::F(3) => self.open_page(Page::Record),
            KeyCode::F(4) => self.open_page(Page::Statistics),
            KeyCode::Char('/') => self.input = Some(Input::Search(String::new())),
            KeyCode::Char(':') => self.input = Some(Input::Position(String::new())),
            KeyCode::Char('n') => {
                if !self.tab().last_search.is_empty() {
                    self.request_view();
                    self.worker.cancel_copy.store(false, Ordering::Relaxed);
                    self.worker.send(Request::Search {
                        token: self.token,
                        file: self.active,
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
                self.worker.cancel_copy.store(true, Ordering::Relaxed);
                self.worker.send(Request::Cancel(self.active));
                self.tab_mut().selection = None;
                self.tab_mut().pending_end = false;
                self.drag = None;
                self.dirty = true;
            }
            _ => {}
        }
    }

    fn input_key(&mut self, key: KeyEvent) {
        match key.code {
            KeyCode::Esc => self.input = None,
            KeyCode::Enter => match self.input.take().unwrap() {
                Input::Search(query) if !query.is_empty() => {
                    self.tab_mut().last_search = query.clone();
                    self.request_view();
                    self.worker.cancel_copy.store(false, Ordering::Relaxed);
                    self.worker.send(Request::Search {
                        token: self.token,
                        file: self.active,
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
            KeyCode::Backspace => match self.input.as_mut().unwrap() {
                Input::Search(text) | Input::Position(text) => {
                    text.pop();
                }
            },
            KeyCode::Char(c) if !key.modifiers.contains(KeyModifiers::CONTROL) => {
                match self.input.as_mut().unwrap() {
                    Input::Search(text) | Input::Position(text) => text.push(c),
                }
            }
            _ => {}
        }
    }

    fn move_vertical(&mut self, delta: i64) {
        if self.focus == Focus::Records {
            self.choose_record(self.tab().record.saturating_add_signed(delta));
        } else {
            self.move_cursor(delta * self.hits.bases_per_line.max(1) as i64);
        }
    }

    fn move_cursor(&mut self, delta: i64) {
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

    fn scroll(&mut self, delta: i64) {
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

    fn end(&mut self) {
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
        self.worker.cancel_copy.store(false, Ordering::Relaxed);
        self.worker.send(Request::Statistics { file: self.active });
        self.tab_mut().message = "Finding the end… Esc cancels".into();
    }

    fn copy(&mut self) {
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
        self.worker.cancel_copy.store(false, Ordering::Relaxed);
        self.worker.send(Request::Copy {
            token: self.token,
            file: self.active,
            record: self.tab().record,
            range,
        });
    }

    fn copy_text(&mut self, text: &str) {
        self.tab_mut().message = match clipboard::copy_text(text, self.can_copy()) {
            Ok(CopyMethod::System) => "Copied to clipboard".into(),
            Ok(CopyMethod::Osc52) => "Clipboard request sent to terminal (OSC 52)".into(),
            Err(error) => error.to_string(),
        };
    }

    fn copy_info(&mut self) {
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
        self.copy_text(&text);
    }

    fn sequence_position(&self, x: u16, y: u16) -> Option<u64> {
        let rect = self.hits.sequence;
        if rect.width == 0 || rect.height == 0 || x < rect.x + self.hits.gutter {
            return None;
        }
        let row =
            y.saturating_sub(rect.y).min(rect.height.saturating_sub(1)) / self.hits.stride.max(1);
        let column = x
            .saturating_sub(rect.x + self.hits.gutter)
            .min(self.hits.bases_per_line.saturating_sub(1) as u16);
        let position =
            self.tab().start + row as u64 * self.hits.bases_per_line as u64 + u64::from(column);
        let snapshot = self.tab().snapshot.as_ref()?;
        if snapshot.request.record != self.tab().record || snapshot.window.start != self.tab().start
        {
            return None;
        }
        let record = snapshot.record.as_ref()?;
        if record.length == 0 {
            None
        } else {
            Some(position.min(record.length - 1))
        }
    }

    fn info_position(&self, x: u16, y: u16) -> (usize, usize) {
        let row = self.info_scroll
            + usize::from(
                y.saturating_sub(self.hits.info.y)
                    .min(self.hits.info.height.saturating_sub(1)),
            );
        let column = x.saturating_sub(self.hits.info.x) as usize;
        let mut used = 0;
        let mut index = 0;
        if let Some(line) = self.info_lines.get(row) {
            for c in line.chars() {
                let width = unicode_width::UnicodeWidthChar::width(c).unwrap_or(0);
                if used + width > column {
                    break;
                }
                used += width;
                index += 1;
            }
        }
        (row, index)
    }

    fn mouse(&mut self, mouse: MouseEvent) {
        let (x, y) = (mouse.column, mouse.row);
        if self.page.is_some() {
            match mouse.kind {
                MouseEventKind::ScrollDown => self.info_scroll += 3,
                MouseEventKind::ScrollUp => self.info_scroll = self.info_scroll.saturating_sub(3),
                MouseEventKind::Down(MouseButton::Left)
                    if self.can_copy() && contains(self.hits.info, x, y) =>
                {
                    let p = self.info_position(x, y);
                    self.info_selection = Some((p, p));
                }
                MouseEventKind::Drag(MouseButton::Left) if self.can_copy() => {
                    let p = self.info_position(x, y);
                    if let Some((_, end)) = &mut self.info_selection {
                        *end = p;
                    }
                    if y >= self.hits.info.bottom() {
                        self.info_scroll += 1;
                    } else if y < self.hits.info.y {
                        self.info_scroll = self.info_scroll.saturating_sub(1);
                    }
                }
                _ => {}
            }
            return;
        }
        match mouse.kind {
            MouseEventKind::Down(MouseButton::Left) => {
                if let Some((id, _)) = self
                    .hits
                    .tabs
                    .iter()
                    .find(|(_, rect)| contains(*rect, x, y))
                {
                    self.switch_tab(*id);
                } else if contains(self.hits.list, x, y) {
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

    fn drag_scroll(&mut self) {
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
        if let Some(p) = self.sequence_position(x, y)
            && let Some((_, end)) = &mut self.tab_mut().selection
        {
            *end = p;
        }
        self.dirty = true;
    }
}

fn contains(rect: Rect, x: u16, y: u16) -> bool {
    x >= rect.x && x < rect.right() && y >= rect.y && y < rect.bottom()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::data::Dataset;
    use ratatui::backend::TestBackend;

    fn app(path: PathBuf, cache: &std::path::Path) -> App {
        let mut data = Dataset::open(&path, cache).unwrap();
        while !data.progress().complete && data.progress().error.is_none() {
            data.step(64 * 1024).unwrap();
        }
        let snapshot = Snapshot {
            request: ViewRequest {
                token: 1,
                file: 0,
                record: 0,
                start: 0,
                count: 1024,
                list_start: 0,
                list_count: 10,
            },
            format: data.format(),
            compression: data.compression().into(),
            records: data.records(0, 10).unwrap(),
            record: data.record(0).unwrap(),
            window: data.read_range(0, 0, 1024).unwrap(),
            progress: data.progress(),
            stats: Some(data.stats().unwrap()),
            paused: false,
        };
        let paths = vec![path];
        let worker = Worker::start(paths.clone(), cache.into());
        let mut app = App::new(paths, Config::default(), worker);
        app.tabs[0].snapshot = Some(Box::new(snapshot));
        app.tabs[0].message.clear();
        app.token = 1;
        app
    }

    fn key(code: KeyCode) -> KeyEvent {
        KeyEvent::new(code, KeyModifiers::NONE)
    }

    #[test]
    fn renderer_selection_and_resize_keep_sequence_coordinates() {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("sample.fa");
        std::fs::write(
            &path,
            format!(">first description\n{}\n>second\nTT", "ACGT".repeat(100)),
        )
        .unwrap();
        let mut app = app(path, &temp.path().join("cache"));
        let mut terminal = Terminal::new(TestBackend::new(80, 24)).unwrap();
        terminal
            .draw(|frame| render::draw(frame, &mut app))
            .unwrap();
        let first_width = app.hits.bases_per_line;
        let x = app.hits.sequence.x + app.hits.gutter + 2;
        let y = app.hits.sequence.y;
        app.mouse(MouseEvent {
            kind: MouseEventKind::Down(MouseButton::Left),
            column: x,
            row: y,
            modifiers: KeyModifiers::NONE,
        });
        app.mouse(MouseEvent {
            kind: MouseEventKind::Drag(MouseButton::Left),
            column: x + 5,
            row: y + 1,
            modifiers: KeyModifiers::NONE,
        });
        app.mouse(MouseEvent {
            kind: MouseEventKind::Up(MouseButton::Left),
            column: x + 5,
            row: y + 1,
            modifiers: KeyModifiers::NONE,
        });
        assert_eq!(app.tab().selection, Some((2, first_width as u64 + 7)));
        let selection = app.tab().selection;
        terminal.backend_mut().resize(120, 30);
        terminal.autoresize().unwrap();
        terminal
            .draw(|frame| render::draw(frame, &mut app))
            .unwrap();
        assert_ne!(first_width, app.hits.bases_per_line);
        assert_eq!(app.tab().selection, selection);
        assert_eq!(app.tab().start, 0);
        app.open_page(Page::Help);
        app.key(key(KeyCode::Char('q')));
        assert!(app.page.is_none());
        assert!(!app.quit);
        app.key(key(KeyCode::Char('q')));
        assert!(app.quit);
    }

    #[test]
    fn sam_copy_is_disabled_in_sequence_and_information_views() {
        let temp = tempfile::tempdir().unwrap();
        let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("test/alignments.sam");
        let mut app = app(path, &temp.path().join("cache"));
        app.copy();
        assert!(app.tab().message.contains("disabled"));
        app.focus = Focus::Sequence;
        app.tab_mut().selection = Some((0, 1));
        app.copy();
        assert!(app.tab().message.contains("disabled"));
        app.open_page(Page::Record);
        app.copy_info();
        assert!(app.tab().message.contains("disabled"));
    }

    #[test]
    fn jump_is_one_based_and_rejects_positions_past_record() {
        let temp = tempfile::tempdir().unwrap();
        let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("test/test.fa");
        let mut app = app(path, &temp.path().join("cache"));
        app.input = Some(Input::Position("5".into()));
        app.key(key(KeyCode::Enter));
        assert_eq!(app.tab().start, 4);
        app.input = Some(Input::Position("9".into()));
        app.key(key(KeyCode::Enter));
        assert_eq!(app.tab().start, 4);
        assert!(app.tab().message.contains("outside"));
    }
}
