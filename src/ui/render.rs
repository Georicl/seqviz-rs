use super::*;
use crate::data::{Format, SeqType};
use ratatui::Frame;
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Clear, Paragraph};
use unicode_width::{UnicodeWidthChar, UnicodeWidthStr};

pub(super) fn draw(frame: &mut Frame, app: &mut App) {
    let area = frame.area();
    let theme = &app.config.theme;
    frame.render_widget(
        Block::default().style(Style::default().bg(theme.background).fg(theme.foreground)),
        area,
    );
    if area.width < 30 || area.height < 8 {
        frame.render_widget(
            Paragraph::new("Resize terminal to at least 30 × 8. q quits."),
            area,
        );
        return;
    }
    draw_tabs(frame, app, Rect::new(area.x, area.y, area.width, 1));
    let body = Rect::new(area.x, area.y + 1, area.width, area.height - 3);
    let left_width = (area.width / 4).clamp(12, 32);
    let left = Rect::new(body.x, body.y, left_width, body.height);
    let right = Rect::new(left.right(), body.y, body.width - left_width, body.height);
    let border = app.config.theme.border;
    let focus = app.config.theme.focus;
    let discovered = app
        .tab()
        .snapshot
        .as_ref()
        .map(|s| {
            format!(
                "{}{}",
                s.progress.records,
                if s.progress.complete { "" } else { "+" }
            )
        })
        .unwrap_or_else(|| "…".into());
    let list_block = Block::default()
        .borders(Borders::ALL)
        .title(format!(" Records · {discovered} "))
        .border_style(Style::default().fg(if app.focus == Focus::Records {
            focus
        } else {
            border
        }));
    app.hits.list = list_block.inner(left);
    frame.render_widget(list_block, left);
    if let Some(snapshot) = &app
        .tab()
        .snapshot
        .as_ref()
        .filter(|s| s.request.list_start == app.tab().list_start)
    {
        let lines: Vec<Line<'_>> = snapshot
            .records
            .iter()
            .take(app.hits.list.height as usize)
            .map(|record| {
                Line::styled(
                    record.name.as_str(),
                    if record.id == app.tab().record {
                        Style::default().bg(app.config.theme.selection).fg(focus)
                    } else {
                        Style::default().fg(app.config.theme.foreground)
                    },
                )
            })
            .collect();
        frame.render_widget(Paragraph::new(lines), app.hits.list);
    } else {
        frame.render_widget(Paragraph::new("Opening…"), app.hits.list);
    }

    let sequence_block = Block::default()
        .borders(Borders::ALL)
        .title(" Sequence ")
        .border_style(Style::default().fg(if app.focus == Focus::Sequence {
            focus
        } else {
            border
        }));
    let inner = sequence_block.inner(right);
    frame.render_widget(sequence_block, right);
    let snapshot = app.tab().snapshot.as_ref();
    let meta = snapshot.and_then(|s| s.record.as_ref());
    let has_quality = meta.is_some_and(|r| r.has_quality);
    let stride = if has_quality && app.config.show_quality {
        2
    } else {
        1
    };
    let details = inner.height.min(4);
    let sequence_rect = Rect::new(
        inner.x,
        inner.y + details,
        inner.width,
        inner.height - details,
    );
    let max_position = meta.map_or(1, |r| r.length.max(app.tab().start + 1));
    let gutter =
        (max_position.to_string().len() + 2).min(inner.width.saturating_sub(1) as usize) as u16;
    let bases = sequence_rect.width.saturating_sub(gutter).max(1) as usize;
    let rows = sequence_rect.height as usize / stride as usize;
    if app.hits.bases_per_line != bases || app.hits.sequence_rows != rows {
        app.dirty = true;
    }
    app.hits.sequence = sequence_rect;
    app.hits.bases_per_line = bases;
    app.hits.sequence_rows = rows;
    app.hits.stride = stride;
    app.hits.gutter = gutter;

    if let Some(meta) = app.tab().snapshot.as_ref().and_then(|s| s.record.as_ref()) {
        let unit = if meta.seq_type == SeqType::Protein {
            "aa"
        } else {
            "bp"
        };
        let kind = match meta.seq_type {
            SeqType::Dna => "Nucleotide",
            SeqType::Protein => "Protein",
            SeqType::Unknown => "Unknown",
        };
        let length = if meta.complete {
            format!("{} {unit}", meta.length)
        } else {
            format!("≥{} {unit} · reading", meta.length)
        };
        let gc = if meta.seq_type != SeqType::Dna
            || (meta.complete && (!meta.has_sequence || meta.length == 0))
        {
            "n/a".into()
        } else {
            meta.gc
                .map(|v| format!("{v:.2}%"))
                .unwrap_or_else(|| "calculating".into())
        };
        let lines = vec![
            Line::styled(meta.header.clone(), Style::default().fg(focus)),
            Line::from(format!("{kind} · {length} · GC {gc}")),
        ];
        frame.render_widget(
            Paragraph::new(lines),
            Rect::new(inner.x, inner.y, inner.width, details.min(2)),
        );
        if details >= 3 {
            let quality_line = if let Some(quality) = &meta.quality {
                let mut spans = vec![Span::styled(
                    format!("Q {:.1} │", quality.mean),
                    Style::default().fg(app.config.theme.muted),
                )];
                let bins = &quality.overview;
                let cells = inner.width.saturating_sub(10) as usize;
                for i in 0..cells.min(bins.len()) {
                    let score = bins[i * bins.len() / cells.min(bins.len()).max(1)];
                    let glyphs = ['▁', '▂', '▃', '▄', '▅', '▆', '▇', '█'];
                    let glyph = glyphs[((score / 45.0 * 7.0) as usize).min(7)];
                    spans.push(Span::styled(
                        glyph.to_string(),
                        Style::default().fg(app.config.theme.quality_color(score as u8)),
                    ));
                }
                Line::from(spans)
            } else if has_quality {
                Line::from("Quality overview: calculating…")
            } else if app
                .tab()
                .snapshot
                .as_ref()
                .is_some_and(|s| s.format != Format::Fasta)
            {
                Line::from("Quality: missing")
            } else {
                Line::from("F3 full header and record properties")
            };
            frame.render_widget(
                Paragraph::new(quality_line),
                Rect::new(inner.x, inner.y + 2, inner.width, 1),
            );
        }
        draw_sequence(frame, app);
    } else {
        let empty = app
            .tab()
            .snapshot
            .as_ref()
            .is_some_and(|s| s.progress.complete && s.progress.records == 0);
        frame.render_widget(
            Paragraph::new(if empty {
                "No records"
            } else {
                "Reading records…"
            }),
            inner,
        );
    }

    let footer = Rect::new(area.x, area.bottom() - 2, area.width, 2);
    draw_footer(frame, app, footer);
    if app.page.is_some() {
        draw_info(frame, app, body);
    }
    if let Some(input) = &app.input {
        let (label, text) = match input {
            Input::Search(text) => ("Name contains: ", text),
            Input::Position(text) => ("Position (1-based): ", text),
        };
        frame.render_widget(Clear, Rect::new(footer.x, footer.y, footer.width, 1));
        frame.render_widget(
            Paragraph::new(format!("{label}{text}")).style(Style::default().fg(focus)),
            Rect::new(footer.x, footer.y, footer.width, 1),
        );
        let cursor =
            (label.width() + text.width()).min(footer.width.saturating_sub(1) as usize) as u16;
        frame.set_cursor_position((footer.x + cursor, footer.y));
    }
}

