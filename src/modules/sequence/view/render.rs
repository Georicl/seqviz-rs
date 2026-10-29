//! Draw sequence document panes and derive status text from already loaded snapshots.
use super::{Focus, Page, SequenceView};
use crate::modules::sequence::data::{Format, SeqType};
use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::style::Style;
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Paragraph};

/// Draw the record list, metadata, sequence widget and optional information-page overlay.
pub(crate) fn draw(frame: &mut Frame, app: &SequenceView) {
    let body = app.hits.area;
    let left = app.hits.left;
    let right = app.hits.right;
    let border = app.theme.border;
    let focus = app.theme.focus;
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
                        Style::default().bg(app.theme.selection).fg(focus)
                    } else {
                        Style::default().fg(app.theme.foreground)
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
                    Style::default().fg(app.theme.muted),
                )];
                let bins = &quality.overview;
                let cells = inner.width.saturating_sub(10) as usize;
                for i in 0..cells.min(bins.len()) {
                    let score = bins[i * bins.len() / cells.min(bins.len()).max(1)];
                    let glyphs = ['▁', '▂', '▃', '▄', '▅', '▆', '▇', '█'];
                    let glyph = glyphs[((score / 45.0 * 7.0) as usize).min(7)];
                    spans.push(Span::styled(
                        glyph.to_string(),
                        Style::default().fg(app.config.palette.quality_color(score as u8)),
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
        if let Some(snapshot) = app.tab().snapshot.as_deref() {
            use crate::modules::sequence::widgets::sequence::{self, Props};
            sequence::draw(
                frame,
                Props {
                    snapshot,
                    theme: &app.theme,
                    palette: &app.config.palette,
                    area: app.hits.sequence,
                    sequence_rows: app.hits.sequence_rows,
                    bases_per_line: app.hits.bases_per_line,
                    gutter: app.hits.gutter,
                    stride: app.hits.stride,
                    record: app.tab().record,
                    start: app.tab().start,
                    cursor: app.tab().cursor,
                    selection: app.tab().selection,
                    focused: app.focus == Focus::Sequence,
                },
            );
        }
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

    if let Some(page) = app.page {
        let title = match page {
            Page::Help => "Help",
            Page::File => "File",
            Page::Record => "Current record",
            Page::Statistics => "Statistics",
        };
        crate::widgets::text_panel::draw(
            frame,
            body,
            &format!("{title} · Tab changes page · q returns"),
            &app.info,
            &app.theme,
        );
    }
}

/// Combine operation messages or scan progress with the format-appropriate keyboard hints.
pub(crate) fn status(app: &SequenceView) -> crate::modules::view::Status {
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
    crate::modules::view::Status {
        message: status,
        hints: hints.into(),
    }
}
