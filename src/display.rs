//! 按终端宽度逐行输出 FASTA，读取分块与显示换行各自处理。
use crate::fa_reader::{FastaSequenceReader, read_header};
use std::io::{self, BufRead, Write};
use terminal_size::{Width, terminal_size_of};

const READ_CHUNK_SIZE: usize = 8192;
const DEFAULT_WIDTH: usize = 80;

/// 获取标准输出所在终端的列数；重定向输出时使用 80 列。
pub fn terminal_width() -> usize {
    terminal_size_of(io::stdout())
        .map(|(Width(width), _)| usize::from(width))
        .filter(|&width| width > 0)
        .unwrap_or(DEFAULT_WIDTH)
}

/// 连续输出所有记录，每个序列行开始时重新获取显示宽度。
pub fn write_fasta(
    reader: &mut impl BufRead,
    output: &mut impl Write,
    mut width: impl FnMut() -> usize,
) -> io::Result<()> {
    while let Some(header) = read_header(reader)? {
        writeln!(output, "{}", header.header)?;
        let mut sequence = FastaSequenceReader::new(&mut *reader);
        write_sequence(&mut sequence, output, &mut width)?;
    }
    Ok(())
}

fn write_sequence(
    sequence: &mut FastaSequenceReader<impl BufRead>,
    output: &mut impl Write,
    width: &mut impl FnMut() -> usize,
) -> io::Result<()> {
    let mut column = 0;
    let mut line_width = 1;

    loop {
        let (chunk, finished) = sequence.read_chunk(READ_CHUNK_SIZE)?;
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

        if finished {
            if column > 0 {
                output.write_all(b"\n")?;
            }
            return Ok(());
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::{BufReader, Cursor};

    fn render(input: &[u8], width: usize) -> String {
        let mut output = Vec::new();
        write_fasta(&mut Cursor::new(input), &mut output, || width).unwrap();
        String::from_utf8(output).unwrap()
    }

    #[test]
    fn wraps_sequences_and_starts_each_record_on_a_new_line() {
        let input = b">seq1 description\r\nACGT\r\nTGCA\r\n>seq2\nAAA";
        assert_eq!(
            render(input, 5),
            ">seq1 description\nACGTT\nGCA\n>seq2\nAAA\n"
        );
    }

    #[test]
    fn exact_width_and_empty_records_add_no_blank_lines() {
        assert_eq!(
            render(b">one\nACGT\n>empty\n>last\nTGCA", 4),
            ">one\nACGT\n>empty\n>last\nTGCA\n"
        );
        assert_eq!(render(b"# comment\n\n", 4), "");
    }

    #[test]
    fn wrapping_is_independent_of_input_lines_and_read_chunks() {
        let sequence = "AcgTN-.*".repeat(2050);
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
        let mut reader = BufReader::with_capacity(3, Cursor::new(input));
        let mut output = Vec::new();
        write_fasta(&mut reader, &mut output, || 127).unwrap();
        assert_eq!(output, expected.as_bytes());
    }

    #[test]
    fn uses_updated_width_at_the_start_of_each_line() {
        let mut widths = [4, 2, 6].into_iter();
        let mut output = Vec::new();
        write_fasta(&mut Cursor::new(b">seq\nACGTACGTACGT"), &mut output, || {
            widths.next().unwrap()
        })
        .unwrap();
        assert_eq!(output, b">seq\nACGT\nAC\nGTACGT\n");
    }

    #[test]
    fn narrow_widths_still_make_progress() {
        for width in [0, 1] {
            assert_eq!(render(b">seq\nACG", width), ">seq\nA\nC\nG\n");
        }
    }
}
