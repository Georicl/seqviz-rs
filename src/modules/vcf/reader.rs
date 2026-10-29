//! Header preservation and lazy-record validation over decoded VCF text.
use super::graph::inspect_graph;
use super::model::*;
use noodles_vcf as vcf;
use std::io::{self, BufRead};

/// Create an invalid-data error for malformed VCF input.
pub(crate) fn invalid(message: impl Into<String>) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, message.into())
}

/// Read through #CHROM and return the parsed header, decoded byte length and line count.
/// Preserve original line endings so the first record offset remains exact.
pub(crate) fn read_header(input: &mut impl BufRead) -> io::Result<(VcfHeader, u64, u64)> {
    let mut raw = Vec::new();
    let mut line = Vec::new();
    let mut line_number = 0;
    loop {
        line.clear();
        if input.read_until(b'\n', &mut line)? == 0 {
            return Err(invalid("VCF header is missing #CHROM columns"));
        }
        line_number += 1;
        let text = std::str::from_utf8(&line)
            .map_err(|e| invalid(format!("VCF header line {line_number}: {e}")))?;
        if !text.starts_with("##") && !text.starts_with("#CHROM\t") {
            return Err(invalid(format!(
                "VCF line {line_number}: expected a header line"
            )));
        }
        raw.extend_from_slice(&line);
        if text.starts_with("#CHROM\t") {
            break;
        }
    }
    let length = raw.len() as u64;
    let raw = String::from_utf8(raw).map_err(|e| invalid(e.to_string()))?;
    let header = parse_header(raw)?;
    Ok((header, length, line_number))
}

/// Validate VCF 4.0-4.5 headers and extract metadata without rewriting their text.
fn parse_header(raw: String) -> io::Result<VcfHeader> {
    let first = raw.lines().next().unwrap_or_default();
    let version = first
        .strip_prefix("##fileformat=VCFv")
        .ok_or_else(|| invalid("VCF header must begin with ##fileformat=VCFv4.x"))?;
    let (major, minor) = version
        .split_once('.')
        .ok_or_else(|| invalid("Invalid VCF version"))?;
    let minor: u32 = minor.parse().map_err(|_| invalid("Invalid VCF version"))?;
    if major != "4" || minor > 5 {
        return Err(invalid(format!("Unsupported VCF version: VCFv{version}")));
    }
    // Validate the header grammar without replacing the original representation.
    let validation = if raw.ends_with('\n') {
        raw.clone()
    } else {
        format!("{raw}\n")
    };
    if let Err(error) = vcf::io::Reader::new(validation.as_bytes()).read_header()
        && (minor != 5 || !validate_local_format_header(&validation)?)
    {
        return Err(invalid(format!("VCF header: {error}")));
    }
    let columns = raw
        .lines()
        .last()
        .unwrap_or_default()
        .split('\t')
        .collect::<Vec<_>>();
    let fixed = [
        "#CHROM", "POS", "ID", "REF", "ALT", "QUAL", "FILTER", "INFO",
    ];
    if columns.len() < fixed.len() || columns[..8] != fixed {
        return Err(invalid(
            "VCF column header must contain CHROM, POS, ID, REF, ALT, QUAL, FILTER and INFO",
        ));
    }
    if columns.len() > 8 && columns[8] != "FORMAT" {
        return Err(invalid("VCF sample columns require a FORMAT column"));
    }
    let mut sample_names = std::collections::HashSet::new();
    for sample in columns.iter().skip(9) {
        if sample.is_empty()
            || sample.bytes().any(|b| b.is_ascii_whitespace())
            || !sample_names.insert(*sample)
        {
            return Err(invalid("VCF sample names must be nonempty and unique"));
        }
    }
    let mut header = VcfHeader {
        version: format!("VCFv{version}"),
        samples: columns
            .iter()
            .skip(9)
            .map(|name| (*name).to_owned())
            .collect(),
        raw: raw.clone(),
        ..VcfHeader::default()
    };
    for line in raw.lines().skip(1).filter(|line| line.starts_with("##")) {
        let (kind, value) = line[2..]
            .split_once('=')
            .filter(|(kind, _)| !kind.is_empty())
            .ok_or_else(|| invalid("VCF metadata must use ##key=value"))?;
        match kind {
            "source" => header.source = Some(value.to_owned()),
            "reference" => header.reference = Some(value.to_owned()),
            _ => {}
        }
        if !value.starts_with('<') {
            continue;
        }
        if !value.ends_with('>') {
            return Err(invalid(format!("Unclosed VCF {kind} metadata")));
        }
        let fields = metadata_fields(&value[1..value.len() - 1]);
        let get = |key: &str| {
            fields
                .iter()
                .find(|(name, _)| name == key)
                .map(|(_, value)| value.clone())
        };
        let Some(id) = get("ID") else { continue };
        if kind == "contig" && !header.contigs.contains(&id) {
            header.contigs.push(id.clone());
        }
        header.definitions.push(FieldDefinition {
            kind: kind.to_owned(),
            id,
            number: get("Number"),
            value_type: get("Type"),
            description: get("Description"),
            raw: line.to_owned(),
        });
    }
    Ok(header)
}

