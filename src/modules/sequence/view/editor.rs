//! Record-local insert mode. Cursor and selection are insertion boundaries rather
//! than terminal cells; the draft changes only on explicit editing input.
use super::{Focus, SequenceView};
use crate::modules::sequence::{data::SeqType, palette::ContentPalette};
use crate::modules::{
    sequence::{
        data::edit::{Draft, is_residue},
        protocol::Request,
    },
    view::Status,
};
use crate::widgets::theme::Theme;
use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use ratatui::{
    Frame,
    layout::Rect,
    style::{Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, Paragraph},
};
use std::{
    ops::Range,
    sync::{Arc, atomic::Ordering},
};

/// Inputs received between pressing i and the worker delivering the record.
pub(super) enum PendingInput {
    Key(KeyEvent),
    Paste(String),
}

/// A single record draft, with an insertion cursor and an optional selection anchor.
pub(super) struct Editor {
    seq_type: SeqType,
    pub(super) pending: Vec<PendingInput>,
    pub(super) id: u64,
    pub(super) cursor: usize,
    anchor: Option<usize>,
    draft: Option<Arc<Draft>>,
    name: String,
    quality: bool,
    changed: bool,
    discard: bool,
    pub(super) saving: bool,
    pub(super) message: String,
}

impl SequenceView {
    pub(super) fn open_editor(&mut self) {
        let Some(snapshot) = &self.document.snapshot else {
            return;
        };
        if snapshot.format.is_alignment() {
            self.document.message = "SAM/BAM is read-only".into();
            return;
        }
        let Some(record) = snapshot.record.as_ref().filter(|r| {
            r.complete
                && r.id == self.document.record
                && snapshot.request.position == self.document.position
                && snapshot.request.query.as_ref() == &self.document.query
        }) else {
            self.document.message = "Wait for the selected record to finish indexing".into();
            return;
        };
        let (cursor, anchor) = match self.document.selection {
            Some((a, b)) => ((a.max(b) + 1) as usize, Some(a.min(b) as usize)),
            None => (self.document.cursor as usize, None),
        };
        let id = self.operation_ids.fetch_add(1, Ordering::Relaxed) + 1;
        let record_id = record.id;
        let source_identity = snapshot.source_identity;
        self.editor = Some(Editor {
            seq_type: record.seq_type,
            pending: Vec::new(),
            id,
            cursor,
            anchor,
            draft: None,
            name: record.header.clone(),
            quality: false,
            changed: false,
            discard: false,
            saving: false,
            message: "Loading record…".into(),
        });
        self.focus = Focus::Sequence;
        self.drag = None;
        self.cancel_operation();
        self.send(Request::Edit {
            source_identity,
            file: self.file,
            operation_id: id,
            record: record_id,
        });
    }

