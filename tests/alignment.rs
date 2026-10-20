//! Independent SAM/BAM acceptance fixtures generated using samtools 1.21.
//!
//! `alignments.bam` is `samtools view --no-PG -b test/alignments.sam`.
//! `spanning.bam` and `long-cigar.bam` each contain one deterministic record
//! produced by `generated_sam` below, converted with the same command. Their
//! read-back SEQ, QUAL and CIGAR were checked with `samtools view`, and all BAM
//! fixtures passed `samtools quickcheck`. Tests require no external commands.
//! SAM v1.6: https://samtools.github.io/hts-specs/SAMv1.pdf

use std::{
    io,
    path::{Path, PathBuf},
    sync::atomic::AtomicBool,
};

use seqviz::data::{Dataset, Format, RecordMeta, SeqType};

fn fixture(name: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("test")
        .join(name)
}

fn scan(path: &Path, cache: &Path, budget: usize) -> Dataset {
    let mut dataset = Dataset::open(path, cache).unwrap();
    for _ in 0..20_000 {
        let progress = dataset.step(budget).unwrap();
        assert!(
            progress.error.is_none(),
            "{}: {:?}",
            path.display(),
            progress.error
        );
        if progress.complete {
            assert_eq!(progress.errors, 0, "{}", path.display());
            return dataset;
        }
    }
    panic!("{} did not finish indexing", path.display());
}

fn field<'a>(record: &'a RecordMeta, key: &str) -> &'a str {
    record
        .fields
        .iter()
        .find(|(name, _)| name == key)
        .map(|(_, value)| value.as_str())
        .unwrap_or_else(|| panic!("{} is missing {key}: {:?}", record.name, record.fields))
}

fn close(left: f64, right: f64) {
    assert!((left - right).abs() < 1e-10, "{left} != {right}");
}

fn sam_records() -> Vec<Vec<&'static str>> {
    include_str!("../test/alignments.sam")
        .lines()
        .filter(|line| !line.starts_with('@'))
        .map(|line| line.split('\t').collect())
        .collect()
}

