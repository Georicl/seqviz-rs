//! BAM v1.6 streaming decoder. The outer Source has already decoded BGZF.

use std::io::{self, BufRead};

use super::{
    BAM_BASES, CHUNK, invalid,
    scan::{RecordSink, ScanInput, SpanEncoding, SpanKind},
};

/// Next BAM field to decode; each phase may span multiple scan steps.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Phase {
    /// Validate the BAM signature.
    Magic,
    /// Read the textual header length.
    HeaderLength,
    /// Skip the textual header within the scan budget.
    HeaderText,
    /// Read the number of reference dictionary entries.
    ReferenceCount,
    /// Read the next reference name length including its NUL terminator.
    ReferenceNameLength,
    /// Collect the NUL-terminated reference name.
    ReferenceName,
    /// Read the reference sequence length and store its dictionary entry.
    ReferenceLength,
    /// Read the next alignment block size or accept clean end of input.
    RecordSize,
    /// Collect the 32 fixed alignment bytes.
    Core,
    /// Collect the alignment read name.
    Name,
    /// Decode the packed core CIGAR operations.
    Cigar,
    /// Decode packed nucleotide nibbles into sequence spans.
    Sequence,
    /// Validate missing or numeric quality and emit quality spans.
    Quality,
    /// Read an optional field tag and type, or complete the alignment.
    AuxHeader,
    /// Decode a single optional-field value.
    AuxScalar,
    /// Collect a NUL-terminated optional string.
    AuxString,
    /// Read an optional array subtype and element count.
    AuxArrayHeader,
    /// Decode array elements, including the long-CIGAR CG tag.
    AuxArray,
}

/// Incremental BAM decoder retaining partial fields, reference progress and alignment metadata.
pub(super) struct BamScanner {
    /// Next field or stream section to decode.
    phase: Phase,
    /// Partial fixed-width or name field retained across calls.
    buffer: Vec<u8>,
    /// Bytes remaining in the current header, sequence or quality section.
    remaining: usize,
    /// Number of entries declared in the BAM reference dictionary.
    references: i32,
    /// Zero-based reference dictionary entry currently being read.
    reference: i32,
    /// Decoded reference name awaiting its length.
    name: String,
    /// Exclusive logical-byte end of the current alignment block, or zero outside one.
    record_end: u64,
    /// The current alignment's 32-byte fixed core.
    core: [u8; 32],
    /// Read-name byte length including the terminating NUL.
    name_len: usize,
    /// Core CIGAR operations still to decode.
    cigar_count: usize,
    /// Declared number of sequence symbols.
    sequence_len: usize,
    /// Sequence symbols already decoded, excluding the unused final nibble.
    sequence_done: usize,
    /// Whether the first quality fragment declared all qualities missing.
    quality_missing: Option<bool>,
    /// Decoded core CIGAR, replaced by CG when a long CIGAR is supplied.
    cigar: String,
    /// Ordered alignment and auxiliary fields accumulated for this record.
    fields: Vec<(String, String)>,
    /// Current two-character auxiliary tag.
    aux_tag: String,
    /// Current auxiliary value type byte.
    aux_type: u8,
    /// Current auxiliary array element type.
    aux_subtype: u8,
    /// Display value being assembled for an auxiliary string or array.
    aux_value: String,
    /// Auxiliary array elements still to decode.
    aux_count: usize,
    /// CIGAR operations decoded from a CG:B:I array.
    aux_cigar: String,
}

impl BamScanner {
    /// Create a decoder at the BAM header or at an alignment boundary when the header is cached.
    pub fn new(header_done: bool) -> Self {
        Self {
            phase: if header_done {
                Phase::RecordSize
            } else {
                Phase::Magic
            },
            buffer: Vec::new(),
            remaining: 0,
            references: 0,
            reference: 0,
            name: String::new(),
            record_end: 0,
            core: [0; 32],
            name_len: 0,
            cigar_count: 0,
            sequence_len: 0,
            sequence_done: 0,
            quality_missing: None,
            cigar: String::new(),
            fields: Vec::new(),
            aux_tag: String::new(),
            aux_type: 0,
            aux_subtype: 0,
            aux_value: String::new(),
            aux_count: 0,
            aux_cigar: String::new(),
        }
    }

