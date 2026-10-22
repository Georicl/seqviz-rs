//! VCF values shared by the data backend and document view.

#[derive(Clone, Debug, Default)]
pub struct VcfHeader {
    pub version: String,
    pub samples: Vec<String>,
    pub contigs: Vec<String>,
    pub definitions: Vec<FieldDefinition>,
    /// Original header lines, including their original line endings.
    pub raw: String,
    pub source: Option<String>,
    pub reference: Option<String>,
}

pub type Header = VcfHeader;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FieldDefinition {
    pub kind: String,
    pub id: String,
    pub number: Option<String>,
    pub value_type: Option<String>,
    pub description: Option<String>,
    pub raw: String,
}

#[derive(Clone, Debug)]
pub struct VcfRecord {
    /// Original record without the terminating CR/LF; all TABs remain intact.
    pub raw: String,
    pub chrom: String,
    pub pos: u64,
    pub ids: Vec<String>,
    pub reference: String,
    pub alternates: Vec<String>,
    pub qual: Option<f64>,
    pub filter: String,
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

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SampleValue {
    pub index: usize,
    pub values: Vec<String>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum VariantKind {
    Snv,
    Mnv,
    Insertion,
    Deletion,
    Symbolic,
    Breakend,
    SpanningDeletion,
    Mixed,
    Other,
}

impl VariantKind {
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

#[derive(Clone, Debug)]
pub struct VariantInfo {
    pub allele_index: usize,
    pub alternate: String,
    pub kind: VariantKind,
    pub length_change: Option<i64>,
}

#[derive(Clone, Debug)]
pub struct VcfEntry {
    pub record: VcfRecord,
    pub variants: Vec<VariantInfo>,
    pub graph: Option<super::graph::GraphAnnotation>,
}

/// Fixed display fields only: no sample or INFO objects are kept in the index.
#[derive(Clone, Debug)]
pub struct RecordSummary {
    pub id: u64,
    pub ordinal: u64,
    pub chrom: String,
    pub pos: u64,
    pub ids: String,
    pub reference: String,
    pub alternates: String,
    pub qual: Option<f64>,
    pub filter: String,
    pub kind: VariantKind,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ContigSummary {
    pub name: String,
    pub records: u64,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Progress {
    pub records: u64,
    pub scanned_bytes: u64,
    pub complete: bool,
    pub errors: u64,
    pub error: Option<String>,
    pub line: u64,
}
