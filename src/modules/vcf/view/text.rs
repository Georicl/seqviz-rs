//! Text geometry shared by drawing, pointer hit tests and original-text copying.
use super::{Detail, VcfView};
use ratatui::{style::Style, text::Span};
use unicode_width::{UnicodeWidthChar, UnicodeWidthStr};

/// Width at the original display column, shared with pointer-to-text mapping.
pub(super) fn character_width(ch: char, column: usize, stops: &[usize]) -> usize {
    if ch == '\t' {
        stops
            .iter()
            .copied()
            .find(|&stop| stop > column)
            .map_or(12 - column % 12, |stop| stop - column)
    } else if ch.is_control() {
        1
    } else {
        ch.width().unwrap_or(0)
    }
}

pub(super) fn grapheme_width(symbol: &str, column: usize, stops: &[usize]) -> usize {
    let mut chars = symbol.chars();
    match (chars.next(), chars.next()) {
        (Some(ch), None) => character_width(ch, column, stops),
        _ => symbol.width(),
    }
}

/// Ratatui's grapheme iterator omits controls. Restore them so TAB expansion and
/// source character indices still describe the original logical line.
pub(super) fn walk_graphemes(text: &str, mut visit: impl FnMut(&str) -> bool) -> bool {
    let span = Span::raw(text);
    let mut offset = 0;
    for grapheme in span.styled_graphemes(Style::default()) {
        let start = offset + text[offset..].find(grapheme.symbol).unwrap_or(0);
        while offset < start {
            let end = offset + text[offset..].chars().next().unwrap().len_utf8();
            if !visit(&text[offset..end]) {
                return false;
            }
            offset = end;
        }
        if !visit(grapheme.symbol) {
            return false;
        }
        offset = start + grapheme.symbol.len();
    }
    while offset < text.len() {
        let end = offset + text[offset..].chars().next().unwrap().len_utf8();
        if !visit(&text[offset..end]) {
            return false;
        }
        offset = end;
    }
    true
}

/// Every visible sample and its heading share the same column boundaries.
pub(super) fn tab_stops(view: &VcfView) -> Vec<usize> {
    if view.detail != Detail::Samples {
        return Vec::new();
    }
    let Some(entry) = view.entry() else {
        return Vec::new();
    };
    let heading = format!("SAMPLE\t{}", entry.record.format.join("\t"));
    let rows = view.detail_rows(view.detail_start, view.layout.detail_body.height as usize);
    let mut widths = vec![12; entry.record.format.len() + 1];
    for line in std::iter::once(&heading).chain(rows.iter()) {
        for (index, value) in line.split('\t').enumerate() {
            if let Some(width) = widths.get_mut(index) {
                *width = (*width).max(value.width().saturating_add(1));
            }
        }
    }
    let mut end = 0;
    widths
        .into_iter()
        .map(|width| {
            end += width;
            end
        })
        .collect()
}
