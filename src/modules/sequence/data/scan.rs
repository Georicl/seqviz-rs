//! Incremental parser input and record output. Neither side knows SQLite or UI state.
use super::CHUNK;
use std::io::{self, BufRead};

/// Borrows one logical-byte cursor for a scan call. The source may already be decompressed.
pub(super) struct ScanInput<'a, R: BufRead> {
    reader: &'a mut R,
    position: &'a mut u64,
}
impl<'a, R: BufRead> ScanInput<'a, R> {
    pub fn new(reader: &'a mut R, position: &'a mut u64) -> Self {
        Self { reader, position }
    }
    pub fn position(&self) -> u64 {
        *self.position
    }
    pub fn available(&mut self) -> io::Result<&[u8]> {
        self.reader.fill_buf()
    }
    pub fn consume(&mut self, count: usize) {
        self.reader.consume(count);
        *self.position += count as u64;
    }
    pub fn take_bytes(&mut self, max: usize) -> io::Result<Vec<u8>> {
        let available = self.available()?;
        let count = max.min(CHUNK).min(available.len());
        let bytes = available[..count].to_vec();
        self.consume(count);
        Ok(bytes)
    }
}

#[derive(Clone, Copy, Debug)]
pub(super) struct RecordState {
    pub length: u64,
    pub qlen: u64,
}

/// Sequence coordinates and qualities have separate source spans.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum SpanKind {
    Sequence,
    Quality,
}

/// A source span is decoded in the same way during scanning and interval reads.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum SpanEncoding {
    Text,
    BamSequence,
    BamQuality,
}

/// Parser output needed by the supported formats. Implementations own record IDs,
/// persistence and aggregates; parsers only describe records and source spans.
pub(super) trait RecordSink {
    fn current(&self) -> Option<RecordState>;
    fn begin_record(&mut self, header: String, has_sequence: bool) -> io::Result<()>;
    fn set_plus(&mut self, plus: String) -> io::Result<()>;
    fn set_fields(&mut self, fields: &[(String, String)]) -> io::Result<()>;
    fn set_sequence_missing(&mut self);
    fn append(
        &mut self,
        raw: &[u8],
        offset: u64,
        kind: SpanKind,
        encoding: SpanEncoding,
        decoded: &[u8],
    ) -> io::Result<()>;
    fn flush_span(&mut self) -> io::Result<()>;
    fn end_record(&mut self, next_offset: u64) -> io::Result<()>;
    fn discard_current(&mut self) -> io::Result<()>;
    fn log_error(&mut self, offset: u64, message: &str) -> io::Result<()>;
    fn set_resume(&mut self, offset: u64) -> io::Result<()>;
    fn clear_references(&mut self) -> io::Result<()>;
    fn add_reference(&mut self, id: i32, name: &str, length: u64) -> io::Result<()>;
    fn reference_name(&self, id: i32) -> io::Result<String>;
    fn finish_header(&mut self, offset: u64) -> io::Result<()>;
}
