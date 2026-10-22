use super::{
    backend::Backend,
    data::Dataset,
    model::*,
    protocol::*,
    reader::{parse_record, read_header},
};
use crate::{core::tasks::Driver, modules::session::SessionFiles};
use flate2::{
    Compression,
    write::{DeflateEncoder, GzEncoder},
};
use std::{
    fs,
    io::{Cursor, Write},
    path::{Path, PathBuf},
    sync::mpsc,
    time::Duration,
};
use tempfile::TempDir;

const HEADER: &str = "##fileformat=VCFv4.3\n##contig=<ID=chr1,length=10000>\n##contig=<ID=chr2,length=10000>\n#CHROM\tPOS\tID\tREF\tALT\tQUAL\tFILTER\tINFO\n";

fn header(raw: &str) -> VcfHeader {
    read_header(&mut Cursor::new(raw)).unwrap().0
}
fn record(chrom: &str, pos: u64) -> String {
    format!("{chrom}\t{pos}\trs{pos}\tA\tT\t40\tPASS\tDP=12\n")
}
fn fixture(bytes: &[u8]) -> (TempDir, PathBuf, PathBuf) {
    let directory = TempDir::new().unwrap();
    let path = directory.path().join("input.vcf");
    let cache = directory.path().join("cache");
    fs::write(&path, bytes).unwrap();
    (directory, path, cache)
}
fn finish(dataset: &mut Dataset) {
    for _ in 0..10000 {
        let progress = dataset.step(64 * 1024).unwrap();
        assert!(progress.error.is_none(), "{:?}", progress.error);
        if progress.complete {
            return;
        }
    }
    panic!("Indexing did not finish");
}
fn database(cache: &Path) -> PathBuf {
    fs::read_dir(cache.join("vcf"))
        .unwrap()
        .next()
        .unwrap()
        .unwrap()
        .path()
        .join("vcf.sqlite")
}
fn gzip(bytes: &[u8]) -> Vec<u8> {
    let mut encoder = GzEncoder::new(Vec::new(), Compression::default());
    encoder.write_all(bytes).unwrap();
    encoder.finish().unwrap()
}
fn bgzf(bytes: &[u8]) -> Vec<u8> {
    let mut output = Vec::new();
    for chunk in bytes.chunks(32000) {
        let mut encoder = DeflateEncoder::new(Vec::new(), Compression::default());
        encoder.write_all(chunk).unwrap();
        let compressed = encoder.finish().unwrap();
        let size = compressed.len() + 26;
        let mut block = vec![31, 139, 8, 4, 0, 0, 0, 0, 0, 255, 6, 0, b'B', b'C', 2, 0];
        block.extend_from_slice(&((size - 1) as u16).to_le_bytes());
        block.extend_from_slice(&compressed);
        block.extend_from_slice(&crc32fast::hash(chunk).to_le_bytes());
        block.extend_from_slice(&(chunk.len() as u32).to_le_bytes());
        output.extend(block);
    }
    output
}

#[test]
fn preserves_crlf_raw_header_record_and_copy() {
    let text = format!(
        "{}{}",
        HEADER.replace('\n', "\r\n"),
        record("chr1", 2).trim_end()
    );
    let (_directory, path, cache) = fixture(text.as_bytes());
    let mut dataset = Dataset::open(&path, &cache).unwrap();
    finish(&mut dataset);
    let entry = dataset.entry(None, 0).unwrap().unwrap();
    assert_eq!(entry.record.raw, record("chr1", 2).trim_end());
    assert_eq!(dataset.header().raw, HEADER.replace('\n', "\r\n"));
    let copied = format!("{}{}\n", dataset.header().raw, entry.record.raw);
    let (_copy_directory, copy_path, copy_cache) = fixture(copied.as_bytes());
    let mut copy = Dataset::open(&copy_path, &copy_cache).unwrap();
    finish(&mut copy);
    assert_eq!(copy.total(None).unwrap(), 1);
    assert_eq!(dataset.progress().scanned_bytes, text.len() as u64);
}

