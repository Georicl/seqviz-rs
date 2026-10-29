//! Incremental FASTA, FASTQ and SAM parsing over logical source bytes.
use std::io::{self, BufRead};

use super::{
    CHUNK, Format, invalid,
    scan::{RecordSink, ScanInput, SpanEncoding, SpanKind},
};

/// Incremental text-record parser with state that survives split lines and bounded scan calls.
pub(super) struct TextScanner {
    /// Text format whose parsing rules are active.
    format: Format,
    /// Whether the next fragment begins at a new source line.
    line_start: bool,
    /// Current FASTA or FASTQ section.
    phase: Phase,
    /// Header or separator text accumulated across fragments.
    text: Vec<u8>,
    /// Whether the current FASTA comment continues into the next fragment.
    comment: bool,
    /// SAM-specific field and row state.
    sam: Sam,
}

/// Current FASTA or FASTQ record section; `Start` waits for the next record.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Phase {
    /// Collect a defline without its format marker.
    Header,
    /// Stream sequence bytes into the record sink.
    Sequence,
    /// Collect a FASTQ separator line including its `+` marker.
    Plus,
    /// Stream FASTQ qualities until sequence and quality lengths match.
    Quality,
    /// Wait for the next FASTQ header or the first FASTA content.
    Start,
}

/// State of the current SAM row, including partial fields and recoverable errors.
#[derive(Default)]
struct Sam {
    /// Zero-based field number within the current SAM row.
    field: usize,
    /// Partial non-sequence field accumulated across fragments.
    token: Vec<u8>,
    /// First nine mandatory fields used to validate core metadata.
    fields: Vec<String>,
    /// Parsed optional fields for the current row.
    aux: Vec<(String, String)>,
    /// Whether this row is a SAM header line.
    header: bool,
    /// Whether this row has failed validation and must be skipped.
    bad: bool,
    /// Whether SEQ has been marked missing by a standalone `*`.
    sequence_star: bool,
    /// A pending `*` that may be a missing QUAL marker or a quality character.
    quality_star: bool,
}

impl TextScanner {
    /// Create a FASTA, FASTQ or SAM scanner positioned at the start of a record.
    pub fn new(format: Format) -> Self {
        Self {
            format,
            line_start: true,
            phase: Phase::Start,
            text: Vec::new(),
            comment: false,
            sam: Sam::default(),
        }
    }

    /// Consume at most `budget` logical bytes while retaining split-line and record state.
    /// Return `true` after validating and closing the final record at end of input.
    pub fn step<R: BufRead>(
        &mut self,
        input: &mut ScanInput<'_, R>,
        records: &mut impl RecordSink,
        budget: usize,
    ) -> io::Result<bool> {
        let begin = input.position();
        while input.position() - begin < budget as u64 {
            let offset = input.position();
            let remaining = budget - (input.position() - begin) as usize;
            let available = input.available()?;
            if available.is_empty() {
                return self.finish(records, input.position());
            }
            let count = available
                .iter()
                .position(|b| *b == b'\n')
                .map_or(available.len(), |p| p + 1)
                .min(remaining)
                .min(CHUNK);
            let bytes = available[..count].to_vec();
            input.consume(count);
            let end_line = bytes.last() == Some(&b'\n');
            match self.format {
                Format::Fasta => self.fasta(records, &bytes, offset, end_line)?,
                Format::Fastq => self.fastq(records, &bytes, offset, end_line)?,
                Format::Sam => self.sam(records, &bytes, offset)?,
                Format::Bam => unreachable!(),
            }
            self.line_start = end_line;
        }
        Ok(false)
    }

    /// Process a FASTA line fragment, excluding comments from spans and closing records at deflines.
    fn fasta(
        &mut self,
        records: &mut impl RecordSink,
        bytes: &[u8],
        offset: u64,
        end: bool,
    ) -> io::Result<()> {
        if self.line_start && bytes.first() == Some(&b'#') {
            records.flush_span()?;
            self.comment = true;
        }
        if self.comment {
            if end {
                self.comment = false;
            }
            return Ok(());
        }
        let header = self.line_start && bytes.first() == Some(&b'>');
        if header {
            if records.current().is_some() {
                records.end_record(offset)?;
            }
            self.phase = Phase::Header;
            self.text.clear();
        }
        if self.phase == Phase::Header {
            self.text
                .extend_from_slice(if header { &bytes[1..] } else { bytes });
            if end {
                records.begin_record(defline(&self.text)?, true)?;
                self.text.clear();
                self.phase = Phase::Sequence;
            }
        } else {
            let bases = sequence(bytes, false)?;
            if !bases.is_empty() {
                records.append(
                    bytes,
                    offset,
                    SpanKind::Sequence,
                    SpanEncoding::Text,
                    &bases,
                )?;
            }
        }
        Ok(())
    }