fn draw_tabs(frame: &mut Frame, app: &mut App, area: Rect) {
    app.hits.tabs.clear();
    frame.render_widget(
        Paragraph::new(" sq ").style(
            Style::default()
                .fg(app.config.theme.focus)
                .add_modifier(Modifier::BOLD),
        ),
        Rect::new(area.x, area.y, 4, 1),
    );
    let open: Vec<usize> = app
        .tabs
        .iter()
        .enumerate()
        .filter(|(_, t)| !t.closed)
        .map(|(id, _)| id)
        .collect();
    let active_index = open.iter().position(|id| *id == app.active).unwrap_or(0);
    let visible = ((area.width.saturating_sub(4)) / 20).max(1) as usize;
    let start = active_index.saturating_sub(visible.saturating_sub(1));
    let mut x = area.x + 4;
    for &id in open.iter().skip(start).take(visible) {
        let name = app.tabs[id]
            .path
            .file_name()
            .unwrap_or_default()
            .to_string_lossy();
        let label = format!(" {}:{} ", id + 1, name);
        let width = 20.min(area.right().saturating_sub(x));
        let rect = Rect::new(x, area.y, width, 1);
        let style = if id == app.active {
            Style::default()
                .fg(app.config.theme.focus)
                .bg(app.config.theme.selection)
                .add_modifier(Modifier::BOLD)
        } else {
            Style::default().fg(app.config.theme.muted)
        };
        frame.render_widget(Paragraph::new(label).style(style), rect);
        app.hits.tabs.push((id, rect));
        x += width;
    }
}