#[test]
fn header_without_final_newline_keeps_physical_offset() {
    let text = HEADER.trim_end();
    let (parsed, offset, lines) = read_header(&mut Cursor::new(text)).unwrap();
    assert_eq!(offset, text.len() as u64);
    assert_eq!(lines, 4);
    assert_eq!(parsed.raw, text);
    let (_directory, path, cache) = fixture(text.as_bytes());
    let mut dataset = Dataset::open(&path, &cache).unwrap();
    finish(&mut dataset);
    assert_eq!(dataset.progress().scanned_bytes, text.len() as u64);
    assert_eq!(dataset.total(None).unwrap(), 0);
}

#[test]
fn versions_and_local_allele_metadata_are_preserved() {
    for minor in 0..=5 {
        let (depth_number, likelihood_number) = if minor == 5 { ("LR", "LG") } else { ("R", "G") };
        let raw = format!(
            "##fileformat=VCFv4.{minor}\n##source=test,source\n##reference=file:///reference.fa\n##FORMAT=<ID=LAA,Number=.,Type=Integer,Description=\"Local, alternate alleles\">\n##FORMAT=<ID=LAD,Number={depth_number},Type=Integer,Description=\"Local allelic depths\">\n##FORMAT=<ID=LPL,Number={likelihood_number},Type=Integer,Description=\"Local likelihoods\">\n#CHROM\tPOS\tID\tREF\tALT\tQUAL\tFILTER\tINFO\tFORMAT\tS1\n"
        );
        let parsed = header(&raw);
        assert_eq!(parsed.version, format!("VCFv4.{minor}"));
        assert_eq!(parsed.raw, raw);
        assert_eq!(parsed.source.as_deref(), Some("test,source"));
        assert_eq!(parsed.reference.as_deref(), Some("file:///reference.fa"));
        assert_eq!(
            parsed.definitions[0].description.as_deref(),
            Some("Local, alternate alleles")
        );
        let entry = parse_record(
            b"chr1\t3\t.\tA\tC,G\t.\t.\t.\tLAA:LAD:LPL\t1,2:7,8,9:0,1,2,3,4,5",
            &parsed,
        )
        .unwrap();
        assert_eq!(entry.record.sample_values(0, 1)[0].values[0], "1,2");
    }
    for version in ["3.3", "4.6", "5.0", "four"] {
        let raw = HEADER.replacen("4.3", version, 1);
        assert!(read_header(&mut Cursor::new(raw)).is_err());
    }
    let local = "##fileformat=VCFv4.5\n##FORMAT=<ID=LAA,Number=.,Type=Integer,Description=\"Local alternate alleles\">\n##FORMAT=<ID=LAD,Number=LR,Type=Integer,Description=\"Local allelic depths\">\n##FORMAT=<ID=LPL,Number=LG,Type=Integer,Description=\"Local likelihoods\">\n#CHROM\tPOS\tID\tREF\tALT\tQUAL\tFILTER\tINFO\tFORMAT\tS1\n";
    let parsed = header(local);
    assert_eq!(parsed.definitions[1].number.as_deref(), Some("LR"));
    assert_eq!(parsed.definitions[2].number.as_deref(), Some("LG"));
    assert_eq!(parsed.raw, local);
    for bad in [
        local.replace("Number=LR,Type=Integer", "Number=LR,Type=Float"),
        local.replace("Number=LR", "Number=R"),
        local.replace("Number=LR", "Number=LX"),
        local.replace(",Description=\"Local allelic depths\"", ""),
        local.replace(
            "#CHROM",
            "##FORMAT=<ID=LAD,Number=LR,Type=Integer,Description=\"duplicate\">\n#CHROM",
        ),
        local.replace(
            "#CHROM",
            "##FORMAT=<ID=GT,Number=2,Type=String,Description=\"invalid GT\">\n#CHROM",
        ),
    ] {
        assert!(read_header(&mut Cursor::new(bad)).is_err());
    }
}

