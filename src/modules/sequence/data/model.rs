//! Public values shared by data, worker and UI; no storage or terminal dependencies.

/// Supported record formats after source decompression.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Format {
    /// Defline-based text records containing sequence.
    Fasta,
    /// Text records containing sequence and Phred+33 quality.
    Fastq,
    /// Tab-delimited alignment records with optional sequence and quality.
    Sam,
    /// Binary alignment records decoded from logical BAM bytes.
    Bam,
}
impl Format {
    /// Whether this format supports reconstructing records for the clipboard.
    pub fn can_copy(self) -> bool {
        matches!(self, Self::Fasta | Self::Fastq)
    }
    /// Alignment formats carry reference-coordinate fields and stored read orientation.
    pub fn is_alignment(self) -> bool {
        matches!(self, Self::Sam | Self::Bam)
    }
    /// Whether length statistics include N50 for this format.
    pub fn supports_n50(self) -> bool {
        matches!(self, Self::Fasta | Self::Fastq)
    }
    /// Return the conventional uppercase format label.
    pub fn name(self) -> &'static str {
        match self {
            Self::Fasta => "FASTA",
            Self::Fastq => "FASTQ",
            Self::Sam => "SAM",
            Self::Bam => "BAM",
        }
    }
}

/// Sequence alphabet inferred from a sample or forced by the input format or extension.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SeqType {
    /// Nucleotide alphabet, including supported ambiguity symbols and uracil.
    Dna,
    /// Protein alphabet inferred from diagnostic residues or selected by the file extension.
    Protein,
    /// Sample not confidently classified as nucleotide or protein.
    Unknown,
}

/// Compact list entry for an indexed record, including records still being scanned.
#[derive(Debug, Clone)]
pub struct RecordSummary {
    /// Zero-based record ID in source order.
    pub id: u64,
    /// First whitespace-delimited token of the defline or alignment read name.
    pub name: String,
    /// Number of sequence symbols indexed so far.
    pub length: u64,
    /// Whether the scanner has reached the end of this record.
    pub complete: bool,
}

/// Indexed record details and statistics used by the selected-record view.
#[derive(Debug, Clone)]
pub struct RecordMeta {
    /// Zero-based record ID in source order.
    pub id: u64,
    /// Record identifier without the remaining defline description.
    pub name: String,
    /// The complete defline without the leading `>` or `@`.
    pub header: String,
    /// FASTQ's complete separator line, including its leading `+`.
    pub plus: String,
    /// Number of sequence symbols indexed so far.
    pub length: u64,
    /// Number of quality scores indexed so far.
    pub quality_length: u64,
    /// Whether the record has been fully scanned.
    pub complete: bool,
    /// Sequence alphabet selected from the format, extension or sampled sequence.
    pub seq_type: SeqType,
    /// GC percentage for a complete, nonempty DNA record; otherwise absent.
    pub gc: Option<f64>,
    /// Aggregates for a complete record with at least one quality score.
    pub quality: Option<QualityStats>,
    /// Ordered header or alignment metadata displayed for this record.
    pub fields: Vec<(String, String)>,
    /// Whether sequence is present, independently of its current indexed length.
    pub has_sequence: bool,
    /// Whether at least one quality score has been indexed.
    pub has_quality: bool,
}

/// Phred quality statistics for a completed record with quality data.
#[derive(Debug, Clone)]
pub struct QualityStats {
    /// Lowest numeric Phred score.
    pub min: u8,
    /// Highest numeric Phred score.
    pub max: u8,
    /// Arithmetic mean of numeric Phred scores.
    pub mean: f64,
    /// Percentage, in the range 0..=100.
    pub q30: f64,
    /// Integer-truncated mean Phred score in each successive positional bin.
    pub overview: Vec<f64>,
}

/// Decoded sequence and quality bytes for a requested zero-based interval.
#[derive(Debug, Clone, Default)]
pub struct SequenceWindow {
    /// Requested zero-based sequence coordinate of the window.
    pub start: u64,
    /// Decoded sequence symbols available in the requested interval.
    pub sequence: Vec<u8>,
    /// Phred+33 ASCII characters, including for BAM input.
    pub quality: Vec<u8>,
}

