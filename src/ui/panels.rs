//! Prepare selectable information text before drawing.
use super::app::{App, Page};
use crate::config;
use crate::data::SeqType;
use unicode_width::UnicodeWidthChar;

pub(super) fn text(app: &App) -> Vec<String> {
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
        if let Some(size) = app.tab().snapshot.as_ref().and_then(|s| s.file_size) {
            lines.push(format!("File size: {size} bytes"));
        }
        lines.push(format!(
            "Index cache: {}",
            config::cache_directory().display()
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
                    if snapshot.format.supports_n50() {
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
                if snapshot.format.is_alignment() {
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

pub(super) fn wrap_line(line: &str, width: usize) -> Vec<String> {
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