    /// Advance the wrapped FASTQ header, sequence, separator and quality phases.
    /// Quality length determines record completion, so `@` and `+` remain valid quality bytes.
    fn fastq(
        &mut self,
        records: &mut impl RecordSink,
        bytes: &[u8],
        offset: u64,
        end: bool,
    ) -> io::Result<()> {
        let mut content = bytes;
        if self.phase == Phase::Start {
            if bytes.iter().all(u8::is_ascii_whitespace) {
                return Ok(());
            }
            if !self.line_start || bytes.first() != Some(&b'@') {
                return Err(invalid("Expected FASTQ @ header"));
            }
            self.phase = Phase::Header;
            self.text.clear();
            content = &bytes[1..];
        }
        if self.phase == Phase::Sequence && self.line_start && bytes.first() == Some(&b'+') {
            self.phase = Phase::Plus;
            self.text.clear();
        }
        match self.phase {
            Phase::Header => {
                self.text.extend_from_slice(content);
                if end {
                    records.begin_record(defline(&self.text)?, true)?;
                    self.text.clear();
                    self.phase = Phase::Sequence;
                }
            }
            Phase::Sequence => {
                let bases = sequence(bytes, false)?;
                records.append(
                    bytes,
                    offset,
                    SpanKind::Sequence,
                    SpanEncoding::Text,
                    &bases,
                )?;
            }
            Phase::Plus => {
                self.text.extend_from_slice(bytes);
                if end {
                    let plus = defline(&self.text)?;
                    records.set_plus(plus)?;
                    self.text.clear();
                    self.phase = Phase::Quality;
                }
            }
            Phase::Quality => {
                let quality = quality(bytes)?;
                let current = records.current().unwrap();
                if current.qlen + quality.len() as u64 > current.length {
                    return Err(invalid("FASTQ quality is longer than its sequence"));
                }
                records.append(
                    bytes,
                    offset,
                    SpanKind::Quality,
                    SpanEncoding::Text,
                    &quality,
                )?;
                let current = records.current().unwrap();
                if end && current.qlen == current.length {
                    records.end_record(offset + bytes.len() as u64)?;
                    self.phase = Phase::Start;
                }
            }
            Phase::Start => unreachable!(),
        }
        Ok(())
    }

    /// Split SAM fragments at tabs and newlines, recovering from invalid data at the next row.
    fn sam(&mut self, records: &mut impl RecordSink, bytes: &[u8], offset: u64) -> io::Result<()> {
        let mut begin = 0;
        for (i, byte) in bytes.iter().enumerate() {
            if *byte != b'\t' && *byte != b'\n' {
                continue;
            }
            let part = &bytes[begin..i];
            let result = self
                .sam_part(records, part, offset + begin as u64)
                .and_then(|()| self.sam_field(records, *byte == b'\n'));
            if let Err(error) = result {
                if error.kind() != io::ErrorKind::InvalidData {
                    return Err(error);
                }
                if !self.sam.bad {
                    records.log_error(offset + begin as u64, &format!("SAM: {error}"))?;
                }
                self.sam.bad = true;
                records.discard_current()?;
            }
            if *byte == b'\n' {
                if !self.sam.bad && !self.sam.header && records.current().is_some() {
                    records.end_record(offset + i as u64 + 1)?;
                } else {
                    records.set_resume(offset + i as u64 + 1)?;
                }
                self.sam = Sam::default();
            }
            begin = i + 1;
        }
        if begin < bytes.len()
            && let Err(error) = self.sam_part(records, &bytes[begin..], offset + begin as u64)
        {
            if error.kind() != io::ErrorKind::InvalidData {
                return Err(error);
            }
            if !self.sam.bad {
                records.log_error(offset + begin as u64, &format!("SAM: {error}"))?;
            }
            self.sam.bad = true;
            records.discard_current()?;
        }
        Ok(())
    }

