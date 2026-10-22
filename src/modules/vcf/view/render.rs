use super::text::{grapheme_width, tab_stops, walk_graphemes};
use super::{Detail, Focus, Input, Overlay, VcfView, layout};
use crate::modules::vcf::palette;
use crate::widgets::text_panel;
use ratatui::{
    Frame,
    layout::Rect,
    style::{Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, Clear, Paragraph, Wrap},
};
use unicode_width::UnicodeWidthStr;

pub(super) fn draw(frame: &mut Frame, view: &VcfView) {
    if view.layout.area.width == 0 || view.layout.area.height == 0 {
        return;
    }
    draw_header(frame, view);
    draw_table(frame, view);
    draw_details(frame, view);
    if let Some(overlay) = view.overlay {
        let title = match overlay {
            Overlay::Help => "VCF help",
            Overlay::File => "File",
            Overlay::Header => "Original VCF header",
        };
        text_panel::draw(
            frame,
            view.layout.area,
            &format!("{title} · q/Esc returns · y copies"),
            &view.info,
            &view.theme,
        );
    }
    if matches!(view.input, Some(Input::Contig(_))) {
        draw_picker(frame, view);
    }
}

fn draw_header(frame: &mut Frame, view: &VcfView) {
    let lines = if let Some(snapshot) = &view.snapshot {
        vec![
            Line::styled(
                format!(
                    "{} · {} samples · {}{} variants · m header",
                    snapshot.header.version,
                    snapshot.header.samples.len(),
                    snapshot.progress.records,
                    if snapshot.progress.complete { "" } else { "+" }
                ),
                Style::default().fg(view.theme.focus),
            ),
            Line::from(format!(
                "Contig: {} · c choose{}",
                view.contig.as_deref().unwrap_or("All records"),
                view.entry()
                    .map(|e| format!(" · {}:{}", e.record.chrom, e.record.pos))
                    .unwrap_or_default()
            )),
        ]
    } else if let Some(error) = &view.load_error {
        vec![Line::from("VCF error"), Line::from(error.as_str())]
    } else {
        vec![Line::from("Reading VCF header…")]
    };
    frame.render_widget(Paragraph::new(lines), view.layout.header);
}

fn columns(area: Rect) -> Vec<(usize, usize)> {
    let preferred = [8usize, 10, 10, 8, 14, 8, 7, 10];
    let available = usize::from(area.width);
    let mut columns = if available >= 43 {
        vec![(0, 8), (1, 10), (3, 8), (4, 14)]
    } else if available >= 24 {
        vec![(0, 5), (1, 10), (3, 3), (4, 3)]
    } else {
        vec![(0, 1), (1, 1), (3, 1), (4, 1)]
    };
    let mut remaining = available.saturating_sub(columns.iter().map(|(_, w)| w).sum::<usize>() + 3);
    if available >= 43 {
        for (index, width) in [(5, 4), (6, 4), (7, 6), (2, 5)] {
            if remaining > width {
                columns.push((index, width));
                remaining -= width + 1;
            }
        }
    }
    columns.sort_unstable_by_key(|&(index, _)| index);
    while remaining > 0 {
        let mut changed = false;
        for (index, width) in &mut columns {
            if remaining > 0 && *width < preferred[*index] {
                *width += 1;
                remaining -= 1;
                changed = true;
            }
        }
        if !changed {
            break;
        }
    }
    columns
}

fn fit(spans: Vec<Span<'static>>, width: usize) -> Vec<Span<'static>> {
    if width == 0 {
        return Vec::new();
    }
    let source_width = spans
        .iter()
        .flat_map(|span| span.styled_graphemes(Style::default()))
        .map(|g| {
            if g.symbol.chars().any(char::is_control) {
                1
            } else {
                g.symbol.width()
            }
        })
        .sum::<usize>();
    let truncated = source_width > width;
    let limit = width.saturating_sub(usize::from(truncated));
    let mut fitted = Vec::new();
    let mut used = 0;
    for span in spans {
        let mut text = String::new();
        for grapheme in span.styled_graphemes(Style::default()) {
            let symbol = if grapheme.symbol.chars().any(char::is_control) {
                " "
            } else {
                grapheme.symbol
            };
            let columns = symbol.width();
            if used + columns > limit {
                break;
            }
            text.push_str(symbol);
            used += columns;
        }
        if !text.is_empty() {
            fitted.push(Span::styled(text, span.style));
        }
        if used >= limit {
            break;
        }
    }
    if truncated {
        if used < limit {
            fitted.push(Span::raw(" ".repeat(limit - used)));
        }
        fitted.push(Span::raw("…"));
        return fitted;
    }
    if used < width {
        fitted.push(Span::raw(" ".repeat(width - used)));
    }
    fitted
}

fn plain(text: impl Into<String>, style: Style) -> Vec<Span<'static>> {
    vec![Span::styled(text.into(), style)]
}

