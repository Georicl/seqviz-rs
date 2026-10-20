use super::*;

fn fixture(content: &[u8], extension: &str) -> (tempfile::TempDir, std::path::PathBuf) {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join(format!("input.{extension}"));
    fs::write(&path, content).unwrap();
    (dir, path)
}

fn finish(data: &mut Dataset, budget: usize) {
    for _ in 0..100_000 {
        let progress = data.step(budget).unwrap();
        assert!(progress.error.is_none(), "{:?}", progress.error);
        if progress.complete {
            return;
        }
    }
    panic!("scan did not finish");
}

#[test]
fn long_fasta_is_browsable_before_record_completion_and_spans_are_compact() {
    let mut input = b">one full header\n".to_vec();
    let sequence = b"ACGTN".repeat(50_000);
    for line in sequence.chunks(70) {
        input.extend_from_slice(line);
        input.push(b'\n');
    }
    input.extend_from_slice(b">two\nGGCC");
    let (dir, path) = fixture(&input, "fa");
    let mut data = Dataset::open(&path, &dir.path().join("cache")).unwrap();
    let progress = data.step(1024).unwrap();
    assert_eq!(progress.records, 1);
    assert!(!progress.complete);
    assert!(!data.record(0).unwrap().unwrap().complete);
    assert_eq!(
        data.read_range(0, 80, 100).unwrap().sequence,
        sequence[80..180]
    );
    finish(&mut data, CHUNK);
    assert_eq!(
        data.read_range(0, 190_000, 10_000).unwrap().sequence,
        sequence[190_000..200_000]
    );
    assert_eq!(data.read_range(1, 0, 100).unwrap().sequence, b"GGCC");
    let spans: u64 = data
        .index
        .db
        .query_row("SELECT COUNT(*) FROM spans", [], |r| r.get(0))
        .unwrap();
    assert!(spans < 20, "{spans} spans for {} source bytes", input.len());
    let stats = data.stats().unwrap();
    assert_eq!(stats.records, 2);
    assert_eq!(stats.n50, Some(250_000));
    assert_eq!(stats.min_length, Some(4));
    assert!((stats.gc.unwrap() - (100_004.0 / 250_004.0 * 100.0)).abs() < 0.000001);
}

#[test]
fn fastq_accepts_wrapped_sequence_quality_and_marker_quality_characters() {
    let (dir, path) = fixture(
        b"@r1 full header\r\nACG\r\nTN\r\n+r1 full header\r\n@+I\r\n?J\r\n@r2\nA\n+\n!",
        "fq",
    );
    let mut data = Dataset::open(&path, &dir.path().join("cache")).unwrap();
    finish(&mut data, 1);
    let record = data.record(0).unwrap().unwrap();
    assert_eq!(record.header, "r1 full header");
    assert_eq!(record.plus, "+r1 full header");
    assert_eq!(record.length, 5);
    assert_eq!(record.quality_length, 5);
    assert_eq!(record.gc, Some(40.0));
    assert_eq!(record.quality.unwrap().q30, 80.0);
    let range = data.read_range(0, 1, 3).unwrap();
    assert_eq!(range.sequence, b"CGT");
    assert_eq!(range.quality, b"+I?");
    assert_eq!(
        data.copy_record(0, &AtomicBool::new(false)).unwrap(),
        "@r1 full header\nACGTN\n+r1 full header\n@+I?J\n"
    );
}

#[test]
fn fasta_copy_wraps_at_sixty_and_protein_gc_is_not_applicable() {
    let mut input = b">protein actual header\n".to_vec();
    input.extend_from_slice(&b"ACGT".repeat(31));
    let (dir, path) = fixture(&input, "faa");
    let mut data = Dataset::open(&path, &dir.path().join("cache")).unwrap();
    finish(&mut data, 31);
    let record = data.record(0).unwrap().unwrap();
    assert_eq!(record.seq_type, SeqType::Protein);
    assert_eq!(record.gc, None);
    let copied = data.copy_record(0, &AtomicBool::new(false)).unwrap();
    assert_eq!(
        copied.lines().skip(1).map(str::len).collect::<Vec<_>>(),
        vec![60, 60, 4]
    );
    let stats = data.stats().unwrap();
    assert!(stats.protein);
    assert_eq!(stats.gc, None);
    assert_eq!(stats.n50, Some(124));
}