    /// Consume at most `budget` logical bytes, preserving partial fields between calls.
    /// Return `true` only at a clean alignment boundary at end of input.
    pub fn step<R: BufRead>(
        &mut self,
        input: &mut ScanInput<'_, R>,
        records: &mut impl RecordSink,
        budget: usize,
    ) -> io::Result<bool> {
        let begin = input.position();
        while input.position() - begin < budget as u64 {
            let left = budget - (input.position() - begin) as usize;
            match self.phase {
                Phase::Magic => {
                    if self.fill(input, 4, left)? {
                        if self.take().as_slice() != b"BAM\x01" {
                            return Err(invalid("Invalid BAM magic"));
                        }
                        self.phase = Phase::HeaderLength;
                    }
                }
                Phase::HeaderLength => {
                    if self.fill(input, 4, left)? {
                        self.remaining = nonnegative(&self.take(), "BAM header length")?;
                        self.phase = Phase::HeaderText;
                    }
                }
                Phase::HeaderText => {
                    if self.remaining == 0 {
                        self.phase = Phase::ReferenceCount;
                        continue;
                    }
                    let bytes = self.read(input, self.remaining.min(left))?;
                    self.remaining -= bytes.len();
                }
                Phase::ReferenceCount => {
                    if self.fill(input, 4, left)? {
                        self.references = nonnegative(&self.take(), "BAM reference count")? as i32;
                        self.reference = 0;
                        records.clear_references()?;
                        if self.references == 0 {
                            self.end_header(records, input.position())?;
                        } else {
                            self.phase = Phase::ReferenceNameLength;
                        }
                    }
                }
                Phase::ReferenceNameLength => {
                    if self.fill(input, 4, left)? {
                        self.remaining = nonnegative(&self.take(), "BAM reference name length")?;
                        if self.remaining == 0 {
                            return Err(invalid("Empty BAM reference name"));
                        }
                        self.phase = Phase::ReferenceName;
                    }
                }
                Phase::ReferenceName => {
                    if self.fill(input, self.remaining, left)? {
                        self.name = nul_string(&self.take())?;
                        self.phase = Phase::ReferenceLength;
                    }
                }
                Phase::ReferenceLength => {
                    if self.fill(input, 4, left)? {
                        let length = nonnegative(&self.take(), "BAM reference length")?;
                        records.add_reference(self.reference, &self.name, length as u64)?;
                        self.reference += 1;
                        if self.reference == self.references {
                            self.end_header(records, input.position())?;
                        } else {
                            self.phase = Phase::ReferenceNameLength;
                        }
                    }
                }
                Phase::RecordSize => {
                    if self.buffer.is_empty() && input.available()?.is_empty() {
                        return Ok(true);
                    }
                    if self.fill(input, 4, left)? {
                        let size = nonnegative(&self.take(), "BAM block size")?;
                        if size < 32 {
                            return Err(invalid(
                                "BAM alignment block is shorter than its fixed fields",
                            ));
                        }
                        self.record_end = input
                            .position()
                            .checked_add(size as u64)
                            .ok_or_else(|| invalid("BAM block position overflow"))?;
                        self.phase = Phase::Core;
                    }
                }
                Phase::Core => {
                    if self.fill(input, 32, left)? {
                        let core = self.take();
                        self.core.copy_from_slice(&core);
                        let bin_mq_nl = u32_at(&self.core, 8);
                        let flag_nc = u32_at(&self.core, 12);
                        self.name_len = (bin_mq_nl & 255) as usize;
                        self.cigar_count = (flag_nc & 65535) as usize;
                        self.sequence_len = nonnegative(&self.core[16..20], "BAM sequence length")?;
                        let required = self.name_len as u64
                            + 4 * self.cigar_count as u64
                            + self.sequence_len.div_ceil(2) as u64
                            + self.sequence_len as u64;
                        if self.name_len == 0 || input.position() + required > self.record_end {
                            return Err(invalid("BAM alignment fields exceed its block length"));
                        }
                        self.sequence_done = 0;
                        self.quality_missing = None;
                        self.cigar.clear();
                        self.fields.clear();
                        self.phase = Phase::Name;
                    }
                }
                Phase::Name => {
                    if self.fill(input, self.name_len, left)? {
                        let name = nul_string(&self.take())?;
                        self.fields = self.core_fields(records)?;
                        records.begin_record(name, self.sequence_len > 0)?;
                        records.set_fields(&self.fields)?;
                        self.phase = Phase::Cigar;
                    }
                }
                Phase::Cigar => {
                    if self.cigar_count == 0 {
                        self.remaining = self.sequence_len.div_ceil(2);
                        self.phase = Phase::Sequence;
                        continue;
                    }
                    if self.fill(input, 4, left)? {
                        let value = u32_at(&self.take(), 0);
                        cigar_op(&mut self.cigar, value)?;
                        self.cigar_count -= 1;
                    }
                }
                Phase::Sequence => {
                    if self.remaining == 0 {
                        self.remaining = self.sequence_len;
                        self.phase = Phase::Quality;
                        continue;
                    }
                    let offset = input.position();
                    let bytes = self.read(input, self.remaining.min(left).min(CHUNK))?;
                    self.remaining -= bytes.len();
                    let decoded: Vec<u8> = bytes
                        .iter()
                        .flat_map(|b| [BAM_BASES[(b >> 4) as usize], BAM_BASES[(b & 15) as usize]])
                        .take(self.sequence_len - self.sequence_done)
                        .collect();
                    self.sequence_done += decoded.len();
                    records.append(
                        &bytes,
                        offset,
                        SpanKind::Sequence,
                        SpanEncoding::BamSequence,
                        &decoded,
                    )?;
                }
                Phase::Quality => {
                    if self.remaining == 0 {
                        self.phase = Phase::AuxHeader;
                        continue;
                    }
                    let offset = input.position();
                    let bytes = self.read(input, self.remaining.min(left).min(CHUNK))?;
                    self.remaining -= bytes.len();
                    let missing = *self.quality_missing.get_or_insert(bytes[0] == 255);
                    if missing {
                        if !bytes.iter().all(|b| *b == 255) {
                            return Err(invalid(
                                "BAM quality contains mixed missing and numeric values",
                            ));
                        }
                    } else {
                        if bytes.iter().any(|b| *b > 93) {
                            return Err(invalid("BAM Phred quality is outside 0..93"));
                        }
                        let decoded: Vec<u8> = bytes.iter().map(|b| b + 33).collect();
                        records.append(
                            &bytes,
                            offset,
                            SpanKind::Quality,
                            SpanEncoding::BamQuality,
                            &decoded,
                        )?;
                    }
                }
                Phase::AuxHeader => {
                    if input.position() == self.record_end && self.buffer.is_empty() {
                        let cigar = if self.cigar.is_empty() {
                            "*".to_owned()
                        } else {
                            self.cigar.clone()
                        };
                        self.fields
                            .iter_mut()
                            .find(|(key, _)| key == "CIGAR")
                            .unwrap()
                            .1 = cigar;
                        records.set_fields(&self.fields)?;
                        records.end_record(input.position())?;
                        self.phase = Phase::RecordSize;
                        self.record_end = 0;
                        continue;
                    }
                    if self.fill(input, 3, left)? {
                        let header = self.take();
                        if !header[0].is_ascii_alphabetic() || !header[1].is_ascii_alphanumeric() {
                            return Err(invalid("Invalid BAM auxiliary tag"));
                        }
                        self.aux_tag = String::from_utf8(header[..2].to_vec()).unwrap();
                        self.aux_type = header[2];
                        self.aux_value.clear();
                        self.phase = match self.aux_type {
                            b'A' | b'c' | b'C' | b's' | b'S' | b'i' | b'I' | b'f' | b'd' => {
                                Phase::AuxScalar
                            }
                            b'Z' | b'H' => Phase::AuxString,
                            b'B' => Phase::AuxArrayHeader,
                            _ => return Err(invalid("Unknown BAM auxiliary value type")),
                        };
                    }
                }
                Phase::AuxScalar => {
                    let size = scalar_size(self.aux_type)?;
                    if self.fill(input, size, left)? {
                        let bytes = self.take();
                        let kind = if b"cCsSiI".contains(&self.aux_type) {
                            'i'
                        } else {
                            char::from(self.aux_type)
                        };
                        self.fields.push((
                            self.aux_tag.clone(),
                            format!("{kind}:{}", scalar_value(self.aux_type, &bytes)?),
                        ));
                        self.phase = Phase::AuxHeader;
                    }
                }
                Phase::AuxString => {
                    if input.position() >= self.record_end {
                        return Err(invalid("Unterminated BAM auxiliary string"));
                    }
                    let remaining = (self.record_end - input.position()) as usize;
                    let available = input.available()?;
                    let max = left.min(CHUNK).min(remaining).min(available.len());
                    if max == 0 {
                        return Err(invalid("Truncated BAM auxiliary string"));
                    }
                    let zero = available[..max].iter().position(|b| *b == 0);
                    let bytes = self.read(input, zero.map_or(max, |p| p + 1))?;
                    let part = if zero.is_some() {
                        &bytes[..bytes.len() - 1]
                    } else {
                        &bytes
                    };
                    self.aux_value.push_str(
                        std::str::from_utf8(part)
                            .map_err(|_| invalid("Non-UTF8 BAM auxiliary string"))?,
                    );
                    if zero.is_some() {
                        self.fields.push((
                            self.aux_tag.clone(),
                            format!("{}:{}", char::from(self.aux_type), self.aux_value),
                        ));
                        self.phase = Phase::AuxHeader;
                    }
                }
                Phase::AuxArrayHeader => {
                    if self.fill(input, 5, left)? {
                        let bytes = self.take();
                        self.aux_subtype = bytes[0];
                        if !b"cCsSiIf".contains(&self.aux_subtype) {
                            return Err(invalid("Invalid BAM auxiliary array subtype"));
                        }
                        self.aux_count = nonnegative(&bytes[1..], "BAM array element count")?;
                        let bytes_needed =
                            self.aux_count as u64 * scalar_size(self.aux_subtype)? as u64;
                        if input.position() + bytes_needed > self.record_end {
                            return Err(invalid("BAM auxiliary array exceeds its alignment block"));
                        }
                        self.aux_value = format!("B:{}", char::from(self.aux_subtype));
                        self.aux_cigar.clear();
                        self.phase = Phase::AuxArray;
                    }
                }
                Phase::AuxArray => {
                    if self.aux_count == 0 {
                        if self.aux_tag == "CG"
                            && self.aux_subtype == b'I'
                            && !self.aux_cigar.is_empty()
                        {
                            self.cigar = std::mem::take(&mut self.aux_cigar);
                        } else {
                            self.fields
                                .push((self.aux_tag.clone(), std::mem::take(&mut self.aux_value)));
                        }
                        self.phase = Phase::AuxHeader;
                        continue;
                    }
                    if self.fill(input, scalar_size(self.aux_subtype)?, left)? {
                        let bytes = self.take();
                        if self.aux_tag == "CG" && self.aux_subtype == b'I' {
                            cigar_op(&mut self.aux_cigar, u32_at(&bytes, 0))?;
                        } else {
                            self.aux_value.push(',');
                            self.aux_value
                                .push_str(&scalar_value(self.aux_subtype, &bytes)?);
                        }
                        self.aux_count -= 1;
                    }
                }
            }
        }
        Ok(false)
    }