fn bases(text: &str, view: &VcfView) -> Vec<Span<'static>> {
    if text
        .bytes()
        .all(|b| b"ACGTUNRYWSMKBDHVacgtunrywsmkbdhv,.-".contains(&b))
    {
        text.split_inclusive(',')
            .flat_map(|part| {
                let mut spans = palette::nucleotides(part.trim_end_matches(','), &view.config);
                if part.ends_with(',') {
                    spans.push(Span::raw(","));
                }
                spans
            })
            .collect()
    } else {
        plain(text, Style::default().fg(view.theme.foreground))
    }
}

fn table_line(
    fields: Vec<Vec<Span<'static>>>,
    columns: &[(usize, usize)],
    selected: bool,
    view: &VcfView,
) -> Line<'static> {
    let mut spans = Vec::new();
    for (ordinal, &(index, width)) in columns.iter().enumerate() {
        if ordinal > 0 {
            spans.push(Span::raw(" "));
        }
        spans.extend(fit(fields[index].clone(), width));
    }
    if selected {
        for span in &mut spans {
            span.style = span.style.bg(view.theme.selection);
        }
    }
    Line::from(spans)
}

fn draw_table(frame: &mut Frame, view: &VcfView) {
    if view.layout.table.height == 0 {
        return;
    }
    let block = Block::default()
        .borders(Borders::ALL)
        .title(" Variants ")
        .border_style(Style::default().fg(if view.focus == Focus::Table {
            view.theme.focus
        } else {
            view.theme.border
        }));
    let inner = block.inner(view.layout.table);
    frame.render_widget(block, view.layout.table);
    if inner.height == 0 {
        return;
    }
    let columns = columns(inner);
    let style = Style::default()
        .fg(view.theme.muted)
        .add_modifier(Modifier::BOLD);
    let header = table_line(
        ["CHROM", "POS", "ID", "REF", "ALT", "TYPE", "QUAL", "FILTER"]
            .into_iter()
            .map(|s| plain(s, style))
            .collect(),
        &columns,
        false,
        view,
    );
    frame.render_widget(
        Paragraph::new(header),
        Rect::new(inner.x, inner.y, inner.width, 1),
    );
    let Some(snapshot) = view
        .snapshot
        .as_ref()
        .filter(|s| s.request.start == view.start && s.request.contig == view.contig)
    else {
        let message = view
            .load_error
            .as_ref()
            .map(|error| format!("Error: {error}"))
            .unwrap_or_else(|| "Reading records…".into());
        frame.render_widget(
            Paragraph::new(message).wrap(Wrap { trim: false }),
            view.layout.rows,
        );
        return;
    };
    if snapshot.rows.is_empty() {
        let message = if let Some(error) = &snapshot.progress.error {
            format!("Error: {error}")
        } else if snapshot.progress.complete {
            "No records".into()
        } else {
            "Reading records…".into()
        };
        frame.render_widget(
            Paragraph::new(message).wrap(Wrap { trim: false }),
            view.layout.rows,
        );
        return;
    }
    let normal = Style::default().fg(view.theme.foreground);
    let lines = snapshot
        .rows
        .iter()
        .take(view.layout.rows.height as usize)
        .map(|row| {
            table_line(
                vec![
                    plain(row.chrom.clone(), normal),
                    plain(row.pos.to_string(), normal),
                    plain(row.ids.clone(), normal),
                    bases(&row.reference, view),
                    bases(&row.alternates, view),
                    plain(row.kind.label(), normal),
                    plain(
                        row.qual
                            .map(|q| q.to_string())
                            .unwrap_or_else(|| ".".into()),
                        normal,
                    ),
                    plain(
                        row.filter.clone(),
                        Style::default().fg(palette::filter_color(&row.filter, &view.config)),
                    ),
                ],
                &columns,
                row.ordinal == view.selected,
                view,
            )
        })
        .collect::<Vec<_>>();
    frame.render_widget(Paragraph::new(lines), view.layout.rows);
}

