use std::io::{self, BufRead};

#[derive(Debug)]
pub struct FastaReader {
    pub header: String,
    pub name: String,
    pub description: String,
}

/// 从当前位置读取下一条 FASTA header，后面的序列留给后续读取。
/// 跳过 header 前的空行和 `#` 注释行。
pub fn read_header(reader: &mut impl BufRead) -> io::Result<Option<FastaReader>> {
    let mut line = String::new();

    // 沿用同一个 reader，序列从 header 之后继续读。
    loop {
        line.clear();

        if reader.read_line(&mut line)? == 0 {
            return Ok(None);
        }

        let line = line.trim_end_matches(['\r', '\n']);

        if line.trim().is_empty() || line.starts_with('#') {
            continue;
        }

        if !line.starts_with('>') {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "Error format: Sequence Must start with header '>'",
            ));
        }

        let text = line[1..].trim_start();
        let mut parts = text.splitn(2, char::is_whitespace);

        let name = parts.next().unwrap_or("");
        let description = parts.next().unwrap_or("").trim_start();

        if name.is_empty() {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "Require name of sequence",
            ));
        }

        return Ok(Some(FastaReader {
            header: line.to_owned(),
            name: name.to_owned(),
            description: description.to_owned(),
        }));
    }
}

/// 分块读取当前 header 后的序列；下一条 header 留在原 reader 中。
pub struct FastaSequenceReader<R: BufRead> {
    reader: R,
    comment: bool,
    at_line_start: bool,
    finished: bool,
}
impl<R: BufRead> FastaSequenceReader<R> {
    /// 传入 `&mut reader`，当前记录读完后仍可用原 reader 读取下一条 header。
    pub fn new(reader: R) -> Self {
        Self {
            reader,
            comment: false,
            at_line_start: true,
            finished: false,
        }
    }

