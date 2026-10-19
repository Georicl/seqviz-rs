//! BAM v1.6 streaming decoder. The outer Source has already decoded BGZF.

use std::io::{self, BufRead};

use rusqlite::{OptionalExtension, params};

use super::{BAM_BASES, CHUNK, Dataset, invalid, sql};

#[derive(Clone, Copy, PartialEq, Eq)]
enum Phase {
    Magic,
    HeaderLength,
    HeaderText,
    ReferenceCount,
    ReferenceNameLength,
    ReferenceName,
    ReferenceLength,
    RecordSize,
    Core,
    Name,
    Cigar,
    Sequence,
    Quality,
    AuxHeader,
    AuxScalar,
    AuxString,
    AuxArrayHeader,
    AuxArray,
}

pub(super) struct BamScanner {
    phase: Phase,
    buffer: Vec<u8>,
    remaining: usize,
    references: i32,
    reference: i32,
    name: String,
    record_end: u64,
    core: [u8; 32],
    name_len: usize,
    cigar_count: usize,
    sequence_len: usize,
    sequence_done: usize,
    quality_missing: Option<bool>,
    cigar: String,
    fields: Vec<(String, String)>,
    aux_tag: String,
    aux_type: u8,
    aux_subtype: u8,
    aux_value: String,
    aux_count: usize,
    aux_cigar: String,
}

impl BamScanner {
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