#[test]
fn sam_and_bam_preserve_record_identity_alignment_fields_and_saved_orientation() {
    let expected = sam_records();
    for (name, format) in [
        ("alignments.sam", Format::Sam),
        ("alignments.bam", Format::Bam),
    ] {
        let cache = tempfile::tempdir().unwrap();
        let mut dataset = scan(&fixture(name), cache.path(), 37);
        assert_eq!(dataset.format(), format);
        assert_eq!(dataset.progress().records as usize, expected.len());
        let summaries = dataset.records(0, expected.len() + 2).unwrap();
        assert_eq!(summaries.len(), expected.len());
        for (id, columns) in expected.iter().enumerate() {
            let record = dataset.record(id as u64).unwrap().unwrap();
            assert_eq!(record.name, columns[0], "{name} record {id}");
            assert!(record.complete);
            assert_eq!(summaries[id].id, id as u64);
            assert_eq!(summaries[id].name, columns[0]);
            for (key, column) in [
                ("FLAG", 1),
                ("RNAME", 2),
                ("POS", 3),
                ("MAPQ", 4),
                ("CIGAR", 5),
                ("RNEXT", 6),
                ("PNEXT", 7),
                ("TLEN", 8),
            ] {
                // BAM stores reference IDs, so a same-reference mate may be rendered as its name.
                let actual = field(&record, key);
                if key == "RNEXT" && columns[column] == "=" {
                    assert!(
                        actual == "=" || actual == columns[2],
                        "{name} {key}: {actual}"
                    );
                } else {
                    assert_eq!(actual, columns[column], "{name} record {id} {key}");
                }
            }
            let flag: u16 = columns[1].parse().unwrap();
            assert_eq!(
                field(&record, "Direction"),
                if flag & 16 == 0 { "Forward" } else { "Reverse" }
            );
            let sequence = if columns[9] == "*" { "" } else { columns[9] };
            let quality = if columns[10] == "*" { "" } else { columns[10] };
            assert_eq!(record.has_sequence, columns[9] != "*");
            assert_eq!(record.has_quality, columns[10] != "*");
            assert_eq!(record.length, sequence.len() as u64);
            assert_eq!(record.quality_length, quality.len() as u64);
            assert_eq!(record.seq_type, SeqType::Dna);
            let window = dataset
                .read_range(id as u64, 0, sequence.len() + 10)
                .unwrap();
            assert_eq!(
                window.sequence,
                sequence.as_bytes(),
                "{name} record {id} SEQ"
            );
            assert_eq!(
                window.quality,
                quality.as_bytes(),
                "{name} record {id} QUAL"
            );
            if sequence.is_empty() {
                assert_eq!(record.gc, None);
            } else {
                let gc = sequence
                    .bytes()
                    .filter(|b| matches!(b, b'G' | b'C'))
                    .count();
                close(
                    record.gc.unwrap(),
                    100.0 * gc as f64 / sequence.len() as f64,
                );
            }
            if quality.is_empty() {
                assert!(record.quality.is_none());
            } else {
                let stats = record.quality.unwrap();
                let scores: Vec<_> = quality.bytes().map(|b| b - 33).collect();
                assert_eq!(stats.min, *scores.iter().min().unwrap());
                assert_eq!(stats.max, *scores.iter().max().unwrap());
                close(
                    stats.mean,
                    scores.iter().map(|q| f64::from(*q)).sum::<f64>() / scores.len() as f64,
                );
                close(
                    stats.q30,
                    100.0 * scores.iter().filter(|q| **q >= 30).count() as f64
                        / scores.len() as f64,
                );
            }
        }
        assert_eq!(
            summaries.iter().filter(|r| r.name == "duplicate").count(),
            4
        );
        assert_eq!(dataset.find_name("DUPLIC", 0).unwrap(), Some(0));
        assert_eq!(dataset.find_name("duplic", 1).unwrap(), Some(1));
        assert_eq!(dataset.find_name("duplic", 3).unwrap(), Some(3));
        assert_eq!(dataset.find_name("duplic", 4).unwrap(), None);
    }
}

#[test]
fn alignment_statistics_include_duplicate_alignments_and_exclude_missing_seq_from_lengths() {
    let expected = sam_records();
    let sequences: Vec<_> = expected
        .iter()
        .filter(|row| row[9] != "*")
        .map(|row| row[9])
        .collect();
    let total = sequences.iter().map(|s| s.len()).sum::<usize>();
    let gc = sequences
        .iter()
        .flat_map(|s| s.bytes())
        .filter(|b| matches!(b, b'G' | b'C'))
        .count();
    for name in ["alignments.sam", "alignments.bam"] {
        let cache = tempfile::tempdir().unwrap();
        let dataset = scan(&fixture(name), cache.path(), 4096);
        let stats = dataset.stats().unwrap();
        assert!(stats.complete);
        assert_eq!(stats.errors, 0);
        assert_eq!(stats.records as usize, expected.len());
        assert_eq!(stats.sequence_records as usize, sequences.len());
        assert_eq!(stats.total_length as usize, total);
        assert_eq!(
            stats.min_length,
            sequences.iter().map(|s| s.len() as u64).min()
        );
        assert_eq!(
            stats.max_length,
            sequences.iter().map(|s| s.len() as u64).max()
        );
        close(
            stats.mean_length.unwrap(),
            total as f64 / sequences.len() as f64,
        );
        close(stats.gc.unwrap(), 100.0 * gc as f64 / total as f64);
        assert_eq!(stats.n50, None);
        assert!(!stats.protein);
    }
}

