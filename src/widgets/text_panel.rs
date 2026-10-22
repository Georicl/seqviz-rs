//! Selectable, scrollable text with display-column-aware wrapping.
use super::theme::Theme;
use ratatui::{
    Frame,
    layout::Rect,
    style::Style,
    text::{Line, Span},
    widgets::{Block, Borders, Clear, Paragraph},
};
use unicode_width::UnicodeWidthChar;

#[derive(Default)]
pub struct State {
    pub scroll: usize,
    pub lines: Vec<String>,
    pub selection: Option<((usize, usize), (usize, usize))>,
}

impl State {
    pub fn reflow(&mut self, lines: Vec<String>, width: u16, height: u16) {
        self.lines = lines
            .into_iter()
            .flat_map(|line| wrap_line(&line, width.max(1) as usize))
            .collect();
        self.scroll = self
            .scroll
            .min(self.lines.len().saturating_sub(height as usize));
    }

    /// Convert a pointer in the inner text area to row and character indices.
    pub fn position(&self, area: Rect, x: u16, y: u16) -> (usize, usize) {
        let row =
            self.scroll + usize::from(y.saturating_sub(area.y).min(area.height.saturating_sub(1)));
        let column = x.saturating_sub(area.x) as usize;
        let mut used = 0;
        let mut index = 0;
        if let Some(line) = self.lines.get(row) {
            for c in line.chars() {
                let width = c.width().unwrap_or(0);
                if used + width > column {
                    break;
                }
                used += width;
                index += 1;
            }
        }
        (row, index)
    }

    pub fn selected_text(&self) -> String {
        let Some((a, b)) = self.selection else {
            return self.lines.join("\n");
        };
        let (start, end) = if a <= b { (a, b) } else { (b, a) };
        self.lines
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
    }

    pub fn clear_selection(&mut self) {
        self.selection = None;
    }
}

pub fn draw(frame: &mut Frame, area: Rect, title: &str, state: &State, theme: &Theme) {
    let block = Block::default()
        .borders(Borders::ALL)
        .title(format!(" {title} "))
        .border_style(Style::default().fg(theme.focus))
        .style(Style::default().bg(theme.background));
    let inner = block.inner(area);
    frame.render_widget(Clear, area);
    frame.render_widget(block, area);
    let selected = state
        .selection
        .map(|(a, b)| if a <= b { (a, b) } else { (b, a) });
    let lines: Vec<Line<'_>> = state
        .lines
        .iter()
        .enumerate()
        .skip(state.scroll)
        .take(inner.height as usize)
        .map(|(row, line)| {
            Line::from(
                line.chars()
                    .enumerate()
                    .map(|(column, c)| {
                        let style = if selected
                            .is_some_and(|(a, b)| (row, column) >= a && (row, column) <= b)
                        {
                            Style::default().bg(theme.selection)
                        } else {
                            Style::default()
                        };
                        Span::styled(c.to_string(), style)
                    })
                    .collect::<Vec<_>>(),
            )
        })
        .collect();
    frame.render_widget(Paragraph::new(lines), inner);
}

pub fn wrap_line(line: &str, width: usize) -> Vec<String> {
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
    use super::wrap_line;

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