fn draw_sequence(frame: &mut Frame, app: &App) {
    let Some(snapshot) = &app.tab().snapshot else {
        return;
    };
    let Some(meta) = &snapshot.record else {
        return;
    };
    let rect = app.hits.sequence;
    if !meta.has_sequence {
        frame.render_widget(Paragraph::new("Sequence: missing"), rect);
        return;
    }
    if snapshot.window.start != app.tab().start || snapshot.request.record != app.tab().record {
        return;
    }
    for row in 0..app.hits.sequence_rows {
        let offset = row * app.hits.bases_per_line;
        if offset >= snapshot.window.sequence.len() {
            break;
        }
        let length = app
            .hits
            .bases_per_line
            .min(snapshot.window.sequence.len() - offset);
        let position = app.tab().start + offset as u64;
        let y = rect.y + row as u16 * app.hits.stride;
        let number = format!(
            "{:>width$} ",
            position + 1,
            width = app.hits.gutter.saturating_sub(1) as usize
        );
        frame.render_widget(
            Paragraph::new(number).style(Style::default().fg(app.config.theme.muted)),
            Rect::new(rect.x, y, app.hits.gutter, 1),
        );
        let mut spans = Vec::with_capacity(length);
        let mut quality = Vec::with_capacity(length);
        for index in offset..offset + length {
            let byte = snapshot.window.sequence[index];
            let score = snapshot.window.quality.get(index).copied();
            let color = if let Some(q) = score {
                app.config.theme.quality_color(q.saturating_sub(33))
            } else if meta.seq_type == SeqType::Protein {
                app.config.theme.amino_acid_color(byte)
            } else {
                app.config.theme.nucleotide_color(byte)
            };
            let mut style = Style::default().fg(color);
            let at = app.tab().start + index as u64;
            if app
                .tab()
                .selection
                .is_some_and(|(a, b)| at >= a.min(b) && at <= a.max(b))
            {
                style = style.bg(app.config.theme.selection);
            }
            if app.focus == Focus::Sequence && at == app.tab().cursor {
                style = style.add_modifier(Modifier::UNDERLINED | Modifier::BOLD);
            }
            spans.push(Span::styled(char::from(byte).to_string(), style));
            if let Some(q) = score {
                quality.push(Span::styled(
                    char::from(q).to_string(),
                    Style::default().fg(app.config.theme.muted),
                ));
            }
        }
        frame.render_widget(
            Paragraph::new(Line::from(spans)),
            Rect::new(
                rect.x + app.hits.gutter,
                y,
                rect.width.saturating_sub(app.hits.gutter),
                1,
            ),
        );
        if app.hits.stride == 2 {
            frame.render_widget(
                Paragraph::new(Line::from(quality)),
                Rect::new(
                    rect.x + app.hits.gutter,
                    y + 1,
                    rect.width.saturating_sub(app.hits.gutter),
                    1,
                ),
            );
        }
    }
}

fn draw_footer(frame: &mut Frame, app: &App, area: Rect) {
    let status = if !app.tab().message.is_empty() {
        app.tab().message.clone()
    } else if let Some(snapshot) = &app.tab().snapshot {
        format!(
            "{} · {} · record {} / {}{} · position {} · {}{}",
            snapshot.format.name(),
            snapshot.compression,
            app.tab().record + 1,
            snapshot.progress.records,
            if snapshot.progress.complete { "" } else { "+" },
            app.tab().cursor + 1,
            if snapshot.progress.complete {
                "Indexed"
            } else if snapshot.paused {
                "Paused"
            } else {
                "Indexing"
            },
            if snapshot.progress.errors > 0 {
                format!(" · {} errors", snapshot.progress.errors)
            } else {
                String::new()
            }
        )
    } else {
        "Opening…".into()
    };
    let hints = if app.can_copy() {
        "? help  / name  : position  y copy  Tab file  x close  F4 stats  q quit"
    } else {
        "? help  / name  : position  Tab file  x close  F4 stats  q quit"
    };
    frame.render_widget(
        Paragraph::new(vec![
            Line::from(status),
            Line::styled(hints, Style::default().fg(app.config.theme.muted)),
        ]),
        area,
    );
}

