use super::app::{App, Focus, Input, Page};
use crate::data::{Format, SeqType};
use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Clear, Paragraph};
use unicode_width::UnicodeWidthStr;

pub(super) fn draw(frame: &mut Frame, app: &App) {
    let area = app.hits.area;
    let theme = &app.config.theme;
    frame.render_widget(
        Block::default().style(Style::default().bg(theme.background).fg(theme.foreground)),
        area,
    );
    if app.hits.too_small {
        frame.render_widget(
            Paragraph::new("Resize terminal to at least 30 × 8. q quits."),
            area,
        );
        return;
    }
    draw_tabs(frame, app, Rect::new(area.x, area.y, area.width, 1));
    let body = app.hits.body;
    let left = app.hits.left;
    let right = app.hits.right;
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
    let inner = app.hits.sequence_inner;
    let details = app.hits.details;
    frame.render_widget(sequence_block, right);
    let has_quality = app
        .tab()
        .snapshot
        .as_ref()
        .and_then(|s| s.record.as_ref())
        .is_some_and(|r| r.has_quality);

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

    let footer = app.hits.footer;
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

fn draw_tabs(frame: &mut Frame, app: &App, area: Rect) {
    frame.render_widget(
        Paragraph::new(" sq ").style(
            Style::default()
                .fg(app.config.theme.focus)
                .add_modifier(Modifier::BOLD),
        ),
        Rect::new(area.x, area.y, 4, 1),
    );
    for &(id, rect) in &app.hits.tabs {
        let name = app.tabs[id]
            .path
            .file_name()
            .unwrap_or_default()
            .to_string_lossy();
        let label = format!(" {}:{} ", id + 1, name);
        let style = if id == app.active {
            Style::default()
                .fg(app.config.theme.focus)
                .bg(app.config.theme.selection)
                .add_modifier(Modifier::BOLD)
        } else {
            Style::default().fg(app.config.theme.muted)
        };
        frame.render_widget(Paragraph::new(label).style(style), rect);
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

fn draw_info(frame: &mut Frame, app: &App, area: Rect) {
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
    let inner = app.hits.info;
    frame.render_widget(Clear, area);
    frame.render_widget(block, area);
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
