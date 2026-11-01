//! VCF filter semantics, expression parsing and stable ordering preferences.
use super::model::VariantKind;

/// Indexed field used to order matching variants; source ID always breaks ties.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum SortField {
    /// Original input order.
    #[default]
    Source,
    /// Contig name followed by genomic position.
    Position,
    /// Numeric QUAL, with missing values last in either direction.
    Qual,
    /// Displayed variant classification.
    Type,
}

/// Sort direction and indexed field, independent of viewport offsets.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Sort {
    /// Indexed field to order by.
    pub field: SortField,
    /// Reverse the primary field; tied records retain source order.
    pub descending: bool,
}

/// Conjunctive predicates over indexed VCF fields. Positions are positive and inclusive.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Query {
    /// Keep only literal FILTER=PASS; a dot is not PASS.
    pub pass_only: bool,
    /// Inclusive lower QUAL threshold; missing values do not match.
    pub min_qual: Option<f64>,
    /// Inclusive upper QUAL threshold; missing values do not match.
    pub max_qual: Option<f64>,
    /// Displayed record classification, including MIXED for mixed ALT types.
    pub kind: Option<VariantKind>,
    /// Literal, case-sensitive substring of the complete ID field.
    pub id: Option<String>,
    /// Exact contig name, in addition to any active contig-picker selection.
    pub contig: Option<String>,
    /// Inclusive lower POS bound.
    pub start: Option<u64>,
    /// Inclusive upper POS bound.
    pub end: Option<u64>,
    /// Stable ordering of matching records.
    pub sort: Sort,
}

impl Query {
    /// Whether every indexed record matches, ignoring the requested ordering.
    pub fn is_unfiltered(&self) -> bool {
        !self.pass_only
            && self.min_qual.is_none()
            && self.max_qual.is_none()
            && self.kind.is_none()
            && self.id.is_none()
            && self.contig.is_none()
            && self.start.is_none()
            && self.end.is_none()
    }

    /// Parse whitespace-separated predicates such as `pass qual>=30 type=SNV sort=-qual`.
    /// An empty expression resets filters and ordering. Unknown or invalid terms are errors.
    pub fn parse(text: &str) -> Result<Self, String> {
        let mut query = Self::default();
        for term in text.split_whitespace() {
            let lower = term.to_ascii_lowercase();
            if lower == "pass" || lower == "filter=pass" {
                query.pass_only = true;
            } else if let Some(value) = lower.strip_prefix("qual>=") {
                query.min_qual = Some(quality(value)?);
            } else if let Some(value) = lower.strip_prefix("qual<=") {
                query.max_qual = Some(quality(value)?);
            } else if let Some(value) = lower.strip_prefix("type=") {
                query.kind = Some(match value {
                    "snv" | "snp" => VariantKind::Snv,
                    "mnv" => VariantKind::Mnv,
                    "ins" | "insertion" => VariantKind::Insertion,
                    "del" | "deletion" => VariantKind::Deletion,
                    "symbolic" => VariantKind::Symbolic,
                    "bnd" | "breakend" => VariantKind::Breakend,
                    "span_del" => VariantKind::SpanningDeletion,
                    "mixed" => VariantKind::Mixed,
                    "other" => VariantKind::Other,
                    _ => return Err(format!("Unknown variant type: {value}")),
                });
            } else if let Some((key, value)) = term.split_once('=') {
                if value.is_empty() {
                    return Err(format!("Missing value for {key}"));
                }
                match key.to_ascii_lowercase().as_str() {
                    "id" => query.id = Some(value.to_owned()),
                    "contig" => query.contig = Some(value.to_owned()),
                    "start" => query.start = Some(position(value)?),
                    "end" => query.end = Some(position(value)?),
                    "sort" => query.sort = Sort::parse(value)?,
                    _ => return Err(format!("Unknown filter: {key}")),
                }
            } else {
                return Err(format!("Unknown filter term: {term}"));
            }
        }
        query.validate()?;
        Ok(query)
    }