#[test]
fn multi_alt_structural_variants_have_independent_kinds_and_no_graph_assumption() {
    let parsed = header(HEADER);
    let entry = parse_record(
        b"chr1\t0\t.;id2\tA\tT,AT,<DEL>,A]chr2:8],*\t.\tPASS\tSVTYPE=DEL;FLAG",
        &parsed,
    )
    .unwrap();
    assert_eq!(
        entry.variants.iter().map(|v| v.kind).collect::<Vec<_>>(),
        [
            VariantKind::Snv,
            VariantKind::Insertion,
            VariantKind::Symbolic,
            VariantKind::Breakend,
            VariantKind::SpanningDeletion
        ]
    );
    assert_eq!(entry.variants[1].length_change, Some(1));
    assert_eq!(entry.variants[2].length_change, None);
    assert!(entry.graph.is_none());
    assert_eq!(
        entry.record.info_fields(),
        [("SVTYPE".into(), "DEL".into()), ("FLAG".into(), "".into())]
    );
    assert_eq!(
        super::reader::classify_variant("AC", "AT"),
        VariantKind::Mnv
    );
    assert_eq!(
        super::reader::classify_variant("AC", "A"),
        VariantKind::Deletion
    );
}

#[test]
fn sample_pages_preserve_polyploid_phased_missing_and_omitted_values() {
    let raw = HEADER.trim_end().to_owned() + "\tFORMAT\tS1\tS2\tS3\tS4\n";
    let parsed = header(&raw);
    let record = parse_record(
        b"chr1\t5\t.\tA\tC,G\t.\t.\tDP=2\tGT:DP:AD\t0|1|2:7:2,3,2\t./.\t.:.\t0/0:9",
        &parsed,
    )
    .unwrap()
    .record;
    assert_eq!(
        record.sample_values(0, 1)[0].values,
        ["0|1|2", "7", "2,3,2"]
    );
    let page = record.sample_values(1, 2);
    assert_eq!(page.len(), 2);
    assert_eq!(page[0].index, 1);
    assert_eq!(page[0].values, ["./.", ".", "."]);
    assert_eq!(page[1].values, [".", ".", "."]);
    assert_eq!(record.sample_values(3, 1)[0].values, ["0/0", "9", "."]);
    assert!(record.sample_values(8, 2).is_empty());
    assert!(record.raw.ends_with("0/0:9"));
}

#[test]
fn rejects_column_mismatches_and_extra_format_values() {
    let parsed = header(&(HEADER.trim_end().to_owned() + "\tFORMAT\tS1\n"));
    for text in [
        "chr1\t1\t.\tA\tT\t.\t.\t.\tGT",
        "chr1\t1\t.\tA\tT\t.\t.\t.\tGT\t0/1\textra",
        "chr1\t1\t.\tA\tT\t.\t.\t.\tGT\t0/1:42",
        "chr1\t1\t.\tA\tT\t.\t.\t.\tGT\t",
    ] {
        assert!(parse_record(text.as_bytes(), &parsed).is_err(), "{text}");
    }
    let mut manual = parse_record(b"chr1\t1\t.\tA\tT\t.\t.\t.\tGT\t0/1", &parsed)
        .unwrap()
        .record;
    manual.raw.push_str(":42");
    assert_eq!(manual.sample_values(0, 1)[0].values, ["0/1", "42"]);
}

