//! Infer file formats and gzip suffix flags from input paths.
use std::path::{Path, PathBuf};

/// File format inferred from the filename suffix.
#[derive(Debug)]
enum FileFormat {
    Fasta,
    Fastq,
    Sam,
    Bam,
    Unknown,
}

/// Input path and format information inferred from its filename.
pub struct FileInfo {
    file_type: FileFormat,

    /// Original input path, including any `.gz` suffix.
    path: PathBuf,
    /// Whether the final suffix is `.gz`, ignoring ASCII case.
    is_gzip: bool,
}

/// Infer the format and gzip suffix flag for each input path.
///
/// For `.gz` paths, classify the extension immediately before `.gz`.
/// Preserve input order and original paths without opening the files.
pub fn receive_files(paths: Vec<PathBuf>) -> Vec<FileInfo> {
    let mut files: Vec<FileInfo> = Vec::new();

    for path in paths {
        let outer_ext = extension_lowercase(&path);
        let is_gzip = outer_ext == "gz";

        let format_ext = if is_gzip {
            extension_lowercase(&path.with_extension(""))
        } else {
            outer_ext
        };
        files.push(FileInfo {
            file_type: identify_format(&format_ext),
            path,
            is_gzip,
        });
    }

    files
}

/// Only input file extension then classification the file type and return FileFormat attribute
fn identify_format(file_ext: &str) -> FileFormat {
    let file_type = match file_ext {
        "fa" | "faa" | "pep" | "fasta" | "fna" | "fsa" | "fas" => FileFormat::Fasta,
        "fastq" | "fq" => FileFormat::Fastq,
        "bam" => FileFormat::Bam,
        "sam" => FileFormat::Sam,
        _ => FileFormat::Unknown,
    };

    file_type
}

/// receive input extension and shift the string to lowercase
fn extension_lowercase(path: &Path) -> String {
    path.extension()
        .and_then(|ext| ext.to_str())
        .unwrap_or("")
        .to_ascii_lowercase()
}