#[test]
fn alignment_copy_is_rejected_at_the_dataset_boundary() {
    for name in ["alignments.sam", "alignments.bam"] {
        let cache = tempfile::tempdir().unwrap();
        let mut dataset = scan(&fixture(name), cache.path(), 4096);
        assert!(!dataset.format().can_copy());
        let error = dataset.copy_record(0, &AtomicBool::new(false)).unwrap_err();
        assert_eq!(error.kind(), io::ErrorKind::PermissionDenied);
    }
}

#[test]
fn bam_optional_fields_cover_scalar_integer_float_string_hex_and_numeric_arrays() {
    let cache = tempfile::tempdir().unwrap();
    let dataset = scan(&fixture("alignments.bam"), cache.path(), 4096);
    let record = dataset.record(0).unwrap().unwrap();
    for (key, value) in [
        ("NM", "i:2"),
        ("RG", "Z:group1"),
        ("XA", "A:Q"),
        ("XB", "B:c,-128,0,127"),
        ("XC", "B:C,0,255"),
        ("XD", "B:s,-32768,32767"),
        ("XE", "B:S,0,65535"),
        ("XF", "B:i,-2147483648,2147483647"),
        ("XG", "B:I,0,4294967295"),
        ("XH", "H:0A7F"),
        ("XI", "f:1.5"),
        ("XJ", "B:f,-0.25,3.5"),
    ] {
        assert_eq!(field(&record, key), value, "tag {key}");
    }
    assert_eq!(field(&dataset.record(2).unwrap().unwrap(), "AS"), "i:-10");
}

fn generated_sequence(length: usize) -> Vec<u8> {
    b"ACGTN".iter().copied().cycle().take(length).collect()
}

fn generated_quality(length: usize) -> Vec<u8> {
    (0..length).map(|i| 33 + (i % 94) as u8).collect()
}

fn generated_sam(name: &str, length: usize, cigar: &str) -> String {
    format!(
        "@HD\tVN:1.6\tSO:unsorted\n@SQ\tSN:chr1\tLN:1000000\n{name}\t16\tchr1\t31\t60\t{cigar}\t*\t0\t0\t{}\t{}\n",
        String::from_utf8(generated_sequence(length)).unwrap(),
        String::from_utf8(generated_quality(length)).unwrap()
    )
}

#[test]
fn long_sequence_ranges_cross_bgzf_and_index_spans_and_survive_reopening() {
    let length = 150_003;
    let sequence = generated_sequence(length);
    let quality = generated_quality(length);
    let root = tempfile::tempdir().unwrap();
    let sam = root.path().join("spanning.sam");
    std::fs::write(&sam, generated_sam("spanning", length, "150003M")).unwrap();
    for path in [sam, fixture("spanning.bam")] {
        let cache = tempfile::tempdir().unwrap();
        let mut dataset = scan(&path, cache.path(), 4096);
        for &(start, count) in &[
            (0, 68_000),
            (1, 5),
            (32_767, 5000),
            (65_529, 68_000),
            (98_301, 30_000),
            (150_000, 20),
        ] {
            let window = dataset.read_range(0, start as u64, count).unwrap();
            let end = (start + count).min(length);
            assert_eq!(window.start, start as u64);
            assert_eq!(
                window.sequence,
                sequence[start..end],
                "{} sequence {start}",
                path.display()
            );
            assert_eq!(
                window.quality,
                quality[start..end],
                "{} quality {start}",
                path.display()
            );
        }
        drop(dataset);
        let mut reopened = Dataset::open(&path, cache.path()).unwrap();
        assert!(reopened.progress().complete);
        let window = reopened.read_range(0, 120_001, 9000).unwrap();
        assert_eq!(window.sequence, sequence[120_001..129_001]);
        assert_eq!(window.quality, quality[120_001..129_001]);
    }
}