    pub(super) fn edit_key(&mut self, key: KeyEvent) {
        let edit = self.editor.as_mut().unwrap();
        if edit.saving {
            return;
        }
        if key.code == KeyCode::Esc
            || key == KeyEvent::new(KeyCode::Char('c'), KeyModifiers::CONTROL)
        {
            // The queued input may change the draft. Replay it before deciding
            // whether leaving requires confirmation.
            if !edit.pending.is_empty() {
                edit.pending.push(PendingInput::Key(key));
                return;
            }
            if edit.changed && !edit.discard {
                edit.discard = true;
                edit.message = "Unsaved changes · Esc again to discard · Ctrl+S to save".into();
            } else {
                self.editor = None;
                self.document.message = "Back to browsing".into();
            }
            return;
        }
        edit.discard = false;
        let Some(draft) = &edit.draft else {
            edit.pending.push(PendingInput::Key(key));
            return;
        };
        let len = draft.bases.len();
        let width = Editor::width(self.hits.area, len);
        let step = width.saturating_mul(Editor::height(self.hits.area, draft.quality.is_some()));
        let shift = key.modifiers.contains(KeyModifiers::SHIFT);
        match key.code {
            KeyCode::Char('s') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                if !edit.changed {
                    self.editor = None;
                    self.document.message = "No changes to save".into();
                    return;
                }
                if let Some(pos) = draft
                    .quality
                    .as_ref()
                    .and_then(|q| q.iter().position(|&b| b == 0))
                {
                    edit.quality = true;
                    edit.cursor = pos;
                    edit.anchor = None;
                    edit.message = format!(
                        "Enter Phred+33 quality for base {} (Tab switches tracks)",
                        pos + 1
                    );
                    return;
                }
                edit.saving = true;
                edit.message = "Saving…".into();
                let request = Request::Save {
                    file: self.file,
                    operation_id: edit.id,
                    draft: Arc::clone(draft),
                };
                self.send(request);
            }
            KeyCode::Tab | KeyCode::BackTab if draft.quality.is_some() => {
                edit.quality = !edit.quality;
                edit.anchor = None;
                if edit.quality
                    && let Some(pos) = draft
                        .quality
                        .as_ref()
                        .and_then(|q| q.iter().position(|&b| b == 0))
                {
                    edit.cursor = pos;
                }
                edit.message.clear();
            }
            KeyCode::Left => edit.move_to(edit.cursor.saturating_sub(1), shift),
            KeyCode::Right => edit.move_to((edit.cursor + 1).min(len), shift),
            KeyCode::Up => edit.move_to(edit.cursor.saturating_sub(width), shift),
            KeyCode::Down => edit.move_to(edit.cursor.saturating_add(width).min(len), shift),
            KeyCode::PageUp => edit.move_to(edit.cursor.saturating_sub(step), shift),
            KeyCode::PageDown => edit.move_to(edit.cursor.saturating_add(step).min(len), shift),
            KeyCode::Home => edit.move_to(0, shift),
            KeyCode::End => edit.move_to(len, shift),
            KeyCode::Backspace | KeyCode::Delete if !edit.quality => {
                let mut range = edit.selected();
                if range.is_empty() {
                    if key.code == KeyCode::Backspace {
                        range.start = range.start.saturating_sub(1);
                    } else {
                        range.end = (range.end + 1).min(len);
                    }
                }
                if !range.is_empty() {
                    edit.replace(range, &[]);
                }
            }
            KeyCode::Char(c)
                if !key
                    .modifiers
                    .intersects(KeyModifiers::CONTROL | KeyModifiers::ALT) =>
            {
                edit.insert(c.encode_utf8(&mut [0; 4]));
            }
            _ => {}
        }
    }
}

