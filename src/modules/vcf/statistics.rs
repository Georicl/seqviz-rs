//! Bounded aggregate values over the currently indexed and filtered VCF records.
use super::model::VariantKind;

/// One explicit statistics request, never recomputed as part of ordinary redraws.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Statistics {
    /// Matching indexed records, including records with missing QUAL.
    pub records: u64,
    /// Whether the source index has reached EOF without an input error.
    pub complete: bool,
    /// Active contig-picker selection and filter expression for this result.
    pub scope: String,
    /// Counts for each displayed record classification, in fixed display order.
    pub kinds: Vec<(VariantKind, u64)>,
    /// Literal PASS records.
    pub pass: u64,
    /// FILTER dot records; they are separate from PASS.
    pub filter_missing: u64,
    /// Named non-PASS filter records.
    pub filtered: u64,
    /// Minimum present QUAL.
    pub qual_min: Option<f64>,
    /// Maximum present QUAL.
    pub qual_max: Option<f64>,
    /// Mean over records with a present QUAL.
    pub qual_mean: Option<f64>,
    /// QUAL bins: missing, <20, [20,30), [30,60), >=60.
    pub qual_bins: [u64; 5],
    /// Genotypes with every allele present, including phased and polyploid calls.
    pub called_genotypes: u64,
    /// Genotypes with every allele missing.
    pub missing_genotypes: u64,
    /// Genotypes with a mixture of present and missing alleles.
    pub partial_genotypes: u64,
    /// Sample values in records that have no GT FORMAT field.
    pub unavailable_genotypes: u64,
}

impl Statistics {
    /// Build a selectable terminal report with explicit scope and partial-index semantics.
    pub fn lines(&self) -> Vec<String> {
        let number = |value: Option<f64>| value.map_or_else(|| ".".into(), |n| format!("{n:.2}"));
        let mut lines = vec![
            "VCF STATISTICS".into(),
            format!("Scope: {}", self.scope),
            format!(
                "Index: {}",
                if self.complete {
                    "complete"
                } else {
                    "partial - indexed records only"
                }
            ),
            format!("Matching records: {}", self.records),
            String::new(),
            "RECORD TYPES (one classification per record)".into(),
        ];
        lines.extend(
            self.kinds
                .iter()
                .map(|(kind, count)| format!("{}: {count}", kind.label())),
        );
        lines.extend([
            String::new(),
            "FILTER".into(),
            format!("PASS: {}", self.pass),
            format!("Missing (.): {}", self.filter_missing),
            format!("Named non-PASS: {}", self.filtered),
            String::new(),
            "QUALITY".into(),
            format!(
                "QUAL min / mean / max: {} / {} / {}",
                number(self.qual_min),
                number(self.qual_mean),
                number(self.qual_max)
            ),
        ]);
        for (label, count) in ["Missing", "<20", "20–<30", "30–<60", ">=60"]
            .into_iter()
            .zip(self.qual_bins)
        {
            lines.push(format!("QUAL {label}: {count}"));
        }
        let known = self.called_genotypes + self.missing_genotypes + self.partial_genotypes;
        let incomplete = self.missing_genotypes + self.partial_genotypes;
        lines.extend([
            String::new(),
            "GENOTYPES (all samples in matching records)".into(),
            format!("Fully called: {}", self.called_genotypes),
            format!("Fully missing: {}", self.missing_genotypes),
            format!("Partially missing: {}", self.partial_genotypes),
            format!("Without GT field: {}", self.unavailable_genotypes),
            format!(
                "Missing or partial / GT values: {}",
                if known == 0 {
                    ".".into()
                } else {
                    format!("{:.2}%", incomplete as f64 * 100.0 / known as f64)
                }
            ),
        ]);
        lines
    }
}
