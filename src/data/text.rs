use std::io::{self, BufRead};

use super::{CHUNK, Dataset, Format, invalid};

pub(super) struct TextScanner {
    format: Format,
    line_start: bool,
    phase: Phase,
    text: Vec<u8>,
    comment: bool,
    sam: Sam,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Phase {
    Header,
    Sequence,
    Plus,
    Quality,
    Start,
}

#[derive(Default)]
struct Sam {
    field: usize,
    token: Vec<u8>,
    fields: Vec<String>,
    aux: Vec<(String, String)>,
    header: bool,
    bad: bool,
    sequence_star: bool,
    quality_star: bool,
}

impl TextScanner {
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

    pub fn step(&mut self, data: &mut Dataset, budget: usize) -> io::Result<bool> {
        let begin = data.position;
        while data.position - begin < budget as u64 {
            let offset = data.position;
            let available = data.reader.fill_buf()?;
            if available.is_empty() {
                return self.finish(data);
            }
            let remaining = budget - (data.position - begin) as usize;
            let count = available
                .iter()
                .position(|b| *b == b'\n')
                .map_or(available.len(), |p| p + 1)
                .min(remaining)
                .min(CHUNK);
            let bytes = available[..count].to_vec();
            data.reader.consume(count);
            data.position += count as u64;
            let end_line = bytes.last() == Some(&b'\n');
            match self.format {
                Format::Fasta => self.fasta(data, &bytes, offset, end_line)?,
                Format::Fastq => self.fastq(data, &bytes, offset, end_line)?,
                Format::Sam => self.sam(data, &bytes, offset)?,
                Format::Bam => unreachable!(),
            }
            self.line_start = end_line;
        }
        Ok(false)
    }

    fn fasta(
        &mut self,
        data: &mut Dataset,
        bytes: &[u8],
        offset: u64,
        end: bool,
    ) -> io::Result<()> {
        if self.line_start && bytes.first() == Some(&b'#') {
            data.flush_span()?;
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
            if data.current.is_some() {
                data.end_record(offset)?;
            }
            self.phase = Phase::Header;
            self.text.clear();
        }
        if self.phase == Phase::Header {
            self.text
                .extend_from_slice(if header { &bytes[1..] } else { bytes });
            if end {
                data.begin_record(defline(&self.text)?, true)?;
                self.text.clear();
                self.phase = Phase::Sequence;
            }
        } else {
            let bases = sequence(bytes, false)?;
            if !bases.is_empty() {
                data.append(bytes, offset, 0, 0, &bases)?;
            }
        }
        Ok(())
    }