impl Editor {
    pub(super) fn load(&mut self, draft: Draft) {
        self.cursor = self.cursor.min(draft.bases.len());
        self.anchor = self.anchor.map(|a| a.min(draft.bases.len()));
        self.draft = Some(Arc::new(draft));
        self.message.clear();
    }
    fn selected(&self) -> Range<usize> {
        let a = self.anchor.unwrap_or(self.cursor);
        self.cursor.min(a)..self.cursor.max(a)
    }
    fn move_to(&mut self, next: usize, select: bool) {
        self.anchor = select.then_some(self.anchor.unwrap_or(self.cursor));
        self.cursor = next;
    }
    fn replace(&mut self, range: Range<usize>, bytes: &[u8]) {
        let draft = Arc::make_mut(self.draft.as_mut().unwrap());
        if let Some(q) = &mut draft.quality {
            q.splice(range.clone(), std::iter::repeat_n(0, bytes.len()));
        }
        self.cursor = range.start + bytes.len();
        draft.bases.splice(range, bytes.iter().copied());
        self.anchor = None;
        self.changed = true;
        self.message.clear();
    }
    pub(super) fn insert(&mut self, text: &str) {
        if self.draft.is_none() {
            self.pending.push(PendingInput::Paste(text.to_owned()));
            return;
        }
        if self.saving {
            return;
        }
        self.discard = false;
        let bytes: Vec<u8> = text
            .bytes()
            .filter(|b| {
                if self.quality {
                    !matches!(b, b'\r' | b'\n')
                } else {
                    !b.is_ascii_whitespace()
                }
            })
            .collect();
        if bytes.is_empty() {
            return;
        }
        let range = self.selected();
        if self.quality {
            let draft = Arc::make_mut(self.draft.as_mut().unwrap());
            let q = draft.quality.as_mut().unwrap();
            if bytes.iter().any(|b| !(33..=126).contains(b)) || bytes.len() > q.len() - range.start
            {
                self.message =
                    "Quality input needs printable Phred+33 characters fitting the record".into();
                return;
            }
            self.cursor = range.start + bytes.len();
            q[range.start..self.cursor].copy_from_slice(&bytes);
            self.anchor = None;
            self.changed = true;
            self.message.clear();
        } else if bytes.iter().all(|&b| is_residue(b)) {
            self.replace(range, &bytes);
        } else {
            self.message = "Paste sequence letters or - . * without headers".into();
        }
    }
    fn width(area: Rect, length: usize) -> usize {
        usize::from(area.width.saturating_sub(2))
            .saturating_sub(length.saturating_add(1).to_string().len() + 2)
            .max(1)
    }
    fn height(area: Rect, quality: bool) -> usize {
        (usize::from(area.height.saturating_sub(2)) / if quality { 2 } else { 1 }).max(1)
    }
    pub(super) fn draw(
        &self,
        frame: &mut Frame,
        area: Rect,
        theme: &Theme,
        palette: &ContentPalette,
    ) {
        let block = Block::default().borders(Borders::ALL).title(format!(
            " Edit {} · {}{} ",
            self.name,
            if self.quality { "Quality" } else { "Sequence" },
            if self.changed { " *" } else { "" }
        ));
        let inner = block.inner(area);
        frame.render_widget(block, area);
        let Some(draft) = &self.draft else {
            return;
        };
        let width = Self::width(area, draft.bases.len());
        let digits = draft.bases.len().saturating_add(1).to_string().len();
        let height = Self::height(area, draft.quality.is_some());
        let start = (self.cursor / width).saturating_sub(height - 1) * width;
        let selected = self.selected();
        let mut lines = Vec::new();
        for row in 0..height {
            let begin = start + row * width;
            if begin > draft.bases.len() {
                break;
            }
            for (quality, values) in [(false, Some(&draft.bases)), (true, draft.quality.as_ref())] {
                let Some(values) = values else {
                    continue;
                };
                let prefix = if quality {
                    format!("{:>digits$}  ", "Q")
                } else {
                    format!("{:>digits$}  ", begin + 1)
                };
                let mut spans = vec![Span::styled(prefix, Style::default().fg(theme.muted))];
                for pos in begin..(begin + width).min(values.len() + 1) {
                    let symbol = match values.get(pos) {
                        Some(0) => '·',
                        Some(&b) => char::from(b),
                        None => ' ',
                    };
                    let color = match values.get(pos) {
                        Some(0) | None => theme.muted,
                        Some(&b) if quality => palette.quality_color(b.saturating_sub(33)),
                        Some(&b) if self.seq_type == SeqType::Protein => {
                            palette.amino_acid_color(b)
                        }
                        Some(&b) => palette.nucleotide_color(b),
                    };
                    let mut style = Style::default().fg(color);
                    if selected.contains(&pos) {
                        style = style.bg(theme.selection);
                    }
                    if self.quality == quality && pos == self.cursor {
                        style = style.add_modifier(Modifier::REVERSED);
                    }
                    spans.push(Span::styled(symbol.to_string(), style));
                }
                lines.push(Line::from(spans));
            }
        }
        frame.render_widget(Paragraph::new(lines), inner);
    }
    pub(super) fn status(&self) -> Status {
        let fastq = self.draft.as_ref().is_some_and(|d| d.quality.is_some());
        let message = if self.message.is_empty() {
            format!(
                "EDIT · {} · cursor {} · length {}",
                self.name,
                self.cursor + 1,
                self.draft.as_ref().map_or(0, |d| d.bases.len())
            )
        } else {
            self.message.clone()
        };
        Status {
            message,
            hints: format!(
                "Type/paste  Shift+arrows select  {}Ctrl+S save  Esc cancel",
                if fastq { "Tab bases/quality  " } else { "" }
            ),
        }
    }
}