fn info_text(app: &App) -> Vec<String> {
    let Some(page) = app.page else {
        return Vec::new();
    };
    if page == Page::Help {
        return [
            "BROWSE",
            "↑/↓ or j/k       Move records or sequence cursor",
            "←/→ or h/l       Move sequence cursor; Enter focuses sequence",
            "Ctrl+h / Ctrl+l  Focus record list / sequence",
            "[ / ]           Previous / next record",
            "PgUp / PgDn     Page through focused pane",
            "Home / End      First / last position or record",
            "Mouse wheel     Scroll under the pointer",
            "",
            "FIND",
            "/               Partial record-name search",
            "n               Next matching record (wraps)",
            ":               Jump to a 1-based position in this sequence",
            "",
            "SELECT AND COPY",
            "Drag / v        Select sequence; drag at an edge to scroll",
            "y               Copy selected sequence, or full record from list",
            "Info panel      Drag-select text, then y; no selection copies page",
            "FASTA record    Full header + sequence wrapped at 60 characters",
            "FASTQ record    Original header and separator + sequence and quality",
            "SAM / BAM       All copying is disabled",
            "",
            "FILES AND PANELS",
            "Tab / Shift+Tab Next / previous file",
            "x               Close current file",
            "F1 / ?          Help",
            "F2 / F3 / F4    File / record / statistics",
            "Panel Tab / ←→  Change information page",
            "Q               Show / hide aligned quality characters",
            "Esc             Cancel search/copy; pause background indexing",
            "q               Close information panel, or quit browser",
            "Ctrl+c          Quit browser",
            "",
            "Statistics run when their page is opened. Unknown totals show +.",
            "Sequence positions and selections survive terminal resizing.",
            "Sources are read-only. Indexes are reused while the source is unchanged.",
            "SAM/BAM statistics count alignment records, including secondary and",
            "supplementary alignments. Missing SEQ is excluded from sequence metrics.",
            "GC = G+C divided by all sequence characters; protein GC is not applicable.",
        ]
        .into_iter()
        .map(String::from)
        .collect();
    }
    let mut lines = Vec::new();
    if page == Page::File {
        lines.push(format!("Path: {}", app.tab().path.display()));
        if let Ok(meta) = std::fs::metadata(&app.tab().path) {
            lines.push(format!("File size: {} bytes", meta.len()));
        }
        lines.push(format!(
            "Index cache: {}",
            worker::cache_directory().display()
        ));
    }
    let Some(snapshot) = &app.tab().snapshot else {
        lines.push("Opening file…".into());
        return lines;
    };
    match page {
        Page::File => {
            lines.extend([
                format!("Format: {}", snapshot.format.name()),
                format!("Compression: {}", snapshot.compression),
                format!("Discovered records: {}", snapshot.progress.records),
                format!(
                    "Scanned (decoded) bytes: {}",
                    snapshot.progress.scanned_bytes
                ),
                format!(
                    "Index: {}",
                    if snapshot.progress.complete {
                        "complete"
                    } else if snapshot.paused {
                        "paused"
                    } else {
                        "building"
                    }
                ),
                format!("Read errors: {}", snapshot.progress.errors),
            ]);
            if let Some(error) = &snapshot.progress.error {
                lines.push(format!("Last error: {error}"));
            } else if let Some(error) = &snapshot.progress.last_error {
                lines.push(format!("Last recoverable error: {error}"));
            }
        }
        Page::Record => {
            if let Some(record) = &snapshot.record {
                lines.extend([
                    format!("Name: {}", record.name),
                    "Full header:".into(),
                    record.header.clone(),
                    String::new(),
                    format!("Type: {:?}", record.seq_type),
                    format!(
                        "Length: {}{} {}",
                        record.length,
                        if record.complete { "" } else { "+ (reading)" },
                        if record.seq_type == SeqType::Protein {
                            "aa"
                        } else {
                            "bp"
                        }
                    ),
                    format!(
                        "GC: {}",
                        if record.seq_type != SeqType::Dna
                            || (record.complete && (!record.has_sequence || record.length == 0))
                        {
                            "not applicable".into()
                        } else {
                            record
                                .gc
                                .map(|v| format!("{v:.2}%"))
                                .unwrap_or_else(|| "calculating…".into())
                        }
                    ),
                    format!(
                        "Sequence: {}",
                        if record.has_sequence {
                            "available"
                        } else {
                            "missing"
                        }
                    ),
                    format!(
                        "Quality: {}",
                        if record.has_quality {
                            "Phred+33"
                        } else {
                            "missing"
                        }
                    ),
                ]);
                if let Some(quality) = &record.quality {
                    lines.extend([
                        format!("Mean quality: {:.2}", quality.mean),
                        format!("Min / max quality: {} / {}", quality.min, quality.max),
                        format!("Q30: {:.2}%", quality.q30),
                    ]);
                }
                for (name, value) in &record.fields {
                    lines.push(format!("{name}: {value}"));
                }
            } else {
                lines.push("Reading current record…".into());
            }
        }
        Page::Statistics => {
            if let Some(stats) = &snapshot.stats {
                let ready = stats.complete || stats.errors > 0 || snapshot.progress.error.is_some();
                lines.push(format!(
                    "Status: {}",
                    if stats.complete {
                        "complete"
                    } else if stats.errors > 0 || snapshot.progress.error.is_some() {
                        "partial: scan contains errors"
                    } else if snapshot.paused {
                        "paused"
                    } else {
                        "calculating…"
                    }
                ));
                lines.push(format!("Discovered records: {}", snapshot.progress.records));
                lines.push(format!(
                    "Records with sequence in statistics: {}",
                    stats.sequence_records
                ));
                lines.push(format!("Errors: {}", stats.errors));
                if ready {
                    let unit = if stats.protein { "aa" } else { "bp" };
                    lines.extend([
                        format!("Total length: {} {unit}", stats.total_length),
                        format!("Minimum length: {}", opt(stats.min_length)),
                        format!("Maximum length: {}", opt(stats.max_length)),
                        format!(
                            "Mean length: {}",
                            stats
                                .mean_length
                                .map(|v| format!("{v:.2}"))
                                .unwrap_or_else(|| "n/a".into())
                        ),
                    ]);
                    if snapshot.format.can_copy() {
                        lines.push(format!("N50: {}", opt(stats.n50)));
                    }
                    lines.push(format!(
                        "GC: {}",
                        stats
                            .gc
                            .map(|v| format!("{v:.2}%"))
                            .unwrap_or_else(|| "not applicable".into())
                    ));
                }
                if matches!(snapshot.format, Format::Sam | Format::Bam) {
                    lines.push("Every alignment is counted separately. Missing SEQ is excluded from length and GC; no deduplication.".into());
                }
            } else {
                lines.push("Calculating…".into());
            }
        }
        Page::Help => {}
    }
    lines
}

