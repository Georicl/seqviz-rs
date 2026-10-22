//! Scanners run with an in-memory cursor and record collector: no Source, files,
//! cache directory or SQLite is involved in these format-boundary tests.
use super::{
    Format,
    bam::BamScanner,
    invalid,
    scan::{RecordSink, RecordState, ScanInput, SpanEncoding, SpanKind},
    text::TextScanner,
};
use std::{
    collections::BTreeMap,
    io::{self, Cursor, Read},
};

#[derive(Default, Debug, PartialEq, Eq)]
struct ParsedRecord {
    header: String,
    plus: String,
    sequence: Vec<u8>,
    quality: Vec<u8>,
    fields: Vec<(String, String)>,
    has_sequence: bool,
}

#[derive(Default)]
struct Collector {
    current: Option<ParsedRecord>,
    records: Vec<ParsedRecord>,
    errors: Vec<(u64, String)>,
    references: BTreeMap<i32, String>,
    resume: u64,
    header_end: Option<u64>,
}

impl RecordSink for Collector {
    fn current(&self) -> Option<RecordState> {
        self.current.as_ref().map(|r| RecordState {
            length: r.sequence.len() as u64,
            qlen: r.quality.len() as u64,
        })
    }
    fn begin_record(&mut self, header: String, has_sequence: bool) -> io::Result<()> {
        assert!(self.current.is_none(), "Previous record must have ended");
        self.current = Some(ParsedRecord {
            header,
            has_sequence,
            ..Default::default()
        });
        Ok(())
    }
    fn set_plus(&mut self, plus: String) -> io::Result<()> {
        self.current.as_mut().unwrap().plus = plus;
        Ok(())
    }
    fn set_fields(&mut self, fields: &[(String, String)]) -> io::Result<()> {
        self.current.as_mut().unwrap().fields = fields.to_vec();
        Ok(())
    }
    fn set_sequence_missing(&mut self) {
        self.current.as_mut().unwrap().has_sequence = false;
    }
    fn append(
        &mut self,
        _raw: &[u8],
        _offset: u64,
        kind: SpanKind,
        _encoding: SpanEncoding,
        decoded: &[u8],
    ) -> io::Result<()> {
        let record = self
            .current
            .as_mut()
            .ok_or_else(|| invalid("Sequence before header"))?;
        if kind == SpanKind::Sequence {
            record.sequence.extend_from_slice(decoded);
        } else {
            record.quality.extend_from_slice(decoded);
        }
        Ok(())
    }
    fn flush_span(&mut self) -> io::Result<()> {
        Ok(())
    }
    fn end_record(&mut self, next_offset: u64) -> io::Result<()> {
        self.records.push(self.current.take().unwrap());
        self.resume = next_offset;
        Ok(())
    }
    fn discard_current(&mut self) -> io::Result<()> {
        self.current = None;
        Ok(())
    }
    fn log_error(&mut self, offset: u64, message: &str) -> io::Result<()> {
        self.errors.push((offset, message.to_owned()));
        Ok(())
    }
    fn set_resume(&mut self, offset: u64) -> io::Result<()> {
        self.resume = offset;
        Ok(())
    }
    fn clear_references(&mut self) -> io::Result<()> {
        self.references.clear();
        Ok(())
    }
    fn add_reference(&mut self, id: i32, name: &str, _length: u64) -> io::Result<()> {
        self.references.insert(id, name.to_owned());
        Ok(())
    }
    fn reference_name(&self, id: i32) -> io::Result<String> {
        if id == -1 {
            return Ok("*".into());
        }
        self.references
            .get(&id)
            .cloned()
            .ok_or_else(|| invalid("Unknown reference"))
    }
    fn finish_header(&mut self, offset: u64) -> io::Result<()> {
        self.header_end = Some(offset);
        self.resume = offset;
        Ok(())
    }
}

fn scan_text(bytes: &[u8], format: Format, budget: usize) -> io::Result<Collector> {
    let mut cursor = Cursor::new(bytes);
    let mut position = 0;
    let mut records = Collector::default();
    let mut parser = TextScanner::new(format);
    loop {
        let previous = position;
        let mut input = ScanInput::new(&mut cursor, &mut position);
        let done = parser.step(&mut input, &mut records, budget)?;
        assert!(position - previous <= budget as u64);
        if done {
            return Ok(records);
        }
    }
}