#[test]
fn partial_large_line_budget_reopens_from_complete_record_boundary() {
    let first = record("chr1", 1);
    let long = format!("chr2\t2\t.\tA\tT\t.\tPASS\tLONG={}\n", "x".repeat(300_000));
    let text = format!("{HEADER}{first}{long}{}", record("chr1", 3));
    let (_directory, path, cache) = fixture(text.as_bytes());
    let mut dataset = Dataset::open(&path, &cache).unwrap();
    assert_eq!(dataset.progress().records, 0);
    assert!(!dataset.progress().complete);
    let progress = dataset.step(65536).unwrap();
    assert_eq!(progress.records, 1);
    assert_eq!(progress.scanned_bytes, HEADER.len() as u64 + 65536);
    assert!(!progress.complete);
    assert_eq!(dataset.entry(None, 0).unwrap().unwrap().record.pos, 1);
    drop(dataset);
    let mut resumed = Dataset::open(&path, &cache).unwrap();
    assert_eq!(resumed.progress().records, 1);
    assert_eq!(
        resumed.progress().scanned_bytes,
        (HEADER.len() + first.len()) as u64
    );
    finish(&mut resumed);
    assert_eq!(resumed.total(None).unwrap(), 3);
    assert_eq!(
        resumed.entry(None, 1).unwrap().unwrap().record.raw,
        long.trim_end()
    );
    assert_eq!(resumed.entry(None, 2).unwrap().unwrap().record.pos, 3);
}

#[test]
fn selected_record_reads_do_not_disturb_pending_scan_bytes() {
    let text = format!(
        "{HEADER}{}chr2\t2\t.\tA\tT\t.\t.\tLONG={}\n{}",
        record("chr1", 1),
        "a".repeat(20000),
        record("chr1", 3)
    );
    let (_directory, path, cache) = fixture(text.as_bytes());
    let mut dataset = Dataset::open(&path, &cache).unwrap();
    dataset.step(200).unwrap();
    for _ in 0..3 {
        assert_eq!(dataset.entry(None, 0).unwrap().unwrap().record.pos, 1);
    }
    finish(&mut dataset);
    assert_eq!(dataset.total(None).unwrap(), 3);
    assert_eq!(
        dataset
            .entry(None, 1)
            .unwrap()
            .unwrap()
            .record
            .info_fields()[0]
            .1
            .len(),
        20000
    );
}

#[test]
fn ordinary_gzip_and_bgzf_resume_and_reuse_complete_cache() {
    let mut text = HEADER.to_owned();
    for pos in 1..=3000 {
        text.push_str(&record(if pos % 2 == 0 { "chr1" } else { "chr2" }, pos));
    }
    for (bytes, compression) in [
        (gzip(text.as_bytes()), "gzip"),
        (bgzf(text.as_bytes()), "BGZF"),
    ] {
        let (_directory, path, cache) = fixture(&bytes);
        let mut dataset = Dataset::open(&path, &cache).unwrap();
        assert_eq!(dataset.compression(), compression);
        let partial = dataset.step(16000).unwrap();
        assert!(partial.records > 0 && !partial.complete);
        drop(dataset);
        let mut resumed = Dataset::open(&path, &cache).unwrap();
        assert_eq!(resumed.progress().records, partial.records);
        finish(&mut resumed);
        let final_record = resumed
            .entry(Some("chr1"), 1499)
            .unwrap()
            .unwrap()
            .record
            .raw
            .clone();
        assert_eq!(resumed.total(Some("chr2")).unwrap(), 1500);
        drop(resumed);
        let mut reopened = Dataset::open(&path, &cache).unwrap();
        assert!(reopened.progress().complete);
        assert_eq!(reopened.progress().records, 3000);
        assert_eq!(
            reopened
                .entry(Some("chr1"), 1499)
                .unwrap()
                .unwrap()
                .record
                .raw,
            final_record
        );
        let db = rusqlite::Connection::open(database(&cache)).unwrap();
        assert_eq!(
            db.query_row("SELECT COUNT(*) FROM records", [], |row| row
                .get::<_, u64>(0))
                .unwrap(),
            3000
        );
    }
}

