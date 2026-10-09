use std::io::{self, BufRead};

#[derive(Debug)]
pub struct FastaReader {
    pub header: String,
    pub name: String,
    pub description: String,
}

/// 读取第一条 FASTA header，后面的序列留给后续读取。
/// 跳过 header 前的空行和 `#` 注释行。
pub fn read_first_header(reader: &mut impl BufRead) -> io::Result<Option<FastaReader>> {
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

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::{BufReader, Cursor};

    #[test]
    fn skips_comments_and_blank_lines_and_preserves_header() {
        let mut reader = Cursor::new(b"# comment\n\n>seq1 example description\nACGT\n");
        let header = read_first_header(&mut reader).unwrap().unwrap();

        assert_eq!(header.header, ">seq1 example description");
        assert_eq!(header.name, "seq1");
        assert_eq!(header.description, "example description");
    }

    #[test]
    fn header_without_description_is_valid() {
        let mut reader = Cursor::new(b">seq1\nACGT\n");
        let header = read_first_header(&mut reader).unwrap().unwrap();

        assert_eq!(header.name, "seq1");
        assert!(header.description.is_empty());
    }

    #[test]
    fn handles_multiple_spaces_tabs_and_crlf() {
        let mut reader = Cursor::new(b"# comment\r\n\r\n>  seq1 \t  description with spaces\r\n");
        let header = read_first_header(&mut reader).unwrap().unwrap();

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
        let header = read_first_header(&mut reader).unwrap().unwrap();

        assert_eq!(header.header, raw_header);
        assert_eq!(header.name, "seq1");
        assert_eq!(header.description, description);
    }

    #[test]
    fn leaves_sequence_and_later_headers_unread() {
        let mut reader = BufReader::with_capacity(64, Cursor::new(b">seq1\nACGT\n>seq2\nAAA\n"));
        read_first_header(&mut reader).unwrap().unwrap();

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
                read_first_header(&mut Cursor::new(input.as_bytes()))
                    .unwrap()
                    .is_none()
            );
        }
    }

    #[test]
    fn accepts_header_at_eof_without_final_newline() {
        let header = read_first_header(&mut Cursor::new(b">seq1 description"))
            .unwrap()
            .unwrap();
        assert_eq!(header.name, "seq1");
        assert_eq!(header.description, "description");
    }

    #[test]
    fn rejects_sequence_before_header_and_missing_name() {
        for input in ["ACGT\n>seq1\n", ">\n", "> \t\n"] {
            let error = read_first_header(&mut Cursor::new(input.as_bytes())).unwrap_err();
            assert_eq!(error.kind(), io::ErrorKind::InvalidData);
        }
    }

    #[test]
    fn propagates_invalid_utf8_error() {
        let error = read_first_header(&mut Cursor::new(b">seq1\xff\n")).unwrap_err();
        assert_eq!(error.kind(), io::ErrorKind::InvalidData);
    }
}
