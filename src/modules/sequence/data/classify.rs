//! Content and sequence-alphabet recognition.
use super::{Format, SeqType, invalid};
use std::{io, path::Path};

/// Recognize decoded BAM, FASTA, FASTQ or SAM content, using the path only for empty input.
pub(super) fn detect_format(prefix: &[u8], path: &Path) -> io::Result<Format> {
    if prefix.starts_with(b"BAM\x01") {
        return Ok(Format::Bam);
    }
    let mut content = prefix;
    let mut comment = false;
    loop {
        if content.starts_with(b"#") {
            comment = true;
            content = content
                .iter()
                .position(|b| *b == b'\n')
                .map_or(&[][..], |n| &content[n + 1..]);
            continue;
        }
        if let Some(position) = content.iter().position(|b| !b.is_ascii_whitespace()) {
            if position > 0 {
                content = &content[position..];
                continue;
            }
        } else {
            content = &[];
        }
        break;
    }
    let prefix = content;
    if prefix.starts_with(b">") {
        return Ok(Format::Fasta);
    }
    if [b"@HD\t", b"@SQ\t", b"@RG\t", b"@PG\t", b"@CO\t"]
        .iter()
        .any(|mark| prefix.starts_with(*mark))
    {
        return Ok(Format::Sam);
    }
    if prefix.starts_with(b"@") {
        return Ok(Format::Fastq);
    }
    if prefix
        .iter()
        .take_while(|b| **b != b'\n')
        .filter(|b| **b == b'\t')
        .count()
        >= 2
    {
        return Ok(Format::Sam);
    }
    if prefix.is_empty() {
        if comment {
            return Ok(Format::Fasta);
        }
        let name = path.to_string_lossy().to_ascii_lowercase();
        let name = name.strip_suffix(".gz").unwrap_or(&name);
        if name.ends_with(".sam") {
            return Ok(Format::Sam);
        }
        if name.ends_with(".fq") || name.ends_with(".fastq") {
            return Ok(Format::Fastq);
        }
        if [".fa", ".fasta", ".fna", ".fas", ".fsa", ".faa", ".pep"]
            .iter()
            .any(|ext| name.ends_with(ext))
        {
            return Ok(Format::Fasta);
        }
    }
    Err(invalid(
        "Input is not recognised as FASTA, FASTQ, SAM or BAM",
    ))
}
/// Classify a sequence sample by its letters, requiring a protein-specific residue for protein.
pub(super) fn detect_type(sample: &[u8]) -> SeqType {
    let non_dna: Vec<u8> = sample
        .iter()
        .map(u8::to_ascii_uppercase)
        .filter(|b| !b"ATCGUNRYWSMKBDHV-.".contains(b))
        .collect();
    if non_dna.is_empty() {
        SeqType::Dna
    } else if non_dna.iter().all(|b| b"EFILPQXJZO*".contains(b))
        && non_dna.iter().any(|b| b"EFILPQ".contains(b))
    {
        SeqType::Protein
    } else {
        SeqType::Unknown
    }
}
