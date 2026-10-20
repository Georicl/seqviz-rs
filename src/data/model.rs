//! Public values shared by data, worker and UI; no storage or terminal dependencies.

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Format {
    Fasta,
    Fastq,
    Sam,
    Bam,
}
impl Format {
    pub fn can_copy(self) -> bool {
        matches!(self, Self::Fasta | Self::Fastq)
    }
    /// Alignment formats carry reference-coordinate fields and stored read orientation.
    pub fn is_alignment(self) -> bool {
        matches!(self, Self::Sam | Self::Bam)
    }
    pub fn supports_n50(self) -> bool {
        matches!(self, Self::Fasta | Self::Fastq)
    }
    pub fn name(self) -> &'static str {
        match self {
            Self::Fasta => "FASTA",
            Self::Fastq => "FASTQ",
            Self::Sam => "SAM",
            Self::Bam => "BAM",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SeqType {
    Dna,
    Protein,
    Unknown,
}

#[derive(Debug, Clone)]
pub struct RecordSummary {
    pub id: u64,
    pub name: String,
    pub length: u64,
    pub complete: bool,
}

#[derive(Debug, Clone)]
pub struct RecordMeta {
    pub id: u64,
    pub name: String,
    /// The complete defline without the leading `>` or `@`.
    pub header: String,
    /// FASTQ's complete separator line, including its leading `+`.
    pub plus: String,
    pub length: u64,
    pub quality_length: u64,
    pub complete: bool,
    pub seq_type: SeqType,
    pub gc: Option<f64>,
    pub quality: Option<QualityStats>,
    pub fields: Vec<(String, String)>,
    pub has_sequence: bool,
    pub has_quality: bool,
}

#[derive(Debug, Clone)]
pub struct QualityStats {
    pub min: u8,
    pub max: u8,
    pub mean: f64,
    /// Percentage, in the range 0..=100.
    pub q30: f64,
    /// Mean Phred score in each successive positional bin.
    pub overview: Vec<f64>,
}

#[derive(Debug, Clone, Default)]
pub struct SequenceWindow {
    pub start: u64,
    pub sequence: Vec<u8>,
    /// Phred+33 ASCII characters, including for BAM input.
    pub quality: Vec<u8>,
}

#[derive(Debug, Clone, Default)]
pub struct Progress {
    pub records: u64,
    pub complete: bool,
    pub scanned_bytes: u64,
    pub errors: u64,
    /// The most recent input error, including recoverable SAM row errors.
    pub last_error: Option<String>,
    /// A fatal error stops this file. Recoverable SAM errors only increment errors.
    pub error: Option<String>,
}

#[derive(Debug, Clone, Default)]
pub struct Stats {
    pub records: u64,
    pub sequence_records: u64,
    pub total_length: u64,
    pub min_length: Option<u64>,
    pub max_length: Option<u64>,
    pub mean_length: Option<f64>,
    pub n50: Option<u64>,
    pub gc: Option<f64>,
    pub protein: bool,
    pub complete: bool,
    pub errors: u64,
}