fn detail_spans<'a>(line: &'a str, view: &VcfView) -> Vec<Span<'a>> {
    let normal = Style::default().fg(view.theme.foreground);
    if view.detail == Detail::Raw {
        return vec![Span::styled(line, normal)];
    }
    let mut result = Vec::new();
    let formats = view
        .entry()
        .map(|e| e.record.format.as_slice())
        .unwrap_or(&[]);
    for (index, value) in line.split('\t').enumerate() {
        if index > 0 {
            result.push(Span::raw("\t"));
        }
        if view.detail == Detail::Samples
            && index > 0
            && formats.get(index - 1).is_some_and(|field| field == "GT")
        {
            result.extend(palette::genotype(value, &view.config, &view.theme));
        } else {
            result.push(Span::styled(
                value,
                if value == "." {
                    Style::default().fg(view.config.missing_color)
                } else {
                    normal
                },
            ));
        }
    }
    result
}

fn clip_spans(
    source: Vec<Span<'_>>,
    horizontal: usize,
    width: usize,
    stops: &[usize],
    style_at: impl Fn(usize, Style) -> Style,
) -> (Vec<Span<'static>>, usize, usize) {
    let mut used = 0usize;
    let mut source_column = 0usize;
    let mut rendered = 0usize;
    let right = horizontal.saturating_add(width);
    let mut spans = Vec::new();
    for span in source {
        let complete = walk_graphemes(&span.content, |symbol| {
            if used >= right {
                return false;
            }
            let columns = grapheme_width(symbol, used, stops);
            let next = used + columns;
            let visible = next.min(right).saturating_sub(used.max(horizontal));
            if visible > 0 || (columns == 0 && used > horizontal && used <= right) {
                // A clipped wide glyph keeps its occupied columns without drawing half a glyph.
                let text = if symbol == "\t" || visible < columns {
                    " ".repeat(visible)
                } else if symbol.chars().any(char::is_control) {
                    "�".into()
                } else {
                    symbol.to_owned()
                };
                spans.push(Span::styled(text, style_at(source_column, span.style)));
                rendered += visible;
            }
            used = next;
            source_column += symbol.chars().count();
            true
        });
        if !complete {
            break;
        }
    }
    (spans, rendered, used)
}

fn visible_detail(line: &str, row: usize, view: &VcfView, stops: &[usize]) -> Vec<Span<'static>> {
    let selection = view
        .selection
        .map(|(a, b)| if a <= b { (a, b) } else { (b, a) });
    let width = view.layout.detail_body.width as usize;
    let (mut spans, rendered, source_width) = clip_spans(
        detail_spans(line, view),
        view.horizontal,
        width,
        stops,
        |column, style| {
            if selection.is_some_and(|(a, b)| (row, column) >= a && (row, column) <= b) {
                style.bg(view.theme.selection)
            } else if row == view.detail_row && view.focus == Focus::Detail {
                style.add_modifier(Modifier::BOLD)
            } else {
                style
            }
        },
    );
    if view.detail == Detail::Samples
        && view.config.show_ad_bars
        && source_width >= view.horizontal
        && rendered + 5 < width
        && let Some(index) = view
            .entry()
            .and_then(|e| e.record.format.iter().position(|f| f == "AD"))
        && let Some(value) = line.split('\t').nth(index + 1)
    {
        spans.push(Span::styled("  AD ", Style::default().fg(view.theme.muted)));
        spans.extend(palette::allele_depth(
            value,
            (width - rendered - 5).min(18),
            &view.config,
            &view.theme,
        ));
    }
    spans
}