fn opt(value: Option<u64>) -> String {
    value.map(|v| v.to_string()).unwrap_or_else(|| "n/a".into())
}

fn draw_info(frame: &mut Frame, app: &mut App, area: Rect) {
    let page = app.page.unwrap();
    let title = match page {
        Page::Help => "Help",
        Page::File => "File",
        Page::Record => "Current record",
        Page::Statistics => "Statistics",
    };
    let block = Block::default()
        .borders(Borders::ALL)
        .title(format!(" {title} · Tab changes page · q returns "))
        .border_style(Style::default().fg(app.config.theme.focus))
        .style(Style::default().bg(app.config.theme.background));
    let inner = block.inner(area);
    app.hits.info = inner;
    frame.render_widget(Clear, area);
    frame.render_widget(block, area);
    app.info_lines = info_text(app)
        .into_iter()
        .flat_map(|line| wrap_line(&line, inner.width.max(1) as usize))
        .collect();
    app.info_scroll = app
        .info_scroll
        .min(app.info_lines.len().saturating_sub(inner.height as usize));
    let selected = app
        .info_selection
        .map(|(a, b)| if a <= b { (a, b) } else { (b, a) });
    let lines: Vec<Line<'_>> = app
        .info_lines
        .iter()
        .enumerate()
        .skip(app.info_scroll)
        .take(inner.height as usize)
        .map(|(row, line)| {
            let spans: Vec<Span<'_>> = line
                .chars()
                .enumerate()
                .map(|(column, c)| {
                    let style = if selected
                        .is_some_and(|(a, b)| (row, column) >= a && (row, column) <= b)
                    {
                        Style::default().bg(app.config.theme.selection)
                    } else {
                        Style::default()
                    };
                    Span::styled(c.to_string(), style)
                })
                .collect();
            Line::from(spans)
        })
        .collect();
    frame.render_widget(Paragraph::new(lines), inner);
}

fn wrap_line(line: &str, width: usize) -> Vec<String> {
    let mut lines = Vec::new();
    let mut row = String::new();
    let mut columns = 0;
    for c in line.chars() {
        let count = c.width().unwrap_or(0);
        if columns + count > width && !row.is_empty() {
            lines.push(std::mem::take(&mut row));
            columns = 0;
        }
        if !c.is_control() {
            row.push(c);
            columns += count;
        }
    }
    lines.push(row);
    lines
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn information_wrapping_respects_display_columns() {
        assert_eq!(wrap_line("ab界cd", 4), vec!["ab界", "cd"]);
        assert_eq!(wrap_line("", 4), vec![""]);
        assert!(
            wrap_line("abc\x1bdef", 4)
                .iter()
                .all(|line| !line.contains('\x1b'))
        );
    }
}