/// Split metadata on unquoted commas while respecting quoted backslash escapes.
fn metadata_ranges(text: &str) -> Vec<std::ops::Range<usize>> {
    let mut boundaries = vec![0];
    let mut quoted = false;
    let mut escaped = false;
    for (i, byte) in text.bytes().enumerate() {
        if escaped {
            escaped = false;
            continue;
        }
        if byte == b'\\' && quoted {
            escaped = true;
            continue;
        }
        if byte == b'"' {
            quoted = !quoted;
        }
        if byte == b',' && !quoted {
            boundaries.push(i + 1);
        }
    }
    boundaries
        .iter()
        .enumerate()
        .map(|(i, &start)| start..boundaries.get(i + 1).map_or(text.len(), |next| next - 1))
        .collect()
}

/// Extract metadata key/value pairs and unescape quoted string values.
fn metadata_fields(text: &str) -> Vec<(String, String)> {
    metadata_ranges(text)
        .into_iter()
        .filter_map(|range| {
            let (name, raw) = text[range].split_once('=')?;
            let value = raw
                .strip_prefix('"')
                .and_then(|s| s.strip_suffix('"'))
                .unwrap_or(raw)
                .replace("\\\"", "\"")
                .replace("\\\\", "\\");
            Some((name.to_owned(), value))
        })
        .collect()
}

/// noodles 0.94 defines the VCF 4.5 local-number variants but its text parser
/// lacks their tokens. Validate those FORMAT lines separately, retaining the
/// library's grammar and reserved Number/Type checks rather than ignoring errors.
fn validate_local_format_header(raw: &str) -> io::Result<bool> {
    use vcf::header::{
        FileFormat,
        record::value::{
            Map,
            map::{
                Format,
                format::{Number, Type},
            },
        },
    };
    let mut validation = String::new();
    let mut adapted = false;
    let mut ids = std::collections::HashSet::new();
    for line in raw.lines() {
        let Some(body) = line
            .strip_prefix("##FORMAT=<")
            .and_then(|s| s.strip_suffix('>'))
        else {
            validation.push_str(line);
            validation.push('\n');
            continue;
        };
        let fields = metadata_fields(body);
        let get = |key: &str| {
            fields
                .iter()
                .find(|(name, _)| name == key)
                .map(|(_, value)| value.as_str())
        };
        let Some(id) = get("ID") else {
            return Err(invalid("VCF FORMAT is missing ID"));
        };
        if !ids.insert(id.to_owned()) {
            return Err(invalid(format!("Duplicate VCF FORMAT ID: {id}")));
        }
        let number = match get("Number") {
            Some("LA") => Number::LocalAlternateBases,
            Some("LR") => Number::LocalReferenceAlternateBases,
            Some("LG") => Number::LocalSamples,
            Some("M") => Number::BaseModifications,
            _ => {
                validation.push_str(line);
                validation.push('\n');
                continue;
            }
        };
        let ty = get("Type")
            .and_then(|value| value.parse::<Type>().ok())
            .ok_or_else(|| invalid("Invalid FORMAT Type"))?;
        let expected = Map::<Format>::from((FileFormat::new(4, 5), id));
        if !expected.description().is_empty()
            && (expected.number() != number || expected.ty() != ty)
        {
            return Err(invalid(format!(
                "Invalid Number or Type for reserved FORMAT {id}"
            )));
        }
        let range = metadata_ranges(body)
            .into_iter()
            .find(|range| body[range.clone()].starts_with("Number="))
            .ok_or_else(|| invalid("FORMAT is missing Number"))?;
        if body[range.clone()].contains('"') {
            return Err(invalid("FORMAT Number must be unquoted"));
        }
        let normalized = format!(
            "##fileformat=VCFv4.2\n##FORMAT=<{}Number=.{}>\n#CHROM\tPOS\tID\tREF\tALT\tQUAL\tFILTER\tINFO\n",
            &body[..range.start],
            &body[range.end..]
        );
        vcf::io::Reader::new(normalized.as_bytes())
            .read_header()
            .map_err(|error| invalid(format!("VCF FORMAT {id}: {error}")))?;
        adapted = true;
    }
    if adapted {
        vcf::io::Reader::new(validation.as_bytes())
            .read_header()
            .map_err(|error| invalid(format!("VCF header: {error}")))?;
    }
    Ok(adapted)
}