    fn fastq(
        &mut self,
        data: &mut Dataset,
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
                    data.begin_record(defline(&self.text)?, true)?;
                    self.text.clear();
                    self.phase = Phase::Sequence;
                }
            }
            Phase::Sequence => {
                let bases = sequence(bytes, false)?;
                data.append(bytes, offset, 0, 0, &bases)?;
            }
            Phase::Plus => {
                self.text.extend_from_slice(bytes);
                if end {
                    let plus = defline(&self.text)?;
                    let id = data.current.as_ref().unwrap().id;
                    data.db
                        .execute(
                            "UPDATE records SET plus=? WHERE id=?",
                            rusqlite::params![plus, id],
                        )
                        .map_err(super::sql)?;
                    self.text.clear();
                    self.phase = Phase::Quality;
                }
            }
            Phase::Quality => {
                let quality = quality(bytes)?;
                let current = data.current.as_ref().unwrap();
                if current.qlen + quality.len() as u64 > current.length {
                    return Err(invalid("FASTQ quality is longer than its sequence"));
                }
                data.append(bytes, offset, 1, 0, &quality)?;
                let current = data.current.as_ref().unwrap();
                if end && current.qlen == current.length {
                    data.end_record(data.position)?;
                    self.phase = Phase::Start;
                }
            }
            Phase::Start => unreachable!(),
        }
        Ok(())
    }

    fn sam(&mut self, data: &mut Dataset, bytes: &[u8], offset: u64) -> io::Result<()> {
        let mut begin = 0;
        for (i, byte) in bytes.iter().enumerate() {
            if *byte != b'\t' && *byte != b'\n' {
                continue;
            }
            let part = &bytes[begin..i];
            let result = self
                .sam_part(data, part, offset + begin as u64)
                .and_then(|()| self.sam_field(data, *byte == b'\n'));
            if let Err(error) = result {
                if error.kind() != io::ErrorKind::InvalidData {
                    return Err(error);
                }
                if !self.sam.bad {
                    data.log_error(offset + begin as u64, &format!("SAM: {error}"))?;
                }
                self.sam.bad = true;
                data.discard_current()?;
            }
            if *byte == b'\n' {
                if !self.sam.bad && !self.sam.header && data.current.is_some() {
                    data.end_record(offset + i as u64 + 1)?;
                } else {
                    data.set_meta("resume", &(offset + i as u64 + 1).to_string())?;
                }
                self.sam = Sam::default();
            }
            begin = i + 1;
        }
        if begin < bytes.len()
            && let Err(error) = self.sam_part(data, &bytes[begin..], offset + begin as u64)
        {
            if error.kind() != io::ErrorKind::InvalidData {
                return Err(error);
            }
            if !self.sam.bad {
                data.log_error(offset + begin as u64, &format!("SAM: {error}"))?;
            }
            self.sam.bad = true;
            data.discard_current()?;
        }
        Ok(())
    }

    fn sam_part(&mut self, data: &mut Dataset, part: &[u8], offset: u64) -> io::Result<()> {
        if self.sam.bad || self.sam.header {
            return Ok(());
        }
        let content = part.strip_suffix(b"\r").unwrap_or(part);
        if self.sam.field == 9 {
            if content == b"*" && data.current.as_ref().unwrap().length == 0 {
                self.sam.sequence_star = true;
                data.current.as_mut().unwrap().has_sequence = false;
            } else if !content.is_empty() {
                if self.sam.sequence_star {
                    return Err(invalid("Invalid SEQ missing-value marker"));
                }
                let bases = sequence(content, true)?;
                data.append(content, offset, 0, 0, &bases)?;
            }
        } else if self.sam.field == 10 {
            if content == b"*" && data.current.as_ref().unwrap().qlen == 0 && !self.sam.quality_star
            {
                // A single '*' is missing only when the complete field is one byte.
                self.sam.quality_star = true;
            } else if !content.is_empty() {
                if self.sam.quality_star {
                    data.append(b"*", offset - 1, 1, 0, b"*")?;
                    self.sam.quality_star = false;
                }
                let scores = quality(content)?;
                data.append(content, offset, 1, 0, &scores)?;
            }
        } else {
            self.sam.token.extend_from_slice(content);
        }
        Ok(())
    }

    fn sam_field(&mut self, data: &mut Dataset, end_line: bool) -> io::Result<()> {
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
                data.begin_record(self.sam.fields[0].clone(), true)?;
                data.set_fields(data.current.as_ref().unwrap().id, &attributes)?;
            }
        } else if self.sam.field == 9 {
            let current = data.current.as_ref().unwrap();
            if current.length == 0 && !self.sam.sequence_star {
                return Err(invalid("Empty SEQ field; use * for missing sequence"));
            }
        } else if self.sam.field == 10 {
            let current = data.current.as_ref().unwrap();
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
            data.set_fields(data.current.as_ref().unwrap().id, &fields)?;
        }
        self.sam.field += 1;
        Ok(())
    }

    fn finish(&mut self, data: &mut Dataset) -> io::Result<bool> {
        match self.format {
            Format::Fasta => {
                if self.phase == Phase::Header {
                    data.begin_record(defline(&self.text)?, true)?;
                }
                if data.current.is_some() {
                    data.end_record(data.position)?;
                }
            }
            Format::Fastq => {
                if self.phase != Phase::Start {
                    if self.phase == Phase::Quality
                        && data.current.as_ref().is_some_and(|c| c.length == c.qlen)
                    {
                        data.end_record(data.position)?;
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
                    if let Err(error) = self.sam_field(data, true) {
                        if error.kind() != io::ErrorKind::InvalidData {
                            return Err(error);
                        }
                        if !self.sam.bad {
                            data.log_error(data.position, &format!("SAM: {error}"))?;
                        }
                        self.sam.bad = true;
                        data.discard_current()?;
                    }
                    if !self.sam.bad && data.current.is_some() {
                        data.end_record(data.position)?;
                    }
                }
            }
            Format::Bam => unreachable!(),
        }
        Ok(true)
    }
}

fn defline(bytes: &[u8]) -> io::Result<String> {
    let text = std::str::from_utf8(bytes).map_err(|_| invalid("Header is not valid UTF-8"))?;
    Ok(text.trim_end_matches(['\r', '\n']).to_owned())
}

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
