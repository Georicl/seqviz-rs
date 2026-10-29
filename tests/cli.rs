//! Command-line help, exit status and redirected-output integration tests.

use std::path::Path;
use std::process::Command;

/// Run the compiled command with captured output and an isolated cache directory.
fn run(paths: &[&Path]) -> std::process::Output {
    let cache = tempfile::tempdir().unwrap();
    Command::new(env!("CARGO_BIN_EXE_sq"))
        .args(paths)
        .env("XDG_CACHE_HOME", cache.path())
        .output()
        .unwrap()
}

/// Check command help, positional-file usage and the package version printed to stdout.
#[test]
fn help_and_version_describe_the_current_cli_contract() {
    let help = Command::new(env!("CARGO_BIN_EXE_sq"))
        .arg("--help")
        .output()
        .unwrap();
    assert!(help.status.success());
    assert!(help.stderr.is_empty());
    let text = String::from_utf8(help.stdout).unwrap();
    assert!(text.contains("A terminal browser for bioformatics file"));
    assert!(text.contains("Usage: sq <FILE>..."));
    assert!(text.contains("-h, --help"));
    assert!(text.contains("-V, --version"));

    let version = Command::new(env!("CARGO_BIN_EXE_sq"))
        .arg("--version")
        .output()
        .unwrap();
    assert!(version.status.success());
    assert!(version.stderr.is_empty());
    assert_eq!(
        version.stdout,
        format!("seqviz {}\n", env!("CARGO_PKG_VERSION")).as_bytes()
    );
}

/// Preserve input order and full FASTA records when multiple paths are redirected.
#[test]
fn prints_all_records_for_each_input_file() {
    let fixture = Path::new(env!("CARGO_MANIFEST_DIR")).join("test/test.fa");
    let output = run(&[&fixture, &fixture]);

    assert!(output.status.success());
    assert!(output.stderr.is_empty());
    let stdout = String::from_utf8(output.stdout).unwrap();
    let expected = ">seq1 example description\nACGTTGCA\n>seq2\nAAA\n";
    assert_eq!(stdout, expected.repeat(2));
}

/// Return a failing exit status and stderr for a missing input without emitting records.
#[test]
fn missing_file_returns_failure() {
    let missing = Path::new(env!("CARGO_MANIFEST_DIR")).join("test/missing-header-test.fa");
    assert!(!missing.exists());
    let output = run(&[&missing]);

    assert!(!output.status.success());
    assert!(!output.stderr.is_empty());
    assert!(output.stdout.is_empty());
}

/// Reject unrelated text with an invalid-data error and empty redirected output.
#[test]
fn invalid_fasta_returns_failure() {
    let fixture = Path::new(env!("CARGO_MANIFEST_DIR")).join("Cargo.toml");
    let output = run(&[&fixture]);

    assert!(!output.status.success());
    assert!(
        String::from_utf8(output.stderr)
            .unwrap()
            .contains("InvalidData")
    );
    assert!(output.stdout.is_empty());
}

/// Keep redirected FASTA headers, whitespace handling and empty records compression-independent.
#[test]
fn gzip_and_plain_fasta_use_identical_parsing_rules() {
    use flate2::{Compression, write::GzEncoder};
    use std::{fs, io::Write};

    let tmp = tempfile::tempdir().unwrap();
    let plain = tmp.path().join("sequence.fa");
    let gzip = tmp.path().join("sequence.fa.gz");
    let input = b"# comment\n>seq full header\nAC GT\tN\nac\n>empty\n";
    fs::write(&plain, input).unwrap();
    let mut encoder = GzEncoder::new(fs::File::create(&gzip).unwrap(), Compression::default());
    encoder.write_all(input).unwrap();
    encoder.finish().unwrap();

    let plain_output = run(&[&plain]);
    let gzip_output = run(&[&gzip]);
    assert!(plain_output.status.success(), "{:?}", plain_output);
    assert!(gzip_output.status.success(), "{:?}", gzip_output);
    assert_eq!(plain_output.stdout, b">seq full header\nACGTNac\n>empty\n");
    assert_eq!(plain_output.stdout, gzip_output.stdout);
}

/// Require an interactive terminal for FASTQ, SAM, BAM and VCF inputs.
#[test]
fn non_fasta_redirects_are_rejected_without_copying_records() {
    let tmp = tempfile::tempdir().unwrap();
    let fastq = tmp.path().join("reads.fastq");
    std::fs::write(&fastq, b"@read\nACGT\n+\nIIII\n").unwrap();
    let vcf = tmp.path().join("variants.txt");
    std::fs::write(&vcf, b"##fileformat=VCFv4.3\n#CHROM\tPOS\tID\tREF\tALT\tQUAL\tFILTER\tINFO\nchr1\t10\t.\tA\tC\t.\tPASS\t.\n").unwrap();
    let root = Path::new(env!("CARGO_MANIFEST_DIR"));
    for path in [
        fastq,
        vcf,
        root.join("test/alignments.sam"),
        root.join("test/alignments.bam"),
    ] {
        let output = run(&[&path]);
        assert!(!output.status.success(), "{}", path.display());
        assert!(output.stdout.is_empty());
        assert!(
            String::from_utf8(output.stderr)
                .unwrap()
                .contains("interactive terminal")
        );
    }
}