#[test]
fn fasta_boundaries_and_content_do_not_depend_on_a_database_or_chunk_size() {
    let bytes =
        b"# intro\r\n>first full description\r\nAc gT\r\n# ignored GGG\nN\n>protein\nMPEPTIDE*";
    for budget in [1, 3, 64 * 1024] {
        let result = scan_text(bytes, Format::Fasta, budget).unwrap();
        assert_eq!(result.records.len(), 2);
        assert_eq!(result.records[0].header, "first full description");
        assert_eq!(result.records[0].sequence, b"AcgTN");
        assert_eq!(result.records[1].header, "protein");
        assert_eq!(result.records[1].sequence, b"MPEPTIDE*");
        assert_eq!(result.resume, bytes.len() as u64);
    }
}

#[test]
fn wrapped_fastq_preserves_separator_and_accepts_marker_characters_in_quality() {
    let bytes = b"@read details\r\nACG\r\nTN\n+read details\n@+I\n?J\n@last\nA\n+\n!";
    for budget in [1, 7, 64 * 1024] {
        let result = scan_text(bytes, Format::Fastq, budget).unwrap();
        assert_eq!(result.records.len(), 2);
        assert_eq!(result.records[0].header, "read details");
        assert_eq!(result.records[0].plus, "+read details");
        assert_eq!(result.records[0].sequence, b"ACGTN");
        assert_eq!(result.records[0].quality, b"@+I?J");
        assert_eq!(result.records[1].quality, b"!");
    }
    assert_eq!(
        scan_text(b"@bad\nACGT\n+\nII\n", Format::Fastq, 1)
            .err()
            .unwrap()
            .kind(),
        io::ErrorKind::InvalidData
    );
}

#[test]
fn sam_recovers_at_row_boundary_and_preserves_missing_values() {
    let bytes=b"@HD\tVN:1.6\ninvalid\trow\nread\t4\t*\t0\t0\t*\t*\t0\t0\t*\t*\nnext\t0\tchr1\t2\t60\t2M\t*\t0\t0\tAC\tII";
    for budget in [1, 11, 64 * 1024] {
        let result = scan_text(bytes, Format::Sam, budget).unwrap();
        assert_eq!(result.errors.len(), 1);
        assert_eq!(result.records.len(), 2);
        assert!(!result.records[0].has_sequence);
        assert!(result.records[0].quality.is_empty());
        assert_eq!(result.records[1].sequence, b"AC");
        assert_eq!(result.records[1].quality, b"II");
    }
}

#[test]
fn bam_scanner_uses_logical_bytes_and_reference_events_without_sqlite() {
    // The existing samtools-verified fixture supplies an independent binary oracle.
    let mut logical = Vec::new();
    flate2::read::MultiGzDecoder::new(
        include_bytes!(concat!(env!("CARGO_MANIFEST_DIR"), "/test/alignments.bam")).as_slice(),
    )
    .read_to_end(&mut logical)
    .unwrap();
    let expected = scan_text(
        include_bytes!(concat!(env!("CARGO_MANIFEST_DIR"), "/test/alignments.sam")),
        Format::Sam,
        7,
    )
    .unwrap();
    for budget in [1, 37, 64 * 1024] {
        let mut cursor = Cursor::new(&logical);
        let mut position = 0;
        let mut records = Collector::default();
        let mut parser = BamScanner::new(false);
        loop {
            let previous = position;
            let mut input = ScanInput::new(&mut cursor, &mut position);
            let done = parser.step(&mut input, &mut records, budget).unwrap();
            assert!(position - previous <= budget as u64);
            if done {
                break;
            }
        }
        assert!(records.header_end.is_some());
        assert_eq!(records.records.len(), expected.records.len());
        for (actual, expected) in records.records.iter().zip(&expected.records) {
            assert_eq!(actual.header, expected.header);
            assert_eq!(actual.sequence, expected.sequence);
            assert_eq!(actual.quality, expected.quality);
            for key in ["RNAME", "POS", "MAPQ", "CIGAR", "FLAG", "Direction"] {
                let field = |r: &ParsedRecord| {
                    r.fields
                        .iter()
                        .find(|(name, _)| name == key)
                        .unwrap()
                        .1
                        .clone()
                };
                assert_eq!(field(actual), field(expected), "{key}");
            }
        }
    }
}