#[test]
fn persistent_complete_and_partial_indexes_are_reused_and_changed_source_invalidates() {
    let (dir, path) = fixture(b">one\nACGT\n>two\nGGGG\n>three\nNNNN\n", "fa");
    let cache = dir.path().join("cache");
    let mut data = Dataset::open(&path, &cache).unwrap();
    data.step(18).unwrap();
    assert_eq!(data.records(0, 10).unwrap().len(), 2);
    drop(data);
    let mut data = Dataset::open(&path, &cache).unwrap();
    assert_eq!(data.progress().records, 1);
    finish(&mut data, 5);
    assert_eq!(data.progress().records, 3);
    drop(data);
    let mut data = Dataset::open(&path, &cache).unwrap();
    assert!(data.progress().complete);
    assert_eq!(data.read_range(2, 0, 4).unwrap().sequence, b"NNNN");
    drop(data);
    fs::write(&path, b">new\nA\n").unwrap();
    let mut data = Dataset::open(&path, &cache).unwrap();
    assert!(!data.progress().complete);
    finish(&mut data, CHUNK);
    assert_eq!(data.record(0).unwrap().unwrap().name, "new");
}

#[test]
fn malformed_fastq_stops_without_guessing_at_next_at_sign() {
    let (dir, path) = fixture(
        b"@good\nAC\n+\nII\n@broken\nACGT\n+\nII\n@next\nAC\n+\nII\n",
        "fq",
    );
    let mut data = Dataset::open(&path, &dir.path().join("cache")).unwrap();
    for _ in 0..100 {
        let progress = data.step(5).unwrap();
        if progress.error.is_some() {
            break;
        }
    }
    assert!(
        data.index
            .progress
            .error
            .as_deref()
            .unwrap()
            .contains("quality")
    );
    assert!(!data.progress().complete);
    assert_eq!(data.progress().errors, 1);
    assert_eq!(data.read_range(0, 0, 100).unwrap().sequence, b"AC");
    assert!(!data.stats().unwrap().complete);
    assert_eq!(data.step(1024).unwrap().errors, 1);
}

#[test]
fn literal_name_search_starts_inclusively_without_implicit_wrap() {
    let (dir, path) = fixture(
        b">Needle_0% description\nA\n>needle_1%\nC\n>other\nG\n",
        "fa",
    );
    let mut data = Dataset::open(&path, &dir.path().join("cache")).unwrap();
    finish(&mut data, CHUNK);
    assert_eq!(data.find_name("NEEDLE", 0).unwrap(), Some(0));
    assert_eq!(data.find_name("needle", 1).unwrap(), Some(1));
    assert_eq!(data.find_name("needle", 2).unwrap(), None);
    assert_eq!(data.find_name("_0%", 0).unwrap(), Some(0));
    assert_eq!(data.find_name("description", 0).unwrap(), None);
}

#[test]
fn cancellation_and_range_reads_leave_scan_position_intact() {
    let (dir, path) = fixture(b">one\nACGT\n>two\nCCCC\n", "fa");
    let mut data = Dataset::open(&path, &dir.path().join("cache")).unwrap();
    data.step(16).unwrap();
    assert_eq!(data.read_range(0, 0, 4).unwrap().sequence, b"ACGT");
    assert_eq!(
        data.copy_record(0, &AtomicBool::new(true))
            .unwrap_err()
            .kind(),
        io::ErrorKind::Interrupted
    );
    finish(&mut data, 2);
    assert_eq!(data.record(1).unwrap().unwrap().name, "two");
    assert_eq!(data.read_range(1, 0, 4).unwrap().sequence, b"CCCC");
}

#[test]
fn content_detection_does_not_depend_on_extension() {
    let (dir, path) = fixture(b">name\nACG\n", "data");
    assert_eq!(
        Dataset::open(&path, &dir.path().join("cache"))
            .unwrap()
            .format(),
        Format::Fasta
    );
    assert_eq!(detect_type(b"acgtnx"), SeqType::Unknown);
    assert_eq!(detect_type(b"MPEPTIDE*"), SeqType::Protein);
}

#[test]
fn fasta_comments_across_chunks_are_not_in_sequence_spans() {
    let (dir, path) = fixture(
        b"# intro\n\n>first\nAC\n# comment containing GGG>ACG\nGT\n>last without newline",
        "fa",
    );
    let mut data = Dataset::open(&path, &dir.path().join("cache")).unwrap();
    finish(&mut data, 3);
    assert_eq!(data.read_range(0, 0, 100).unwrap().sequence, b"ACGT");
    let empty = data.record(1).unwrap().unwrap();
    assert_eq!(empty.header, "last without newline");
    assert_eq!(empty.length, 0);
    assert!(empty.complete);
    assert_eq!(
        data.copy_record(0, &AtomicBool::new(false)).unwrap(),
        ">first\nACGT\n"
    );
}

