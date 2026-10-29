//! Incremental parser input and record output. Neither side knows SQLite or UI state.
use super::CHUNK;
use std::io::{self, BufRead};

/// Borrows one logical-byte cursor for a scan call. The source may already be decompressed.
pub(super) struct ScanInput<'a, R: BufRead> {
    /// Buffered source exposing logical, possibly decompressed bytes.
    reader: &'a mut R,
    /// Shared logical offset advanced alongside the buffered reader.
    position: &'a mut u64,
}
impl<'a, R: BufRead> ScanInput<'a, R> {
    /// Borrow the buffered reader and its logical, decompressed byte offset for one scan step.
    pub fn new(reader: &'a mut R, position: &'a mut u64) -> Self {
        Self { reader, position }
    }
    /// Return the logical-byte offset of the next unconsumed source byte.
    pub fn position(&self) -> u64 {
        *self.position
    }
    /// Expose the buffered source bytes without advancing the logical position.
    pub fn available(&mut self) -> io::Result<&[u8]> {
        self.reader.fill_buf()
    }
    /// Advance both the reader and the logical position by the consumed byte count.
    pub fn consume(&mut self, count: usize) {
        self.reader.consume(count);
        *self.position += count as u64;
    }
    /// Consume at most `max`, one chunk, and the currently buffered bytes.
    pub fn take_bytes(&mut self, max: usize) -> io::Result<Vec<u8>> {
        let available = self.available()?;
        let count = max.min(CHUNK).min(available.len());
        let bytes = available[..count].to_vec();
        self.consume(count);
        Ok(bytes)
    }
}

/// Decoded lengths of the current record, used to validate parser boundaries.
#[derive(Clone, Copy, Debug)]
pub(super) struct RecordState {
    /// Sequence symbols decoded for the current record.
    pub length: u64,
    /// Quality scores decoded for the current record.
    pub qlen: u64,
}

/// Sequence coordinates and qualities have separate source spans.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum SpanKind {
    /// Record sequence symbols in zero-based sequence coordinates.
    Sequence,
    /// Phred scores in zero-based quality coordinates.
    Quality,
}

/// A source span is decoded in the same way during scanning and interval reads.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum SpanEncoding {
    /// ASCII symbols with source whitespace removed during interval reads.
    Text,
    /// Two four-bit nucleotide codes per source byte, high nibble first.
    BamSequence,
    /// Raw numeric Phred scores converted to Phred+33 for display.
    BamQuality,
}

/// Parser output needed by the supported formats. Implementations own record IDs,
/// persistence and aggregates; parsers only describe records and source spans.
pub(super) trait RecordSink {
    /// Return the decoded sequence and quality lengths of the record being assembled.
    fn current(&self) -> Option<RecordState>;
    /// Start a record from its defline or read name and indicate whether sequence is present.
    fn begin_record(&mut self, header: String, has_sequence: bool) -> io::Result<()>;
    /// Store the complete FASTQ separator line, including its leading `+`.
    fn set_plus(&mut self, plus: String) -> io::Result<()>;
    /// Replace the current record's ordered metadata fields.
    fn set_fields(&mut self, fields: &[(String, String)]) -> io::Result<()>;
    /// Mark the current alignment record as having a missing SEQ field.
    fn set_sequence_missing(&mut self);
    /// Append decoded sequence or Phred+33 quality and its corresponding source span.
    /// `offset` counts logical source bytes; `raw` retains the original encoding.
    fn append(
        &mut self,
        raw: &[u8],
        offset: u64,
        kind: SpanKind,
        encoding: SpanEncoding,
        decoded: &[u8],
    ) -> io::Result<()>;
    /// Persist any pending span before a discontinuity in the source or record stream.
    fn flush_span(&mut self) -> io::Result<()>;
    /// Complete the current record and set the next safe resume offset in logical bytes.
    fn end_record(&mut self, next_offset: u64) -> io::Result<()>;
    /// Discard the partially assembled record after a recoverable row error.
    fn discard_current(&mut self) -> io::Result<()>;
    /// Record an input error at a logical-byte offset without deciding whether scanning stops.
    fn log_error(&mut self, offset: u64, message: &str) -> io::Result<()>;
    /// Set the logical-byte boundary from which a reopened scan can safely resume.
    fn set_resume(&mut self, offset: u64) -> io::Result<()>;
    /// Remove the reference dictionary before reading a new BAM header.
    fn clear_references(&mut self) -> io::Result<()>;
    /// Store a BAM reference ID, name and sequence length from the header.
    fn add_reference(&mut self, id: i32, name: &str, length: u64) -> io::Result<()>;
    /// Resolve a BAM reference ID to its name, mapping the missing ID `-1` to `*`.
    fn reference_name(&self, id: i32) -> io::Result<String>;
    /// Mark the BAM header as complete and persist the first alignment offset.
    fn finish_header(&mut self, offset: u64) -> io::Result<()>;
}