    /// Stream a SAM field fragment, retaining missing-value markers across chunk boundaries.
    fn sam_part(
        &mut self,
        records: &mut impl RecordSink,
        part: &[u8],
        offset: u64,
    ) -> io::Result<()> {
        if self.sam.bad || self.sam.header {
            return Ok(());
        }
        let content = part.strip_suffix(b"\r").unwrap_or(part);
        if self.sam.field == 9 {
            if content == b"*" && records.current().unwrap().length == 0 {
                self.sam.sequence_star = true;
                records.set_sequence_missing();
            } else if !content.is_empty() {
                if self.sam.sequence_star {
                    return Err(invalid("Invalid SEQ missing-value marker"));
                }
                let bases = sequence(content, true)?;
                records.append(
                    content,
                    offset,
                    SpanKind::Sequence,
                    SpanEncoding::Text,
                    &bases,
                )?;
            }
        } else if self.sam.field == 10 {
            if content == b"*" && records.current().unwrap().qlen == 0 && !self.sam.quality_star {
                // A single '*' is missing only when the complete field is one byte.
                self.sam.quality_star = true;
            } else if !content.is_empty() {
                if self.sam.quality_star {
                    records.append(
                        b"*",
                        offset - 1,
                        SpanKind::Quality,
                        SpanEncoding::Text,
                        b"*",
                    )?;
                    self.sam.quality_star = false;
                }
                let scores = quality(content)?;
                records.append(
                    content,
                    offset,
                    SpanKind::Quality,
                    SpanEncoding::Text,
                    &scores,
                )?;
            }
        } else {
            self.sam.token.extend_from_slice(content);
        }
        Ok(())
    }

    /// Validate a completed SAM field and assemble metadata before accepting the row.
    fn sam_field(&mut self, records: &mut impl RecordSink, end_line: bool) -> io::Result<()> {
        if self.sam.bad || self.sam.header {
            return Ok(());
        }
        if self.sam.field == 0 && self.sam.token.is_empty() && end_line {
            return Ok(());
        }
        if self.sam.field == 0
            && matches!(
                self.sam.token.as_slice(),
                b"@HD" | b"@SQ" | b"@RG" | b"@PG" | b"@CO"
            )
        {
            self.sam.header = true;
            self.sam.token.clear();
            return Ok(());
        }
        if self.sam.field < 9 {
            self.sam.fields.push(defline(&self.sam.token)?);
            self.sam.token.clear();
            if self.sam.field == 8 {
                let attributes = sam_attributes(&self.sam.fields)?;
                records.begin_record(self.sam.fields[0].clone(), true)?;
                records.set_fields(&attributes)?;
            }
        } else if self.sam.field == 9 {
            let current = records.current().unwrap();
            if current.length == 0 && !self.sam.sequence_star {
                return Err(invalid("Empty SEQ field; use * for missing sequence"));
            }
        } else if self.sam.field == 10 {
            let current = records.current().unwrap();
            if !self.sam.quality_star && current.qlen != current.length {
                return Err(invalid("SEQ and QUAL lengths differ"));
            }
            if !self.sam.quality_star && current.qlen == 0 {
                return Err(invalid("Empty QUAL field; use * for missing quality"));
            }
        } else {
            let token = defline(&self.sam.token)?;
            let (tag, value) = token
                .split_once(':')
                .ok_or_else(|| invalid("Malformed optional SAM field"))?;
            if tag.len() != 2 || !value.as_bytes().get(1).is_some_and(|b| *b == b':') {
                return Err(invalid("Malformed optional SAM field"));
            }
            self.sam.aux.push((tag.to_owned(), value.to_owned()));
            self.sam.token.clear();
        }
        if end_line {
            if self.sam.field < 10 {
                return Err(invalid("SAM record has fewer than 11 mandatory fields"));
            }
            let mut fields = sam_attributes(&self.sam.fields)?;
            fields.append(&mut self.sam.aux);
            records.set_fields(&fields)?;
        }
        self.sam.field += 1;
        Ok(())
    }