    pub fn step(&mut self, data: &mut Dataset, budget: usize) -> io::Result<bool> {
        let begin = data.position;
        while data.position - begin < budget as u64 {
            let left = budget - (data.position - begin) as usize;
            match self.phase {
                Phase::Magic => {
                    if self.fill(data, 4, left)? {
                        if self.take().as_slice() != b"BAM\x01" {
                            return Err(invalid("Invalid BAM magic"));
                        }
                        self.phase = Phase::HeaderLength;
                    }
                }
                Phase::HeaderLength => {
                    if self.fill(data, 4, left)? {
                        self.remaining = nonnegative(&self.take(), "BAM header length")?;
                        self.phase = Phase::HeaderText;
                    }
                }
                Phase::HeaderText => {
                    if self.remaining == 0 {
                        self.phase = Phase::ReferenceCount;
                        continue;
                    }
                    let bytes = self.read(data, self.remaining.min(left))?;
                    self.remaining -= bytes.len();
                }
                Phase::ReferenceCount => {
                    if self.fill(data, 4, left)? {
                        self.references = nonnegative(&self.take(), "BAM reference count")? as i32;
                        self.reference = 0;
                        data.db.execute("DELETE FROM refs", []).map_err(sql)?;
                        if self.references == 0 {
                            self.end_header(data)?;
                        } else {
                            self.phase = Phase::ReferenceNameLength;
                        }
                    }
                }
                Phase::ReferenceNameLength => {
                    if self.fill(data, 4, left)? {
                        self.remaining = nonnegative(&self.take(), "BAM reference name length")?;
                        if self.remaining == 0 {
                            return Err(invalid("Empty BAM reference name"));
                        }
                        self.phase = Phase::ReferenceName;
                    }
                }
                Phase::ReferenceName => {
                    if self.fill(data, self.remaining, left)? {
                        self.name = nul_string(&self.take())?;
                        self.phase = Phase::ReferenceLength;
                    }
                }
                Phase::ReferenceLength => {
                    if self.fill(data, 4, left)? {
                        let length = nonnegative(&self.take(), "BAM reference length")?;
                        data.db
                            .execute(
                                "INSERT INTO refs VALUES(?,?,?)",
                                params![self.reference, self.name, length as u64],
                            )
                            .map_err(sql)?;
                        self.reference += 1;
                        if self.reference == self.references {
                            self.end_header(data)?;
                        } else {
                            self.phase = Phase::ReferenceNameLength;
                        }
                    }
                }
                Phase::RecordSize => {
                    if self.buffer.is_empty() && data.reader.fill_buf()?.is_empty() {
                        return Ok(true);
                    }
                    if self.fill(data, 4, left)? {
                        let size = nonnegative(&self.take(), "BAM block size")?;
                        if size < 32 {
                            return Err(invalid(
                                "BAM alignment block is shorter than its fixed fields",
                            ));
                        }
                        self.record_end = data
                            .position
                            .checked_add(size as u64)
                            .ok_or_else(|| invalid("BAM block position overflow"))?;
                        self.phase = Phase::Core;
                    }
                }
                Phase::Core => {
                    if self.fill(data, 32, left)? {
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
                        if self.name_len == 0 || data.position + required > self.record_end {
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
                    if self.fill(data, self.name_len, left)? {
                        let name = nul_string(&self.take())?;
                        self.fields = self.core_fields(data)?;
                        data.begin_record(name, self.sequence_len > 0)?;
                        data.set_fields(data.current.as_ref().unwrap().id, &self.fields)?;
                        self.phase = Phase::Cigar;
                    }
                }
                Phase::Cigar => {
                    if self.cigar_count == 0 {
                        self.remaining = self.sequence_len.div_ceil(2);
                        self.phase = Phase::Sequence;
                        continue;
                    }
                    if self.fill(data, 4, left)? {
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
                    let offset = data.position;
                    let bytes = self.read(data, self.remaining.min(left).min(CHUNK))?;
                    self.remaining -= bytes.len();
                    let decoded: Vec<u8> = bytes
                        .iter()
                        .flat_map(|b| [BAM_BASES[(b >> 4) as usize], BAM_BASES[(b & 15) as usize]])
                        .take(self.sequence_len - self.sequence_done)
                        .collect();
                    self.sequence_done += decoded.len();
                    data.append(&bytes, offset, 0, 1, &decoded)?;
                }
                Phase::Quality => {
                    if self.remaining == 0 {
                        self.phase = Phase::AuxHeader;
                        continue;
                    }
                    let offset = data.position;
                    let bytes = self.read(data, self.remaining.min(left).min(CHUNK))?;
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
                        data.append(&bytes, offset, 1, 2, &decoded)?;
                    }
                }
                Phase::AuxHeader => {
                    if data.position == self.record_end && self.buffer.is_empty() {
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
                        data.set_fields(data.current.as_ref().unwrap().id, &self.fields)?;
                        data.end_record(data.position)?;
                        self.phase = Phase::RecordSize;
                        self.record_end = 0;
                        continue;
                    }
                    if self.fill(data, 3, left)? {
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
                    if self.fill(data, size, left)? {
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
                    if data.position >= self.record_end {
                        return Err(invalid("Unterminated BAM auxiliary string"));
                    }
                    let available = data.reader.fill_buf()?;
                    let max = left
                        .min(CHUNK)
                        .min((self.record_end - data.position) as usize)
                        .min(available.len());
                    if max == 0 {
                        return Err(invalid("Truncated BAM auxiliary string"));
                    }
                    let zero = available[..max].iter().position(|b| *b == 0);
                    let bytes = self.read(data, zero.map_or(max, |p| p + 1))?;
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
                    if self.fill(data, 5, left)? {
                        let bytes = self.take();
                        self.aux_subtype = bytes[0];
                        if !b"cCsSiIf".contains(&self.aux_subtype) {
                            return Err(invalid("Invalid BAM auxiliary array subtype"));
                        }
                        self.aux_count = nonnegative(&bytes[1..], "BAM array element count")?;
                        let bytes_needed =
                            self.aux_count as u64 * scalar_size(self.aux_subtype)? as u64;
                        if data.position + bytes_needed > self.record_end {
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
                    if self.fill(data, scalar_size(self.aux_subtype)?, left)? {
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

    fn end_header(&mut self, data: &mut Dataset) -> io::Result<()> {
        data.set_meta("bam_header", &data.position.to_string())?;
        data.set_meta("resume", &data.position.to_string())?;
        self.phase = Phase::RecordSize;
        Ok(())
    }

    fn read(&self, data: &mut Dataset, amount: usize) -> io::Result<Vec<u8>> {
        if self.record_end > 0 && data.position + amount as u64 > self.record_end {
            return Err(invalid("BAM field exceeds its alignment block"));
        }
        let bytes = data.take_bytes(amount)?;
        if bytes.is_empty() {
            return Err(invalid("Truncated BAM structure"));
        }
        Ok(bytes)
    }

    fn fill(&mut self, data: &mut Dataset, needed: usize, budget: usize) -> io::Result<bool> {
        if self.buffer.len() < needed {
            let bytes = self.read(data, (needed - self.buffer.len()).min(budget))?;
            self.buffer.extend_from_slice(&bytes);
        }
        Ok(self.buffer.len() == needed)
    }

    fn take(&mut self) -> Vec<u8> {
        std::mem::take(&mut self.buffer)
    }

    fn core_fields(&self, data: &Dataset) -> io::Result<Vec<(String, String)>> {
        let reference = reference_name(data, i32_at(&self.core, 0))?;
        let mate = reference_name(data, i32_at(&self.core, 20))?;
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

fn reference_name(data: &Dataset, id: i32) -> io::Result<String> {
    if id == -1 {
        return Ok("*".into());
    }
    data.db
        .query_row("SELECT name FROM refs WHERE id=?", [id], |r| r.get(0))
        .optional()
        .map_err(sql)?
        .ok_or_else(|| invalid("BAM references an unknown reference sequence"))
}

fn nul_string(bytes: &[u8]) -> io::Result<String> {
    let bytes = bytes
        .strip_suffix(&[0])
        .ok_or_else(|| invalid("BAM name lacks terminating NUL"))?;
    if bytes.contains(&0) {
        return Err(invalid("BAM name contains embedded NUL"));
    }
    String::from_utf8(bytes.to_vec()).map_err(|_| invalid("BAM name is not UTF-8"))
}
fn nonnegative(bytes: &[u8], field: &str) -> io::Result<usize> {
    usize::try_from(i32_at(bytes, 0)).map_err(|_| invalid(format!("Negative {field}")))
}
fn u32_at(bytes: &[u8], offset: usize) -> u32 {
    u32::from_le_bytes(bytes[offset..offset + 4].try_into().unwrap())
}
fn i32_at(bytes: &[u8], offset: usize) -> i32 {
    i32::from_le_bytes(bytes[offset..offset + 4].try_into().unwrap())
}

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
fn scalar_size(kind: u8) -> io::Result<usize> {
    match kind {
        b'A' | b'c' | b'C' => Ok(1),
        b's' | b'S' => Ok(2),
        b'i' | b'I' | b'f' => Ok(4),
        b'd' => Ok(8),
        _ => Err(invalid("Unknown BAM value type")),
    }
}
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
