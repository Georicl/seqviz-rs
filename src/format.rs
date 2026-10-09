//! 根据输入路径的后缀识别文件格式和 gzip 标记。
use std::path::{Path, PathBuf};

/// 文件后缀对应的格式。
#[derive(Debug)]
pub enum FileFormat {
    Fasta,
    Fastq,
    Sam,
    Bam,
    Unknown,
}

/// 保存输入路径和按文件名识别的格式信息。
pub struct FileInfo {
    pub file_type: FileFormat,

    /// 输入时的完整路径，保留 `.gz` 后缀。
    pub path: PathBuf,
    /// 末尾是否带 `.gz` 后缀，忽略 ASCII 大小写。
    pub is_gzip: bool,
}

/// 逐个识别输入文件的格式和 `.gz` 后缀。
///
/// 带 `.gz` 后缀时，用前一个后缀判断格式。
/// 按输入顺序返回，保留原始路径；文件留到读取阶段再打开。
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

/// 在打开文件之前，根据小写后缀判断格式。
fn identify_format(file_ext: &str) -> FileFormat {
    match file_ext {
        "fa" | "faa" | "pep" | "fasta" | "fna" | "fsa" | "fas" => FileFormat::Fasta,
        "fastq" | "fq" => FileFormat::Fastq,
        "bam" => FileFormat::Bam,
        "sam" => FileFormat::Sam,
        _ => FileFormat::Unknown,
    }
}

/// 取得路径后缀并转为小写。
fn extension_lowercase(path: &Path) -> String {
    path.extension()
        .and_then(|ext| ext.to_str())
        .unwrap_or("")
        .to_ascii_lowercase()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn recognizes_supported_extensions() {
        for extension in ["fa", "faa", "pep", "fasta", "fna", "fsa", "fas"] {
            assert!(matches!(identify_format(extension), FileFormat::Fasta));
        }
        for extension in ["fq", "fastq"] {
            assert!(matches!(identify_format(extension), FileFormat::Fastq));
        }
        assert!(matches!(identify_format("sam"), FileFormat::Sam));
        assert!(matches!(identify_format("bam"), FileFormat::Bam));
    }

    #[test]
    fn recognizes_uppercase_gzip_and_preserves_original_paths() {
        let paths = vec![
            PathBuf::from("reads.FASTQ.GZ"),
            PathBuf::from("protein.FAA"),
        ];
        let files = receive_files(paths.clone());

        assert!(matches!(files[0].file_type, FileFormat::Fastq));
        assert!(files[0].is_gzip);
        assert!(matches!(files[1].file_type, FileFormat::Fasta));
        assert!(!files[1].is_gzip);
        assert_eq!(files[0].path, paths[0]);
        assert_eq!(files[1].path, paths[1]);
    }

    #[test]
    fn unknown_or_missing_extensions_remain_unknown() {
        let files = receive_files(vec![PathBuf::from("notes.txt.gz"), PathBuf::from("reads")]);

        assert!(matches!(files[0].file_type, FileFormat::Unknown));
        assert!(files[0].is_gzip);
        assert!(matches!(files[1].file_type, FileFormat::Unknown));
        assert!(!files[1].is_gzip);
    }
}