    /// 返回至多 `max_base` 个序列字节，以及是否已到当前记录末尾。
    /// 跳过换行和行首 `#` 注释，保留其余字节的原始内容。
    /// 满额时可能尚未遇到下一 header 或 EOF；调用方继续读取，直到返回 `true`。
    pub fn read_chunk(&mut self, max_base: usize) -> io::Result<(Vec<u8>, bool)> {
        if max_base == 0 {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "Sequence chunk size must be greater than zero",
            ));
        }

        let mut chunk: Vec<u8> = Vec::new();

        while chunk.len() < max_base && !self.finished {
            let buffer = self.reader.fill_buf()?;

            if buffer.is_empty() {
                self.finished = true;
                break;
            }

            let mut consumed = 0;
            for &byte in buffer {
                if chunk.len() >= max_base {
                    break;
                }

                match byte {
                    b'\n' => {
                        self.at_line_start = true;
                        self.comment = false;
                    }
                    b'\r' => {}

                    // 注释行里的内容直接跳过。
                    _ if self.comment => {}

                    b'#' if self.at_line_start => {
                        self.comment = true;
                        self.at_line_start = false;
                    }

                    // 下一条记录开始了，把这个 > 留在缓冲区里。
                    b'>' if self.at_line_start => {
                        self.finished = true;
                        break;
                    }

                    // 将序列字符加入本次返回的片段。
                    _ => {
                        chunk.push(byte);
                        self.at_line_start = false;
                    }
                }
                consumed += 1;
            }
            self.reader.consume(consumed);
        }

        Ok((chunk, self.finished))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::{BufReader, Cursor};

    #[test]
    fn skips_comments_and_blank_lines_and_preserves_header() {
        let mut reader = Cursor::new(b"# comment\n\n>seq1 example description\nACGT\n");
        let header = read_header(&mut reader).unwrap().unwrap();

        assert_eq!(header.header, ">seq1 example description");
        assert_eq!(header.name, "seq1");
        assert_eq!(header.description, "example description");
    }

    #[test]
    fn header_without_description_is_valid() {
        let mut reader = Cursor::new(b">seq1\nACGT\n");
        let header = read_header(&mut reader).unwrap().unwrap();

        assert_eq!(header.name, "seq1");
        assert!(header.description.is_empty());
    }

    #[test]
    fn handles_multiple_spaces_tabs_and_crlf() {
        let mut reader = Cursor::new(b"# comment\r\n\r\n>  seq1 \t  description with spaces\r\n");
        let header = read_header(&mut reader).unwrap().unwrap();

        assert_eq!(header.header, ">  seq1 \t  description with spaces");
        assert_eq!(header.name, "seq1");
        assert_eq!(header.description, "description with spaces");
    }

    #[test]
    fn header_can_cross_buffer_boundaries() {
        let description = "description ".repeat(40);
        let raw_header = format!(">seq1 {description}");
        let input = format!("{raw_header}\nACGT\n");
        let mut reader = BufReader::with_capacity(4, Cursor::new(input.as_bytes()));
        let header = read_header(&mut reader).unwrap().unwrap();

        assert_eq!(header.header, raw_header);
        assert_eq!(header.name, "seq1");
        assert_eq!(header.description, description);
    }

    #[test]
    fn leaves_sequence_and_later_headers_unread() {
        let mut reader = BufReader::with_capacity(64, Cursor::new(b">seq1\nACGT\n>seq2\nAAA\n"));
        read_header(&mut reader).unwrap().unwrap();

        let mut line = String::new();
        reader.read_line(&mut line).unwrap();
        assert_eq!(line, "ACGT\n");
        line.clear();
        reader.read_line(&mut line).unwrap();
        assert_eq!(line, ">seq2\n");
    }

    #[test]
    fn empty_or_comment_only_input_returns_none() {
        for input in ["", "# comment\n\n", "# comment without newline"] {
            assert!(
                read_header(&mut Cursor::new(input.as_bytes()))
                    .unwrap()
                    .is_none()
            );
        }
    }

    #[test]
    fn accepts_header_at_eof_without_final_newline() {
        let header = read_header(&mut Cursor::new(b">seq1 description"))
            .unwrap()
            .unwrap();
        assert_eq!(header.name, "seq1");
        assert_eq!(header.description, "description");
    }

    #[test]
    fn rejects_sequence_before_header_and_missing_name() {
        for input in ["ACGT\n>seq1\n", ">\n", "> \t\n"] {
            let error = read_header(&mut Cursor::new(input.as_bytes())).unwrap_err();
            assert_eq!(error.kind(), io::ErrorKind::InvalidData);
        }
    }

    #[test]
    fn propagates_invalid_utf8_error() {
        let error = read_header(&mut Cursor::new(b">seq1\xff\n")).unwrap_err();
        assert_eq!(error.kind(), io::ErrorKind::InvalidData);
    }

    #[test]
    fn reads_multiple_records_without_consuming_next_header() {
        let mut reader =
            BufReader::with_capacity(64, Cursor::new(b">first description\nACGT\n>second\nNN\n"));
        assert_eq!(read_header(&mut reader).unwrap().unwrap().name, "first");

        {
            let mut sequence = FastaSequenceReader::new(&mut reader);
            assert_eq!(sequence.read_chunk(8).unwrap(), (b"ACGT".to_vec(), true));
            assert_eq!(sequence.read_chunk(8).unwrap(), (Vec::new(), true));
        }
        assert_eq!(reader.fill_buf().unwrap()[0], b'>');
        assert_eq!(read_header(&mut reader).unwrap().unwrap().name, "second");
        {
            let mut sequence = FastaSequenceReader::new(&mut reader);
            assert_eq!(sequence.read_chunk(8).unwrap(), (b"NN".to_vec(), true));
        }
        assert!(read_header(&mut reader).unwrap().is_none());
    }

    #[test]
    fn preserves_sequence_across_buffer_and_chunk_boundaries() {
        let input = b">one\r\nAc\r\n# comment with > and sequence letters\r\n\r\ngTNry.-*\r\n>two\r\nMKuX\r\n";

        for buffer_size in 1..=7 {
            for chunk_size in 1..=7 {
                let mut reader = BufReader::with_capacity(buffer_size, Cursor::new(input));

                for (name, expected) in [("one", &b"AcgTNry.-*"[..]), ("two", &b"MKuX"[..])] {
                    assert_eq!(read_header(&mut reader).unwrap().unwrap().name, name);
                    let mut sequence = FastaSequenceReader::new(&mut reader);
                    let mut actual = Vec::new();
                    loop {
                        let (chunk, finished) = sequence.read_chunk(chunk_size).unwrap();
                        assert!(chunk.len() <= chunk_size);
                        assert!(finished || !chunk.is_empty());
                        actual.extend(chunk);
                        if finished {
                            break;
                        }
                    }
                    assert_eq!(actual, expected);
                }
                assert!(read_header(&mut reader).unwrap().is_none());
            }
        }
    }

    #[test]
    fn recognizes_record_and_comment_markers_only_at_line_start() {
        let mut reader = BufReader::with_capacity(1, Cursor::new(b">seq\nAc>G#T\n>next\n"));
        read_header(&mut reader).unwrap().unwrap();
        {
            let mut sequence = FastaSequenceReader::new(&mut reader);
            assert_eq!(sequence.read_chunk(20).unwrap(), (b"Ac>G#T".to_vec(), true));
        }
        assert_eq!(read_header(&mut reader).unwrap().unwrap().name, "next");
    }

    #[test]
    fn reads_empty_records_and_header_at_eof() {
        let mut reader = BufReader::with_capacity(
            1,
            Cursor::new(b">empty\n>comment_only\r\n# comment\r\n\r\n>last"),
        );
        for expected_name in ["empty", "comment_only", "last"] {
            assert_eq!(
                read_header(&mut reader).unwrap().unwrap().name,
                expected_name
            );
            let mut sequence = FastaSequenceReader::new(&mut reader);
            assert_eq!(sequence.read_chunk(3).unwrap(), (Vec::new(), true));
        }
        assert!(read_header(&mut reader).unwrap().is_none());
    }

    #[test]
    fn finishes_at_eof_without_a_final_newline() {
        for input in [&b">seq\nAcGT"[..], &b">seq\nAcGT\n# trailing comment"[..]] {
            let mut reader = BufReader::with_capacity(2, Cursor::new(input));
            read_header(&mut reader).unwrap().unwrap();
            let mut sequence = FastaSequenceReader::new(&mut reader);
            assert_eq!(sequence.read_chunk(2).unwrap(), (b"Ac".to_vec(), false));
            assert_eq!(sequence.read_chunk(2).unwrap(), (b"GT".to_vec(), false));
            assert_eq!(sequence.read_chunk(2).unwrap(), (Vec::new(), true));
        }
    }

    #[test]
    fn rejects_zero_chunk_size_without_consuming_sequence() {
        let mut reader = Cursor::new(b">seq\nAcGT\n");
        read_header(&mut reader).unwrap().unwrap();
        let mut sequence = FastaSequenceReader::new(&mut reader);
        assert_eq!(
            sequence.read_chunk(0).unwrap_err().kind(),
            io::ErrorKind::InvalidInput
        );
        assert_eq!(sequence.read_chunk(8).unwrap(), (b"AcGT".to_vec(), true));
    }
}