    /// Persist the first alignment offset and advance from header parsing to record sizes.
    fn end_header(&mut self, records: &mut impl RecordSink, position: u64) -> io::Result<()> {
        records.finish_header(position)?;
        self.phase = Phase::RecordSize;
        Ok(())
    }

    /// Read a bounded logical-byte fragment, rejecting truncation and alignment-block overruns.
    fn read<R: BufRead>(&self, input: &mut ScanInput<'_, R>, amount: usize) -> io::Result<Vec<u8>> {
        if self.record_end > 0 && input.position() + amount as u64 > self.record_end {
            return Err(invalid("BAM field exceeds its alignment block"));
        }
        let bytes = input.take_bytes(amount)?;
        if bytes.is_empty() {
            return Err(invalid("Truncated BAM structure"));
        }
        Ok(bytes)
    }

    /// Accumulate up to `needed` field bytes within the remaining budget and report completion.
    fn fill<R: BufRead>(
        &mut self,
        input: &mut ScanInput<'_, R>,
        needed: usize,
        budget: usize,
    ) -> io::Result<bool> {
        if self.buffer.len() < needed {
            let bytes = self.read(input, (needed - self.buffer.len()).min(budget))?;
            self.buffer.extend_from_slice(&bytes);
        }
        Ok(self.buffer.len() == needed)
    }

