//! VCF values shared by the data backend and document view.

/// Parsed header metadata together with the untouched original header text.
#[derive(Clone, Debug, Default)]
pub struct VcfHeader {
    /// Validated VCF version string, including the VCFv prefix.
    pub version: String,
    /// Sample names in original column order.
    pub samples: Vec<String>,
    /// Distinct declared contigs in header order.
    pub contigs: Vec<String>,
    /// Structured metadata entries that declare an ID.
    pub definitions: Vec<FieldDefinition>,
    /// Original header lines, including their original line endings.
    pub raw: String,
    /// Value of the last source metadata line, when present.
    pub source: Option<String>,
    /// Value of the last reference metadata line, when present.
    pub reference: Option<String>,
}

/// Short name for the VCF header shared with backend callers.
pub type Header = VcfHeader;

/// One structured metadata declaration with its original header line.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FieldDefinition {
    /// Metadata category, such as INFO, FORMAT, FILTER or contig.
    pub kind: String,
    /// Declared ID within the metadata category.
    pub id: String,
    /// Original Number token, retaining symbolic and local-allele cardinalities.
    pub number: Option<String>,
    /// Declared Type token, if applicable.
    pub value_type: Option<String>,
    /// Decoded Description text, if present.
    pub description: Option<String>,
    /// Original metadata line without its line ending.
    pub raw: String,
}

/// Fixed VCF fields plus original text for lazy INFO and sample access.
#[derive(Clone, Debug)]
pub struct VcfRecord {
    /// Original record without the terminating CR/LF; all TABs remain intact.
    pub raw: String,
    /// Original CHROM identifier.
    pub chrom: String,
    /// Original VCF POS coordinate; zero is preserved for a telomeric position.
    pub pos: u64,
    /// Semicolon-separated identifiers; an ID field of dot becomes an empty list.
    pub ids: Vec<String>,
    /// Original REF allele string.
    pub reference: String,
    /// ALT alleles in source order; a dot becomes an empty list.
    pub alternates: Vec<String>,
    /// Finite, nonnegative QUAL value, or None for a missing dot.
    pub qual: Option<f64>,
    /// Original FILTER text, retaining PASS, dot and named filters.
    pub filter: String,
    /// FORMAT keys in sample-value order; a missing dot produces an empty list.
    pub format: Vec<String>,
}

impl VcfRecord {
    /// Decode only INFO when its panel is requested. Flag values use an empty string.
    pub fn info_fields(&self) -> Vec<(String, String)> {
        self.raw
            .split('\t')
            .nth(7)
            .filter(|field| *field != ".")
            .into_iter()
            .flat_map(|field| field.split(';'))
            .filter(|field| !field.is_empty())
            .map(|field| {
                let (key, value) = field.split_once('=').unwrap_or((field, ""));
                (key.to_owned(), value.to_owned())
            })
            .collect()
    }

    /// Materialize one requested sample page, without decoding other sample columns.
    /// `start` is a zero-based sample index; omitted trailing FORMAT values become dots.
    pub fn sample_values(&self, start: usize, count: usize) -> Vec<SampleValue> {
        self.raw
            .split('\t')
            .skip(9)
            .enumerate()
            .skip(start)
            .take(count)
            .map(|(index, text)| {
                let mut values = text.split(':').map(str::to_owned).collect::<Vec<_>>();
                if values.len() < self.format.len() {
                    values.resize(self.format.len(), ".".into());
                }
                SampleValue { index, values }
            })
            .collect()
    }
}

/// One sample column decoded into values in FORMAT order.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SampleValue {
    /// Zero-based sample index in the header.
    pub index: usize,
    /// Original colon-separated values, padded with dots for omitted trailing FORMAT fields.
    pub values: Vec<String>,
}

/// Display classification inferred from REF/ALT syntax and allele lengths.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum VariantKind {
    /// Single-base REF and ALT alleles.
    Snv,
    /// Equal-length literal alleles longer than one base.
    Mnv,
    /// Literal ALT longer than REF.
    Insertion,
    /// Literal ALT shorter than REF.
    Deletion,
    /// Angle-bracket symbolic allele, such as `<DEL>`.
    Symbolic,
    /// Paired bracket notation or a leading/trailing-dot single breakend.
    Breakend,
    /// The `*` allele representing a spanning deletion.
    SpanningDeletion,
    /// Record summary whose ALT alleles have different classifications.
    Mixed,
    /// Missing or otherwise unclassified allele.
    Other,
}

impl VariantKind {
    /// Return the compact label shown in the variant table.
    pub fn label(self) -> &'static str {
        match self {
            Self::Snv => "SNV",
            Self::Mnv => "MNV",
            Self::Insertion => "INS",
            Self::Deletion => "DEL",
            Self::Symbolic => "SYMBOLIC",
            Self::Breakend => "BND",
            Self::SpanningDeletion => "SPAN_DEL",
            Self::Mixed => "MIXED",
            Self::Other => "OTHER",
        }
    }
}

/// Classification of one ALT allele with its original genotype allele index.
#[derive(Clone, Debug)]
pub struct VariantInfo {
    /// One-based ALT index used by genotype alleles; zero denotes REF elsewhere.
    pub allele_index: usize,
    /// Original ALT allele text.
    pub alternate: String,
    /// Classification inferred for this ALT independently of the other alleles.
    pub kind: VariantKind,
    /// ALT minus REF length for literal sequence alleles; absent for structural symbols.
    pub length_change: Option<i64>,
}

/// A selected record, per-ALT classifications and optional graph annotations.
#[derive(Clone, Debug)]
pub struct VcfEntry {
    /// Complete selected record, including original text.
    pub record: VcfRecord,
    /// One classification per ALT allele in source order.
    pub variants: Vec<VariantInfo>,
    /// Annotations supplied only by an explicitly supported graph adapter.
    pub graph: Option<super::graph::GraphAnnotation>,
}

/// Fixed display fields only: no sample or INFO objects are kept in the index.
#[derive(Clone, Debug)]
pub struct RecordSummary {
    /// Zero-based record ID in source order across all contigs.
    pub id: u64,
    /// Zero-based position in the current filtered table; equals id without a contig filter.
    pub ordinal: u64,
    /// Contig identifier used for table display and filtering.
    pub chrom: String,
    /// Original VCF POS coordinate.
    pub pos: u64,
    /// Identifier text shortened to at most 64 characters.
    pub ids: String,
    /// REF display text shortened to at most 64 characters.
    pub reference: String,
    /// Comma-joined ALT display text shortened to at most 64 characters.
    pub alternates: String,
    /// QUAL value, or None for a missing dot.
    pub qual: Option<f64>,
    /// FILTER display text shortened to at most 64 characters.
    pub filter: String,
    /// Common ALT classification, Mixed for differing kinds, or Other when ALT is absent.
    pub kind: VariantKind,
}

/// A contig name and its currently indexed record count.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ContigSummary {
    /// Original contig identifier.
    pub name: String,
    /// Number of records currently indexed for this contig.
    pub records: u64,
}

/// Decoded-source scan progress and any input or backend error shown to the view.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Progress {
    /// Number of successfully indexed records.
    pub records: u64,
    /// Decoded byte position reached; a reopened index resumes at a complete-line boundary.
    pub scanned_bytes: u64,
    /// Whether scanning reached EOF and completed the source index.
    pub complete: bool,
    /// Number of reported input errors.
    pub errors: u64,
    /// Input error with source context, or a backend failure attached to a snapshot.
    pub error: Option<String>,
    /// Number of physical lines processed, including the header and any failing record.
    pub line: u64,
}