#[test]
fn mixed_alt_types_remain_mixed_in_persisted_table_summaries() {
    let text =
        format!("{HEADER}chr1\t10\t.\tA\tT,AT\t.\tPASS\t.\nchr1\t20\t.\tA\tT,C\t.\tPASS\t.\n");
    let (_directory, path, cache) = fixture(text.as_bytes());
    let mut dataset = Dataset::open(&path, &cache).unwrap();
    finish(&mut dataset);
    let rows = dataset.rows(None, 0, 10).unwrap();
    assert_eq!(rows[0].kind, VariantKind::Mixed);
    assert_eq!(rows[1].kind, VariantKind::Snv);
    let entry = dataset.entry(None, 0).unwrap().unwrap();
    assert_eq!(
        entry.variants.iter().map(|v| v.kind).collect::<Vec<_>>(),
        [VariantKind::Snv, VariantKind::Insertion]
    );
    drop(dataset);
    let reopened = Dataset::open(&path, &cache).unwrap();
    assert_eq!(
        reopened.rows(None, 0, 1).unwrap()[0].kind,
        VariantKind::Mixed
    );
}

#[test]
fn single_breakends_remain_breakends_in_records_and_cached_summaries() {
    let mut text = HEADER.to_owned();
    let alleles = ["A.", ".A", "TCC.", ".TGCA", "AT"];
    for (i, alternate) in alleles.iter().enumerate() {
        text.push_str(&format!("chr1\t{}\t.\tA\t{alternate}\t.\tPASS\t.\n", i + 1));
    }
    let (_directory, path, cache) = fixture(text.as_bytes());
    let mut dataset = Dataset::open(&path, &cache).unwrap();
    finish(&mut dataset);
    let expected = [
        VariantKind::Breakend,
        VariantKind::Breakend,
        VariantKind::Breakend,
        VariantKind::Breakend,
        VariantKind::Insertion,
    ];
    assert_eq!(
        dataset
            .rows(None, 0, 10)
            .unwrap()
            .iter()
            .map(|row| row.kind)
            .collect::<Vec<_>>(),
        expected
    );
    for (i, alternate) in alleles.iter().enumerate() {
        let entry = dataset.entry(None, i as u64).unwrap().unwrap();
        assert_eq!(entry.variants[0].alternate, *alternate);
        assert_eq!(entry.variants[0].kind, expected[i]);
        assert_eq!(entry.variants[0].length_change, (i == 4).then_some(1));
    }
    drop(dataset);
    let reopened = Dataset::open(&path, &cache).unwrap();
    assert_eq!(
        reopened
            .rows(None, 0, 10)
            .unwrap()
            .iter()
            .map(|row| row.kind)
            .collect::<Vec<_>>(),
        expected
    );
}

#[test]
fn index_stores_short_fixed_fields_and_contig_ordinals() {
    let text = format!(
        "{HEADER}chr2\t20\t{}\tA\tT\t.\tPASS\tSECRET_INFO={}\n{}{}",
        "id".repeat(100),
        "z".repeat(1000),
        record("chr1", 10),
        record("chr2", 5)
    );
    let (_directory, path, cache) = fixture(text.as_bytes());
    let mut dataset = Dataset::open(&path, &cache).unwrap();
    finish(&mut dataset);
    let all = dataset.rows(None, 0, 10).unwrap();
    assert_eq!(all.iter().map(|r| r.ordinal).collect::<Vec<_>>(), [0, 1, 2]);
    assert!(all[0].ids.chars().count() <= 64);
    let filtered = dataset.rows(Some("chr2"), 0, 10).unwrap();
    assert_eq!(
        filtered
            .iter()
            .map(|r| (r.id, r.ordinal))
            .collect::<Vec<_>>(),
        [(0, 0), (2, 1)]
    );
    assert_eq!(dataset.locate("chr2", 5).unwrap(), Some(1));
    assert_eq!(dataset.locate("chr2", 6).unwrap(), Some(0));
    assert_eq!(dataset.locate("chr2", 999).unwrap(), Some(0));
    assert_eq!(dataset.contigs().unwrap()[0].name, "chr1");
    let db = rusqlite::Connection::open(database(&cache)).unwrap();
    let mut statement = db.prepare("PRAGMA table_info(records)").unwrap();
    let columns = statement
        .query_map([], |row| row.get::<_, String>(1))
        .unwrap()
        .collect::<Result<Vec<_>, _>>()
        .unwrap();
    assert!(
        !columns
            .iter()
            .any(|name| name == "raw" || name == "samples" || name == "info")
    );
}