    /// Take the completed field buffer, leaving an empty buffer for the next field.
    fn take(&mut self) -> Vec<u8> {
        std::mem::take(&mut self.buffer)
    }

    /// Translate fixed BAM fields into display metadata, converting positions to SAM coordinates.
    fn core_fields(&self, records: &impl RecordSink) -> io::Result<Vec<(String, String)>> {
        let reference = records.reference_name(i32_at(&self.core, 0))?;
        let mate = records.reference_name(i32_at(&self.core, 20))?;
        let flag = u32_at(&self.core, 12) >> 16;
        let pos = i32_at(&self.core, 4);
        let next = i32_at(&self.core, 24);
        if pos < -1 || next < -1 {
            return Err(invalid("Invalid BAM alignment position"));
        }
        Ok(vec![
            ("FLAG".into(), flag.to_string()),
            ("RNAME".into(), reference),
            ("POS".into(), (i64::from(pos) + 1).to_string()),
            (
                "MAPQ".into(),
                ((u32_at(&self.core, 8) >> 8) & 255).to_string(),
            ),
            ("CIGAR".into(), "*".into()),
            ("RNEXT".into(), mate),
            ("PNEXT".into(), (i64::from(next) + 1).to_string()),
            ("TLEN".into(), i32_at(&self.core, 28).to_string()),
            (
                "Direction".into(),
                if flag & 16 != 0 { "Reverse" } else { "Forward" }.into(),
            ),
        ])
    }
}