/// Validate one record against its header, preserving raw fields for lazy detail access.
/// Classify each ALT independently without expanding INFO or sample values into stored objects.
pub(crate) fn parse_record(bytes: &[u8], header: &VcfHeader) -> io::Result<VcfEntry> {
    let text =
        std::str::from_utf8(bytes).map_err(|e| invalid(format!("VCF record is not UTF-8: {e}")))?;
    let raw = text.strip_suffix('\n').unwrap_or(text);
    let raw = raw.strip_suffix('\r').unwrap_or(raw);
    if raw.bytes().any(|byte| matches!(byte, b'\r' | b'\n' | 0)) {
        return Err(invalid("Unexpected line break or NUL inside a VCF record"));
    }
    vcf::Record::try_from(raw.as_bytes())
        .map_err(|error| invalid(format!("VCF record: {error}")))?;
    let fields = raw.split('\t').take(9).collect::<Vec<_>>();
    if fields.len() < 8 {
        return Err(invalid("VCF record has fewer than eight fields"));
    }
    let column_count = raw.bytes().filter(|b| *b == b'\t').count() + 1;
    let has_format = !header.samples.is_empty()
        || header
            .raw
            .trim_end_matches(['\r', '\n'])
            .ends_with("\tFORMAT");
    let expected = if has_format {
        9 + header.samples.len()
    } else {
        8
    };
    if column_count != expected {
        return Err(invalid(format!(
            "VCF record has {column_count} columns; header declares {expected}"
        )));
    }
    if fields[0].is_empty() || fields[0].bytes().any(|b| b.is_ascii_whitespace()) {
        return Err(invalid("Invalid CHROM field"));
    }
    let pos = fields[1]
        .parse::<u64>()
        .map_err(|_| invalid("Invalid POS field"))?;
    if fields[2].is_empty() {
        return Err(invalid("Empty ID field; use '.' for missing values"));
    }
    if fields[3].is_empty()
        || fields[3] == "."
        || !fields[3].bytes().all(|b| b.is_ascii_alphabetic())
    {
        return Err(invalid("Invalid REF field"));
    }
    if fields[4].is_empty()
        || fields[4]
            .split(',')
            .any(|alt| alt.is_empty() || alt.bytes().any(|b| b.is_ascii_whitespace()))
    {
        return Err(invalid("Invalid ALT field"));
    }
    let qual = if fields[5] == "." {
        None
    } else {
        let value = fields[5]
            .parse::<f64>()
            .map_err(|_| invalid("Invalid QUAL field"))?;
        if !value.is_finite() || value < 0.0 {
            return Err(invalid("Invalid QUAL field"));
        }
        Some(value)
    };
    if fields[6].is_empty() || fields[7].is_empty() {
        return Err(invalid(
            "Empty FILTER or INFO field; use '.' for missing values",
        ));
    }
    let format = fields
        .get(8)
        .filter(|value| **value != ".")
        .map(|value| value.split(':').map(str::to_owned).collect::<Vec<_>>())
        .unwrap_or_default();
    if format.iter().any(String::is_empty) {
        return Err(invalid("Empty FORMAT key"));
    }
    for (index, sample) in raw.split('\t').skip(9).enumerate() {
        if sample.is_empty() || sample.bytes().any(|byte| byte.is_ascii_whitespace()) {
            return Err(invalid(format!("Invalid sample column {}", index + 1)));
        }
        if sample != "." && sample.split(':').count() > format.len() {
            return Err(invalid(format!(
                "Sample column {} has more values than FORMAT keys",
                index + 1
            )));
        }
    }
    let record = VcfRecord {
        raw: raw.to_owned(),
        chrom: fields[0].to_owned(),
        pos,
        ids: if fields[2] == "." {
            Vec::new()
        } else {
            fields[2].split(';').map(str::to_owned).collect()
        },
        reference: fields[3].to_owned(),
        alternates: if fields[4] == "." {
            Vec::new()
        } else {
            fields[4].split(',').map(str::to_owned).collect()
        },
        qual,
        filter: fields[6].to_owned(),
        format,
    };
    let variants = record
        .alternates
        .iter()
        .enumerate()
        .map(|(i, alternate)| {
            let kind = classify_variant(&record.reference, alternate);
            VariantInfo {
                allele_index: i + 1,
                alternate: alternate.clone(),
                kind,
                length_change: matches!(
                    kind,
                    VariantKind::Snv
                        | VariantKind::Mnv
                        | VariantKind::Insertion
                        | VariantKind::Deletion
                )
                .then_some(alternate.len() as i64 - record.reference.len() as i64),
            }
        })
        .collect();
    let graph = inspect_graph(header, &record);
    Ok(VcfEntry {
        record,
        variants,
        graph,
    })
}

/// Infer an allele display kind, recognising symbolic alleles and both breakend forms.
pub(crate) fn classify_variant(reference: &str, alternate: &str) -> VariantKind {
    if alternate == "*" {
        VariantKind::SpanningDeletion
    } else if alternate.contains(['[', ']'])
        || (alternate.len() > 1 && (alternate.starts_with('.') || alternate.ends_with('.')))
    {
        VariantKind::Breakend
    } else if alternate.starts_with('<') && alternate.ends_with('>') {
        VariantKind::Symbolic
    } else if alternate == "." {
        VariantKind::Other
    } else if reference.len() == 1 && alternate.len() == 1 {
        VariantKind::Snv
    } else if alternate.len() > reference.len() {
        VariantKind::Insertion
    } else if alternate.len() < reference.len() {
        VariantKind::Deletion
    } else {
        VariantKind::Mnv
    }
}