#[test]
fn bam_long_cigar_uses_cg_array_instead_of_the_placeholder_cigar() {
    let cache = tempfile::tempdir().unwrap();
    let mut dataset = scan(&fixture("long-cigar.bam"), cache.path(), 4096);
    let record = dataset.record(0).unwrap().unwrap();
    assert_eq!(record.length, 65_536);
    assert_eq!(field(&record, "CIGAR"), "1M".repeat(65_536));
    let range = dataset.read_range(0, 65_520, 30).unwrap();
    assert_eq!(range.sequence, generated_sequence(65_536)[65_520..]);
    assert_eq!(range.quality, generated_quality(65_536)[65_520..]);
}

#[test]
fn interrupted_bam_index_resumes_without_duplicate_or_truncated_record() {
    let cache = tempfile::tempdir().unwrap();
    let path = fixture("spanning.bam");
    let mut partial = Dataset::open(&path, cache.path()).unwrap();
    for _ in 0..3 {
        let progress = partial.step(4096).unwrap();
        assert!(progress.error.is_none(), "{:?}", progress.error);
    }
    assert!(!partial.progress().complete);
    drop(partial);
    let mut resumed = scan(&path, cache.path(), 4096);
    assert_eq!(resumed.progress().records, 1);
    let record = resumed.record(0).unwrap().unwrap();
    assert_eq!(record.name, "spanning");
    assert_eq!(record.length, 150_003);
    assert_eq!(record.quality_length, 150_003);
    assert_eq!(field(&record, "CIGAR"), "150003M");
    let window = resumed.read_range(0, 7_003, 77_777).unwrap();
    assert_eq!(window.sequence, generated_sequence(150_003)[7_003..84_780]);
    assert_eq!(window.quality, generated_quality(150_003)[7_003..84_780]);
}

#[test]
fn gzip_sam_retains_missing_values_and_sam_statistics() {
    use std::io::Write;

    let files = tempfile::tempdir().unwrap();
    let path = files.path().join("alignments.sam.gz");
    let mut output = flate2::write::GzEncoder::new(
        std::fs::File::create(&path).unwrap(),
        flate2::Compression::default(),
    );
    output
        .write_all(include_bytes!("../test/alignments.sam"))
        .unwrap();
    output.finish().unwrap();
    let cache = tempfile::tempdir().unwrap();
    let mut compressed = scan(&path, cache.path(), 4096);
    let plain_cache = tempfile::tempdir().unwrap();
    let plain = scan(&fixture("alignments.sam"), plain_cache.path(), 4096);
    assert_eq!(compressed.format(), Format::Sam);
    assert_eq!(compressed.progress().records, plain.progress().records);
    assert!(!compressed.record(4).unwrap().unwrap().has_sequence);
    assert!(!compressed.record(5).unwrap().unwrap().has_quality);
    assert_eq!(compressed.read_range(5, 0, 99).unwrap().sequence, b"GCGNN");
    assert_eq!(
        compressed.stats().unwrap().total_length,
        plain.stats().unwrap().total_length
    );
    assert_eq!(compressed.stats().unwrap().gc, plain.stats().unwrap().gc);
    assert_eq!(compressed.stats().unwrap().n50, None);
}

#[test]
fn sam_star_quality_characters_split_at_chunk_boundaries_are_not_missing_quality() {
    let files = tempfile::tempdir().unwrap();
    let path = files.path().join("star-quality.sam");
    std::fs::write(&path, "@SQ\tSN:chr1\tLN:100\nall-star\t0\tchr1\t1\t60\t4M\t*\t0\t0\tACGT\t****\nstarts-star\t0\tchr1\t5\t60\t4M\t*\t0\t0\tACGT\t**I?\n").unwrap();
    let cache = tempfile::tempdir().unwrap();
    let mut dataset = scan(&path, cache.path(), 1);
    assert_eq!(dataset.progress().records, 2);
    for (id, expected) in [(0, b"****"), (1, b"**I?")] {
        let record = dataset.record(id).unwrap().unwrap();
        assert!(record.has_quality);
        assert_eq!(record.quality_length, 4);
        assert_eq!(dataset.read_range(id, 0, 4).unwrap().quality, expected);
    }
}