    /// Validate end-of-input state and complete the final record even without a trailing newline.
    fn finish(&mut self, records: &mut impl RecordSink, position: u64) -> io::Result<bool> {
        match self.format {
            Format::Fasta => {
                if self.phase == Phase::Header {
                    records.begin_record(defline(&self.text)?, true)?;
                }
                if records.current().is_some() {
                    records.end_record(position)?;
                }
            }
            Format::Fastq => {
                if self.phase != Phase::Start {
                    if self.phase == Phase::Quality
                        && records.current().is_some_and(|c| c.length == c.qlen)
                    {
                        records.end_record(position)?;
                        self.phase = Phase::Start;
                    } else {
                        return Err(invalid(
                            "Truncated FASTQ record or unequal sequence/quality lengths",
                        ));
                    }
                }
            }
            Format::Sam => {
                if !self.line_start {
                    if let Err(error) = self.sam_field(records, true) {
                        if error.kind() != io::ErrorKind::InvalidData {
                            return Err(error);
                        }
                        if !self.sam.bad {
                            records.log_error(position, &format!("SAM: {error}"))?;
                        }
                        self.sam.bad = true;
                        records.discard_current()?;
                    }
                    if !self.sam.bad && records.current().is_some() {
                        records.end_record(position)?;
                    }
                }
            }
            Format::Bam => unreachable!(),
        }
        Ok(true)
    }
}

/// Decode UTF-8 header text and remove only trailing CR and LF characters.
fn defline(bytes: &[u8]) -> io::Result<String> {
    let text = std::str::from_utf8(bytes).map_err(|_| invalid("Header is not valid UTF-8"))?;
    Ok(text.trim_end_matches(['\r', '\n']).to_owned())
}

/// Strip ASCII whitespace and validate sequence symbols for text or alignment input.
fn sequence(bytes: &[u8], sam: bool) -> io::Result<Vec<u8>> {
    bytes
        .iter()
        .filter(|b| !b.is_ascii_whitespace())
        .map(|b| {
            if b.is_ascii_alphabetic()
                || b"-.".contains(b)
                || (!sam && *b == b'*')
                || (sam && *b == b'=')
            {
                Ok(*b)
            } else {
                Err(invalid(format!("Invalid sequence character 0x{b:02x}")))
            }
        })
        .collect()
}

/// Remove line endings and validate printable Phred+33 quality bytes.
fn quality(bytes: &[u8]) -> io::Result<Vec<u8>> {
    bytes
        .iter()
        .filter(|b| **b != b'\r' && **b != b'\n')
        .map(|b| {
            if (33..=126).contains(b) {
                Ok(*b)
            } else {
                Err(invalid(format!("Invalid Phred+33 quality byte 0x{b:02x}")))
            }
        })
        .collect()
}

/// Validate numeric SAM core fields and CIGAR, then add a direction derived from FLAG.
fn sam_attributes(fields: &[String]) -> io::Result<Vec<(String, String)>> {
    let flag: u16 = fields[1]
        .parse()
        .map_err(|_| invalid("Invalid FLAG field"))?;
    let _: u8 = fields[4]
        .parse()
        .map_err(|_| invalid("Invalid MAPQ field"))?;
    for index in [3, 7] {
        let position: u32 = fields[index]
            .parse()
            .map_err(|_| invalid("Invalid position field"))?;
        if position > i32::MAX as u32 {
            return Err(invalid("Position is out of range"));
        }
    }
    let _: i32 = fields[8]
        .parse()
        .map_err(|_| invalid("Invalid TLEN field"))?;
    validate_cigar(&fields[5])?;
    let mut result = [
        "FLAG", "RNAME", "POS", "MAPQ", "CIGAR", "RNEXT", "PNEXT", "TLEN",
    ]
    .into_iter()
    .zip(&fields[1..])
    .map(|(name, value)| (name.to_owned(), value.clone()))
    .collect::<Vec<_>>();
    result.push((
        "Direction".into(),
        if flag & 16 != 0 { "Reverse" } else { "Forward" }.into(),
    ));
    Ok(result)
}

/// Accept `*` or a series of positive lengths and supported SAM CIGAR operations.
fn validate_cigar(cigar: &str) -> io::Result<()> {
    if cigar == "*" {
        return Ok(());
    }
    let mut digits = false;
    let mut number = 0u64;
    for b in cigar.bytes() {
        if b.is_ascii_digit() {
            digits = true;
            number = number
                .checked_mul(10)
                .and_then(|n| n.checked_add(u64::from(b - b'0')))
                .ok_or_else(|| invalid("CIGAR length overflow"))?;
        } else if b"MIDNSHP=X".contains(&b) && digits && number > 0 {
            digits = false;
            number = 0;
        } else {
            return Err(invalid("Invalid CIGAR field"));
        }
    }
    if digits || cigar.is_empty() {
        return Err(invalid("Invalid CIGAR field"));
    }
    Ok(())
}