#[test]
fn errors_keep_valid_prefix_and_report_actual_line_and_offset() {
    let first = record("chr1", 1);
    let text = format!(
        "{HEADER}{first}chr1\tbad\t.\tA\tT\t.\t.\t.\n{}",
        record("chr1", 3)
    );
    let (_directory, path, cache) = fixture(text.as_bytes());
    let mut dataset = Dataset::open(&path, &cache).unwrap();
    let progress = dataset.step(65536).unwrap();
    assert_eq!(progress.records, 1);
    assert_eq!(progress.errors, 1);
    let message = progress.error.unwrap();
    assert!(message.contains("line 6"), "{message}");
    assert!(
        message.contains(&format!("decoded byte {}", HEADER.len() + first.len())),
        "{message}"
    );
    assert_eq!(dataset.entry(None, 0).unwrap().unwrap().record.pos, 1);
    drop(dataset);
    let reopened = Dataset::open(&path, &cache).unwrap();
    assert_eq!(reopened.progress().records, 1);
    assert!(reopened.progress().error.is_some());
}

#[test]
fn source_changes_are_rejected_before_using_stale_index() {
    let text = format!("{HEADER}{}", record("chr1", 1));
    let (_directory, path, cache) = fixture(text.as_bytes());
    let mut dataset = Dataset::open(&path, &cache).unwrap();
    finish(&mut dataset);
    fs::write(&path, format!("{text}{}", record("chr1", 2))).unwrap();
    assert!(
        dataset
            .rows(None, 0, 1)
            .unwrap_err()
            .to_string()
            .contains("Source changed")
    );
    assert!(
        dataset
            .entry(None, 0)
            .unwrap_err()
            .to_string()
            .contains("Source changed")
    );
}

fn view(file: usize, token: u64, selected: u64) -> Request {
    Request::View(ViewRequest {
        file,
        token,
        contig: None,
        selected,
        start: selected,
        count: 2,
    })
}

#[test]
fn backend_coalesces_views_and_preserves_pause_operation_barrier() {
    let mut text = HEADER.to_owned();
    for pos in 0..10000 {
        text.push_str(&record("chr1", pos));
    }
    let (directory, path, cache) = fixture(text.as_bytes());
    let (send, receive) = mpsc::channel();
    let mut backend = Backend::new(
        SessionFiles::new(vec![directory.path().join("missing.vcf"), path]),
        cache,
        send,
    );
    assert!(
        backend.handle_requests(
            vec![
                view(0, 1, 0),
                view(1, 2, 0),
                Request::Cancel(1),
                view(1, 3, 0)
            ]
            .into_iter()
        )
    );
    backend.tick();
    let updates = receive.try_iter().collect::<Vec<_>>();
    assert!(
        !updates
            .iter()
            .any(|update| matches!(update, Update::Message { file: 0, .. }))
    );
    let snapshot = updates
        .into_iter()
        .find_map(|update| {
            if let Update::View(s) = update {
                Some(s)
            } else {
                None
            }
        })
        .unwrap();
    assert!(snapshot.paused);
    assert_eq!(snapshot.progress.records, 0);
    assert_eq!(snapshot.request.token, 3);
    assert_eq!(backend.timeout(), Duration::from_secs(60));
    backend.handle_requests(std::iter::once(view(1, 4, 1)));
    backend.tick();
    let snapshot = receive
        .try_iter()
        .find_map(|update| {
            if let Update::View(s) = update {
                Some(s)
            } else {
                None
            }
        })
        .unwrap();
    assert!(snapshot.paused && snapshot.progress.records > 0 && !snapshot.progress.complete);
}

