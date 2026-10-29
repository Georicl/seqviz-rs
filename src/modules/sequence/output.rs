//! Streaming FASTA output over the same indexed reader used by the browser.
use super::data::{Dataset, Format};
use std::io::{self, Write};

/// Maximum sequence bytes materialised for one streaming output read.
const READ_CHUNK_SIZE: usize = 64 * 1024;
/// Print FASTA records without materialising an entire sequence in memory.
/// Format recognition, validation and gzip decoding belong to `Dataset`.
pub fn write_fasta(
    dataset: &mut Dataset,
    output: &mut impl Write,
    mut width: impl FnMut() -> usize,
) -> io::Result<()> {
    if dataset.format() != Format::Fasta {
        return Err(io::Error::new(
            io::ErrorKind::Unsupported,
            "Redirected output supports FASTA only; open this format in an interactive terminal",
        ));
    }
    let mut id = 0;
    loop {
        if let Some(record) = dataset.record(id)? {
            writeln!(output, ">{}", record.header)?;
            write_sequence(dataset, id, output, &mut width)?;
            id += 1;
        } else if dataset.progress().complete {
            return Ok(());
        } else {
            scan_step(dataset)?;
        }
    }
}

/// Stream one indexed record, rereading the requested width at the start of each output line.
fn write_sequence(
    dataset: &mut Dataset,
    id: u64,
    output: &mut impl Write,
    width: &mut impl FnMut() -> usize,
) -> io::Result<()> {
    let mut column = 0;
    let mut line_width = 1;
    let mut position = 0;
    loop {
        let record = dataset.record(id)?.ok_or_else(|| {
            io::Error::new(io::ErrorKind::InvalidData, "Indexed record disappeared")
        })?;
        let count = record
            .length
            .saturating_sub(position)
            .min(READ_CHUNK_SIZE as u64) as usize;
        let chunk = dataset.read_range(id, position, count)?.sequence;
        if chunk.len() != count {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "Incomplete indexed sequence range",
            ));
        }
        position += chunk.len() as u64;
        let mut offset = 0;

        while offset < chunk.len() {
            if column == 0 {
                line_width = width().max(1);
            }
            let count = (line_width - column).min(chunk.len() - offset);
            output.write_all(&chunk[offset..offset + count])?;
            offset += count;
            column += count;

            if column == line_width {
                output.write_all(b"\n")?;
                column = 0;
            }
        }

        if record.complete && position == record.length {
            if column > 0 {
                output.write_all(b"\n")?;
            }
            return Ok(());
        }
        if position == record.length {
            scan_step(dataset)?;
        }
    }
}

/// Advance one scan chunk and surface recorded parser failures as I/O errors.
fn scan_step(dataset: &mut Dataset) -> io::Result<()> {
    if let Some(error) = dataset.progress().error {
        return Err(io::Error::new(io::ErrorKind::InvalidData, error));
    }
    if let Some(error) = dataset.step(64 * 1024)?.error {
        return Err(io::Error::new(io::ErrorKind::InvalidData, error));
    }
    Ok(())
}

/// Streaming FASTA wrapping and normalisation regressions.
#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    /// Render a temporary FASTA source with a width callback into an in-memory byte buffer.
    fn render_with_width(input: &[u8], width: impl FnMut() -> usize) -> Vec<u8> {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("test.fa");
        fs::write(&path, input).unwrap();
        let mut dataset = Dataset::open(&path, &tmp.path().join("cache")).unwrap();
        let mut output = Vec::new();
        write_fasta(&mut dataset, &mut output, width).unwrap();
        output
    }

    /// Render a temporary FASTA source at a fixed width and decode its output as UTF-8.
    fn render(input: &[u8], width: usize) -> String {
        String::from_utf8(render_with_width(input, || width)).unwrap()
    }

    /// Check that input line endings do not affect wrapping or record boundaries.
    #[test]
    fn wraps_sequences_and_starts_each_record_on_a_new_line() {
        let input = b">seq1 description\r\nACGT\r\nTGCA\r\n>seq2\nAAA";
        assert_eq!(
            render(input, 5),
            ">seq1 description\nACGTT\nGCA\n>seq2\nAAA\n"
        );
    }

    /// Check that full-width lines, empty records and comment-only input produce no extra blank lines.
    #[test]
    fn exact_width_and_empty_records_add_no_blank_lines() {
        assert_eq!(
            render(b">one\nACGT\n>empty\n>last\nTGCA", 4),
            ">one\nACGT\n>empty\n>last\nTGCA\n"
        );
        assert_eq!(render(b"# comment\n\n", 4), "");
    }

    /// Check wrapping across 64 KiB reads using a line width that does not divide the chunk size.
    #[test]
    fn wrapping_is_independent_of_input_lines_and_read_chunks() {
        let sequence = "AcgTN-.*".repeat(20000);
        let input = format!(">long\n{sequence}\n");
        // 127 不整除读取块大小，显示行会跨过读取块边界。
        let expected = format!(
            ">long\n{}",
            sequence
                .as_bytes()
                .chunks(127)
                .map(|line| format!("{}\n", std::str::from_utf8(line).unwrap()))
                .collect::<String>()
        );
        let output = render_with_width(input.as_bytes(), || 127);
        assert_eq!(output, expected.as_bytes());
    }

    /// Check that changing widths apply only when the next output line starts.
    #[test]
    fn uses_updated_width_at_the_start_of_each_line() {
        let mut widths = [4, 2, 6].into_iter();
        let output = render_with_width(b">seq\nACGTACGTACGT", || widths.next().unwrap());
        assert_eq!(output, b">seq\nACGT\nAC\nGTACGT\n");
    }

    /// Check that zero and one-column widths both emit one base per line.
    #[test]
    fn narrow_widths_still_make_progress() {
        for width in [0, 1] {
            assert_eq!(render(b">seq\nACG", width), ">seq\nA\nC\nG\n");
        }
    }

    /// Check that output keeps sequence case while omitting whitespace and FASTA comments.
    #[test]
    fn uses_the_browser_sequence_normalisation() {
        assert_eq!(
            render(b">seq\nAC GT\tN\n# skipped\nac\n", 80),
            ">seq\nACGTNac\n"
        );
    }
}
