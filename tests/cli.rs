use std::path::Path;
use std::process::Command;

#[test]
fn prints_first_header_for_each_input_file() {
    let fixture = Path::new(env!("CARGO_MANIFEST_DIR")).join("test/test.fa");
    let output = Command::new(env!("CARGO_BIN_EXE_sq"))
        .arg(&fixture)
        .arg(&fixture)
        .output()
        .unwrap();

    assert!(output.status.success());
    assert!(output.stderr.is_empty());
    let stdout = String::from_utf8(output.stdout).unwrap();
    assert_eq!(stdout.matches("FastaReader {").count(), 2);
    assert!(stdout.contains("name: \"seq1\""));
    assert!(stdout.contains("description: \"example description\""));
    assert!(stdout.contains("}\nFastaReader {"));
    assert!(!stdout.contains("seq2"));
}

#[test]
fn missing_file_returns_failure() {
    let missing = Path::new(env!("CARGO_MANIFEST_DIR")).join("test/missing-header-test.fa");
    assert!(!missing.exists());
    let output = Command::new(env!("CARGO_BIN_EXE_sq"))
        .arg(missing)
        .output()
        .unwrap();

    assert!(!output.status.success());
    assert!(!output.stderr.is_empty());
    assert!(output.stdout.is_empty());
}

#[test]
fn invalid_fasta_returns_failure() {
    let fixture = Path::new(env!("CARGO_MANIFEST_DIR")).join("Cargo.toml");
    let output = Command::new(env!("CARGO_BIN_EXE_sq"))
        .arg(fixture)
        .output()
        .unwrap();

    assert!(!output.status.success());
    assert!(
        String::from_utf8(output.stderr)
            .unwrap()
            .contains("InvalidData")
    );
    assert!(output.stdout.is_empty());
}