fn draw_details(frame: &mut Frame, view: &VcfView) {
    if view.layout.details.height == 0 {
        return;
    }
    frame.render_widget(
        Block::default()
            .borders(Borders::ALL)
            .title(" Details ")
            .border_style(Style::default().fg(if view.focus == Focus::Detail {
                view.theme.focus
            } else {
                view.theme.border
            })),
        view.layout.details,
    );
    let labels = ["INFO", "SAMPLES", "RAW"];
    let tabs = labels
        .into_iter()
        .enumerate()
        .map(|(index, label)| {
            Span::styled(
                format!(" {label:<10} "),
                if index == view.detail.index() {
                    Style::default()
                        .fg(view.theme.focus)
                        .bg(view.theme.selection)
                } else {
                    Style::default().fg(view.theme.muted)
                },
            )
        })
        .collect::<Vec<_>>();
    frame.render_widget(Paragraph::new(Line::from(tabs)), view.layout.detail_tabs);
    let stops = tab_stops(view);
    let heading = match view.detail {
        Detail::Info => "FIELD       VALUE".into(),
        Detail::Samples => view
            .entry()
            .map(|e| format!("SAMPLE\t{}", e.record.format.join("\t")))
            .unwrap_or_else(|| "SAMPLE".into()),
        Detail::Raw => "Original VCF row · TABs preserved when copied".into(),
    };
    let (heading, _, _) = clip_spans(
        vec![Span::styled(heading, Style::default().fg(view.theme.muted))],
        view.horizontal,
        view.layout.detail_heading.width as usize,
        &stops,
        |_, style| style,
    );
    frame.render_widget(
        Paragraph::new(Line::from(heading)),
        view.layout.detail_heading,
    );
    if view.entry().is_none() {
        let error = view.load_error.as_deref().or_else(|| {
            view.snapshot
                .as_ref()
                .and_then(|snapshot| snapshot.progress.error.as_deref())
        });
        let message = if let Some(error) = error {
            format!("Error: {error}")
        } else if view
            .snapshot
            .as_ref()
            .is_some_and(|snapshot| snapshot.progress.complete && snapshot.total == 0)
        {
            "No record selected".into()
        } else {
            "Reading selected record…".into()
        };
        frame.render_widget(
            Paragraph::new(message).wrap(Wrap { trim: false }),
            view.layout.detail_body,
        );
        return;
    }
    if view.detail == Detail::Raw {
        let line = &view.entry().unwrap().record.raw;
        frame.render_widget(
            Paragraph::new(Line::from(visible_detail(line, 0, view, &stops))),
            view.layout.detail_body,
        );
        return;
    }
    let rows = view.detail_rows(view.detail_start, view.layout.detail_body.height as usize);
    if rows.is_empty() {
        frame.render_widget(
            Paragraph::new(match view.detail {
                Detail::Info => "No INFO fields",
                Detail::Samples => "No samples",
                Detail::Raw => "No record",
            }),
            view.layout.detail_body,
        );
        return;
    }
    let lines = rows
        .iter()
        .enumerate()
        .map(|(offset, line)| {
            Line::from(visible_detail(
                line,
                view.detail_start + offset,
                view,
                &stops,
            ))
        })
        .collect::<Vec<_>>();
    frame.render_widget(Paragraph::new(lines), view.layout.detail_body);
}

fn draw_picker(frame: &mut Frame, view: &VcfView) {
    let (area, list, start) = view.picker_layout();
    if area.width == 0 || area.height == 0 {
        return;
    }
    frame.render_widget(Clear, area);
    frame.render_widget(
        Block::default()
            .borders(Borders::ALL)
            .title(" Contig · type to search · Enter selects ")
            .border_style(Style::default().fg(view.theme.focus))
            .style(Style::default().bg(view.theme.background)),
        area,
    );
    let inner = layout::inset(area);
    let query = match &view.input {
        Some(Input::Contig(query)) => query.as_str(),
        _ => "",
    };
    frame.render_widget(
        Paragraph::new(format!("Find: {query}")),
        Rect::new(inner.x, inner.y, inner.width, inner.height.min(1)),
    );
    let lines = view
        .picker_options()
        .into_iter()
        .enumerate()
        .skip(start)
        .take(list.height as usize)
        .map(|(index, (name, count))| {
            Line::styled(
                format!("{}  ({count})", name.as_deref().unwrap_or("All records")),
                if index == view.picker_row {
                    Style::default()
                        .bg(view.theme.selection)
                        .fg(view.theme.focus)
                } else {
                    Style::default().fg(view.theme.foreground)
                },
            )
        })
        .collect::<Vec<_>>();
    frame.render_widget(Paragraph::new(lines), list);
}