#[test]
fn backend_locates_contig_ordinal_with_request_identity() {
    let text = format!(
        "{HEADER}{}{}{}",
        record("chr1", 100),
        record("chr2", 40),
        record("chr1", 20)
    );
    let (_directory, path, cache) = fixture(text.as_bytes());
    let (send, receive) = mpsc::channel();
    let mut backend = Backend::new(SessionFiles::new(vec![path]), cache, send);
    backend.handle_requests(
        vec![
            view(0, 1, 0),
            Request::Locate {
                file: 0,
                token: 2,
                contig: "chr1".into(),
                position: 20,
            },
        ]
        .into_iter(),
    );
    backend.tick();
    assert!(receive.try_iter().any(|update| matches!(
        update,
        Update::Located {
            file: 0,
            token: 2,
            selected: 1
        }
    )));
    backend.handle_requests(std::iter::once(Request::Locate {
        file: 0,
        token: 3,
        contig: "chr1".into(),
        position: 21,
    }));
    backend.tick();
    assert!(receive.try_iter().any(|update| matches!(
        update,
        Update::Located {
            file: 0,
            token: 3,
            selected: 0
        }
    )));
    assert!(!backend.handle_requests(std::iter::once(Request::Quit)));
}

#[test]
fn backend_locate_preserves_scan_and_cache_errors() {
    for cache_failure in [false, true] {
        let suffix = if cache_failure {
            ""
        } else {
            "chr1\tbad\t.\tA\tT\t.\tPASS\t.\n"
        };
        let text = format!("{HEADER}{}{suffix}", record("chr1", 10));
        let (_directory, path, cache) = fixture(text.as_bytes());
        let (send, receive) = mpsc::channel();
        let mut backend = Backend::new(SessionFiles::new(vec![path]), cache.clone(), send);
        backend.handle_requests(std::iter::once(view(0, 1, 0)));
        if cache_failure {
            let db = rusqlite::Connection::open(database(&cache)).unwrap();
            db.execute_batch(
                "CREATE TRIGGER fail_insert BEFORE INSERT ON records
                BEGIN
                    SELECT RAISE(ABORT, 'cache write failed');
                END;",
            )
            .unwrap();
        }
        backend.handle_requests(std::iter::once(Request::Locate {
            file: 0,
            token: 2,
            contig: "chr1".into(),
            position: 11,
        }));
        backend.tick();
        let failure = receive
            .try_iter()
            .find_map(|update| match update {
                Update::LocateFailed {
                    file: 0,
                    token: 2,
                    text,
                } => Some(text),
                _ => None,
            })
            .unwrap();
        assert!(
            failure.contains(if cache_failure {
                "cache write failed"
            } else {
                "Invalid POS"
            }),
            "{failure}"
        );
        assert!(!failure.contains("No indexed variants"));
    }
}

#[test]
fn backend_locate_failures_keep_file_and_request_token() {
    let (_directory, path, cache) = fixture(HEADER.as_bytes());
    let (send, receive) = mpsc::channel();
    let mut backend = Backend::new(SessionFiles::new(vec![path]), cache, send);
    backend.handle_requests(
        vec![
            view(0, 1, 0),
            Request::Locate {
                file: 0,
                token: 7,
                contig: "missing".into(),
                position: 20,
            },
        ]
        .into_iter(),
    );
    backend.tick();
    assert!(receive.try_iter().any(|update| matches!(update, Update::LocateFailed {file:0, token:7, text} if text.contains("missing"))));
    backend.handle_requests(std::iter::once(Request::Locate {
        file: 0,
        token: 8,
        contig: "chr1".into(),
        position: u64::MAX,
    }));
    backend.tick();
    assert!(receive.try_iter().any(|update| matches!(update, Update::LocateFailed {file:0, token:8, text} if text.contains("Position exceeds"))));
}