    /// Validate manually constructed filters as well as parsed expressions.
    pub fn validate(&self) -> Result<(), String> {
        for value in [self.min_qual, self.max_qual].into_iter().flatten() {
            if !value.is_finite() || value < 0.0 {
                return Err("QUAL must be a finite nonnegative number".into());
            }
        }
        if self.min_qual.zip(self.max_qual).is_some_and(|(a, b)| a > b) {
            return Err("Minimum QUAL exceeds maximum QUAL".into());
        }
        for value in [self.start, self.end].into_iter().flatten() {
            if value == 0 || value > i64::MAX as u64 {
                return Err("Position must be between 1 and the supported integer maximum".into());
            }
        }
        if self.start.zip(self.end).is_some_and(|(a, b)| a > b) {
            return Err("Start position exceeds end position".into());
        }
        Ok(())
    }

    /// Return the editable canonical filter expression.
    pub fn summary(&self) -> String {
        let mut terms = Vec::new();
        if self.pass_only {
            terms.push("pass".into());
        }
        if let Some(value) = self.min_qual {
            terms.push(format!("qual>={value}"));
        }
        if let Some(value) = self.max_qual {
            terms.push(format!("qual<={value}"));
        }
        if let Some(kind) = self.kind {
            terms.push(format!("type={}", kind.label()));
        }
        if let Some(value) = &self.id {
            terms.push(format!("id={value}"));
        }
        if let Some(value) = &self.contig {
            terms.push(format!("contig={value}"));
        }
        if let Some(value) = self.start {
            terms.push(format!("start={value}"));
        }
        if let Some(value) = self.end {
            terms.push(format!("end={value}"));
        }
        terms.push(format!("sort={}", self.sort.label()));
        terms.join(" ")
    }
}

impl Sort {
    /// Parse source/pos/qual/type, prefixed with `-` for descending order.
    pub fn parse(text: &str) -> Result<Self, String> {
        let descending = text.starts_with('-');
        let field = match text
            .trim_start_matches(['-', '+'])
            .to_ascii_lowercase()
            .as_str()
        {
            "source" => SortField::Source,
            "pos" | "position" => SortField::Position,
            "qual" => SortField::Qual,
            "type" => SortField::Type,
            _ => return Err("Sort must be source, pos, qual or type; prefix - to reverse".into()),
        };
        Ok(Self { field, descending })
    }

    /// Return the expression accepted by the filter editor.
    pub fn label(self) -> String {
        let field = match self.field {
            SortField::Source => "source",
            SortField::Position => "pos",
            SortField::Qual => "qual",
            SortField::Type => "type",
        };
        format!("{}{field}", if self.descending { "-" } else { "" })
    }
}

fn quality(value: &str) -> Result<f64, String> {
    let value: f64 = value
        .parse()
        .map_err(|_| "QUAL must be numeric".to_owned())?;
    if !value.is_finite() || value < 0.0 {
        return Err("QUAL must be a finite nonnegative number".into());
    }
    Ok(value)
}

fn position(value: &str) -> Result<u64, String> {
    let value = value
        .parse::<u64>()
        .map_err(|_| "Position must be a positive integer".to_owned())?;
    if value == 0 || value > i64::MAX as u64 {
        return Err("Position exceeds the supported positive range".into());
    }
    Ok(value)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn expressions_round_trip_and_reject_invalid_predicates() {
        let query = Query::parse(
            "pass qual>=20.5 qual<=60 type=SNP id=rs contig=chr1 start=10 end=90 sort=-qual",
        )
        .unwrap();
        assert_eq!(Query::parse(&query.summary()).unwrap(), query);
        for text in [
            "qual>=NaN",
            "qual>=-1",
            "qual>=40 qual<=20",
            "start=0",
            "start=10 end=2",
            "type=wat",
            "sort=raw",
            "unknown=1",
        ] {
            assert!(Query::parse(text).is_err(), "{text}");
        }
        assert_eq!(Query::parse("").unwrap(), Query::default());
    }
}