/// Committed scan status, including partial records and recoverable or fatal errors.
#[derive(Debug, Clone, Default)]
pub struct Progress {
    /// Number of indexed records, including a current partial record.
    pub records: u64,
    /// Whether scanning reached clean end of input, even if recoverable errors occurred.
    pub complete: bool,
    /// Logical, decompressed source bytes consumed by committed scan steps.
    pub scanned_bytes: u64,
    /// Number of recorded recoverable and fatal input errors.
    pub errors: u64,
    /// The most recent input error, including recoverable SAM row errors.
    pub last_error: Option<String>,
    /// A fatal error stops this file. Recoverable SAM errors only increment errors.
    pub error: Option<String>,
}

/// One fixed histogram interval, including empty intervals for comparable displays.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HistogramBin {
    /// Human-readable interval; brackets describe inclusive and exclusive boundaries.
    pub label: &'static str,
    /// Number of completed records whose measurement lies in this interval.
    pub count: u64,
}

/// Quality aggregates over completed records containing at least one quality score.
#[derive(Debug, Clone, Default)]
pub struct FileQualityStats {
    /// Number of completed records contributing quality scores.
    pub records: u64,
    /// Total number of quality scores, used to weight the file-wide mean and Q30.
    pub scores: u64,
    /// Mean numeric Phred score across all contributing quality scores.
    pub mean: f64,
    /// Percentage of contributing scores at or above Phred 30.
    pub q30: f64,
}

/// FLAG-based counts over completed SAM/BAM alignment records, including missing SEQ.
#[derive(Debug, Clone, Default)]
pub struct MappingStats {
    /// Number of completed alignment records with a FLAG value.
    pub alignments: u64,
    /// Alignments whose unmapped flag (0x4) is absent.
    pub mapped: u64,
    /// Alignments whose unmapped flag (0x4) is present.
    pub unmapped: u64,
    /// Alignments with neither the secondary (0x100) nor supplementary (0x800) flag.
    pub primary: u64,
    /// Alignments carrying the secondary flag (0x100).
    pub secondary: u64,
    /// Alignments carrying the supplementary flag (0x800).
    pub supplementary: u64,
    /// Alignments carrying the duplicate flag (0x400), without deduplication.
    pub duplicate: u64,
}

/// File-wide scan counts and aggregates over completed applicable records.
#[derive(Debug, Clone, Default)]
pub struct Stats {
    /// Number of all indexed records, including a current partial record.
    pub records: u64,
    /// Number of completed records that have sequence.
    pub sequence_records: u64,
    /// Sum of lengths over completed sequence-bearing records.
    pub total_length: u64,
    /// Shortest completed sequence length, if any sequence-bearing record exists.
    pub min_length: Option<u64>,
    /// Longest completed sequence length, if any sequence-bearing record exists.
    pub max_length: Option<u64>,
    /// Arithmetic mean length of completed sequence-bearing records.
    pub mean_length: Option<f64>,
    /// Length at which descending lengths reach half the total, for FASTA and FASTQ only.
    pub n50: Option<u64>,
    /// GC percentage when every included record is DNA and total length is nonzero.
    pub gc: Option<f64>,
    /// Whether every included record is protein and at least one record is included.
    pub protein: bool,
    /// Whether scanning finished with no recorded input errors.
    pub complete: bool,
    /// Number of recorded input errors.
    pub errors: u64,
    /// Length histogram over completed sequence-bearing records, including empty sequences.
    pub length_distribution: Vec<HistogramBin>,
    /// Per-record GC-percentage histogram over completed nonempty DNA sequences only.
    pub gc_distribution: Vec<HistogramBin>,
    /// Per-record mean Phred histogram over completed records with quality scores.
    pub quality_distribution: Vec<HistogramBin>,
    /// File-wide quality aggregates, absent when no completed record has quality scores.
    pub quality: Option<FileQualityStats>,
    /// SAM/BAM FLAG summary; absent for sequence formats without alignment metadata.
    pub mapping: Option<MappingStats>,
}