/// Decode a UTF-8 BAM name with exactly one terminating NUL and no embedded NULs.
fn nul_string(bytes: &[u8]) -> io::Result<String> {
    let bytes = bytes
        .strip_suffix(&[0])
        .ok_or_else(|| invalid("BAM name lacks terminating NUL"))?;
    if bytes.contains(&0) {
        return Err(invalid("BAM name contains embedded NUL"));
    }
    String::from_utf8(bytes.to_vec()).map_err(|_| invalid("BAM name is not UTF-8"))
}
/// Decode a signed little-endian length or count, rejecting negative values.
fn nonnegative(bytes: &[u8], field: &str) -> io::Result<usize> {
    usize::try_from(i32_at(bytes, 0)).map_err(|_| invalid(format!("Negative {field}")))
}
/// Read a four-byte little-endian unsigned value from an already-sized field buffer.
fn u32_at(bytes: &[u8], offset: usize) -> u32 {
    u32::from_le_bytes(bytes[offset..offset + 4].try_into().unwrap())
}
/// Read a four-byte little-endian signed value from an already-sized field buffer.
fn i32_at(bytes: &[u8], offset: usize) -> i32 {
    i32::from_le_bytes(bytes[offset..offset + 4].try_into().unwrap())
}

/// Append one packed BAM CIGAR operation, rejecting unknown operations and zero lengths.
fn cigar_op(output: &mut String, value: u32) -> io::Result<()> {
    let operation = b"MIDNSHP=X"
        .get((value & 15) as usize)
        .ok_or_else(|| invalid("Unknown BAM CIGAR operation"))?;
    let length = value >> 4;
    if length == 0 {
        return Err(invalid("Zero-length BAM CIGAR operation"));
    }
    output.push_str(&length.to_string());
    output.push(char::from(*operation));
    Ok(())
}
/// Return the byte width of a supported BAM auxiliary scalar type.
fn scalar_size(kind: u8) -> io::Result<usize> {
    match kind {
        b'A' | b'c' | b'C' => Ok(1),
        b's' | b'S' => Ok(2),
        b'i' | b'I' | b'f' => Ok(4),
        b'd' => Ok(8),
        _ => Err(invalid("Unknown BAM value type")),
    }
}
/// Format one little-endian BAM auxiliary scalar as its display value.
fn scalar_value(kind: u8, bytes: &[u8]) -> io::Result<String> {
    Ok(match kind {
        b'A' => char::from(bytes[0]).to_string(),
        b'c' => (bytes[0] as i8).to_string(),
        b'C' => bytes[0].to_string(),
        b's' => i16::from_le_bytes(bytes.try_into().unwrap()).to_string(),
        b'S' => u16::from_le_bytes(bytes.try_into().unwrap()).to_string(),
        b'i' => i32_at(bytes, 0).to_string(),
        b'I' => u32_at(bytes, 0).to_string(),
        b'f' => f32::from_le_bytes(bytes.try_into().unwrap()).to_string(),
        b'd' => f64::from_le_bytes(bytes.try_into().unwrap()).to_string(),
        _ => return Err(invalid("Unknown BAM value type")),
    })
}