#[test]
fn empty_and_comment_only_fasta_have_zero_records() {
    for content in [
        b"".as_slice(),
        b"# comment without newline",
        b"\n# comment\n\n",
    ] {
        let (dir, path) = fixture(content, "fa");
        let mut data = Dataset::open(&path, &dir.path().join("cache")).unwrap();
        finish(&mut data, 1);
        assert_eq!(data.progress().records, 0);
        assert_eq!(data.stats().unwrap().total_length, 0);
    }
}

#[test]
fn cancelled_sql_operations_report_interrupted_and_remove_hooks() {
    let (dir, path) = fixture(b">one\nA\n", "fa");
    let mut data = Dataset::open(&path, &dir.path().join("cache")).unwrap();
    finish(&mut data, 10);
    let cancel = Arc::new(AtomicBool::new(true));
    assert_eq!(
        data.stats_cancellable(cancel.clone()).unwrap_err().kind(),
        io::ErrorKind::Interrupted
    );
    assert_eq!(
        data.find_name_cancellable("one", 0, cancel)
            .unwrap_err()
            .kind(),
        io::ErrorKind::Interrupted
    );
    assert_eq!(data.find_name("one", 0).unwrap(), Some(0));
}

#[test]
fn concurrent_datasets_use_isolated_writer_caches() {
    use std::io::Write;
    let (dir, path) = fixture(b"", "fa.gz");
    let file = File::create(&path).unwrap();
    let mut gzip = flate2::write::GzEncoder::new(file, flate2::Compression::default());
    gzip.write_all(b">one\nACGT\n>two\nGGCC\n").unwrap();
    gzip.finish().unwrap();
    let cache = dir.path().join("cache");
    let mut first = Dataset::open(&path, &cache).unwrap();
    first.step(9).unwrap();
    let mut second = Dataset::open(&path, &cache).unwrap();
    assert!(first.private_cache.is_none());
    let private = second.private_cache.clone().unwrap();
    finish(&mut second, 2);
    finish(&mut first, 3);
    assert_eq!(first.read_range(1, 0, 4).unwrap().sequence, b"GGCC");
    assert_eq!(second.read_range(0, 0, 4).unwrap().sequence, b"ACGT");
    drop(second);
    assert!(!private.exists());
    drop(first);
    let reopened = Dataset::open(&path, &cache).unwrap();
    assert!(reopened.progress().complete);
    assert_eq!(reopened.progress().records, 2);
}

#[test]
fn source_rewrite_invalidates_live_partial_complete_and_reopened_indexes() {
    for mode in 0..3 {
        let (dir, path) = fixture(b">one\nACGT\n>two\nTTTT\n", "fa");
        let cache = dir.path().join("cache");
        let mut data = Dataset::open(&path, &cache).unwrap();
        if mode == 0 {
            data.step(9).unwrap();
        } else {
            finish(&mut data, CHUNK);
            if mode == 2 {
                drop(data);
                data = Dataset::open(&path, &cache).unwrap();
                assert!(data.progress().complete);
            }
        }
        let modified = fs::metadata(&path).unwrap().modified().unwrap();
        fs::write(&path, b">one\nGGGG\n>two\nCCCC\n").unwrap();
        // A same-size rewrite with restored mtime must also invalidate ctime.
        File::options()
            .write(true)
            .open(&path)
            .unwrap()
            .set_times(fs::FileTimes::new().set_modified(modified))
            .unwrap();
        let range_error = data.read_range(0, 0, 4).unwrap_err();
        assert_eq!(range_error.kind(), io::ErrorKind::InvalidData);
        assert!(range_error.to_string().contains("Source changed"));
        assert_eq!(data.step(4).unwrap_err().kind(), io::ErrorKind::InvalidData);
        assert_eq!(
            data.copy_record(0, &AtomicBool::new(false))
                .unwrap_err()
                .kind(),
            io::ErrorKind::InvalidData
        );
        drop(data);
        let mut reopened = Dataset::open(&path, &cache).unwrap();
        assert!(!reopened.progress().complete);
        finish(&mut reopened, CHUNK);
        assert_eq!(reopened.read_range(0, 0, 4).unwrap().sequence, b"GGGG");
    }
}
