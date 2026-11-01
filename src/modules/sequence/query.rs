//! Record query values shared by the sequence data layer, worker and document view.

/// Optional predicates combined with logical AND; missing measurements never match a bound.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct RecordFilter {
    /// Case-insensitive literal substring of the record identifier.
    pub name: String,
    /// Inclusive minimum sequence length; applies only to completed records with sequence.
    pub min_length: Option<u64>,
    /// Inclusive maximum sequence length; applies only to completed records with sequence.
    pub max_length: Option<u64>,
    /// Inclusive minimum GC percentage for completed, nonempty DNA records.
    pub min_gc: Option<f64>,
    /// Inclusive maximum GC percentage for completed, nonempty DNA records.
    pub max_gc: Option<f64>,
    /// Inclusive minimum mean Phred score for completed records with quality.
    pub min_quality: Option<f64>,
    /// Inclusive maximum mean Phred score for completed records with quality.
    pub max_quality: Option<f64>,
    /// Inclusive minimum SAM/BAM mapping quality.
    pub min_mapq: Option<u8>,
    /// SAM/BAM FLAG bits that must all be present.
    pub required_flags: u16,
    /// SAM/BAM FLAG bits that must all be absent.
    pub excluded_flags: u16,
}

impl RecordFilter {
    /// Parse whitespace-separated `key=value` clauses; an empty string clears the filter.
    /// Keys are name, minlen, maxlen, mingc, maxgc, minq, maxq, mapq, flag and noflag.
    /// FLAG masks accept decimal and `0x` hexadecimal notation.
    pub fn parse(text: &str) -> Result<Self, String> {
        let mut filter = Self::default();
        for clause in text.split_whitespace() {
            let (key, value) = clause
                .split_once('=')
                .ok_or_else(|| format!("Expected key=value, got {clause:?}"))?;
            if value.is_empty() {
                return Err(format!("Missing value for {key}"));
            }
            match key {
                "name" => filter.name = value.into(),
                "minlen" => filter.min_length = Some(length(value)?),
                "maxlen" => filter.max_length = Some(length(value)?),
                "mingc" => filter.min_gc = Some(measurement(value, 100.0)?),
                "maxgc" => filter.max_gc = Some(measurement(value, 100.0)?),
                "minq" => filter.min_quality = Some(measurement(value, 93.0)?),
                "maxq" => filter.max_quality = Some(measurement(value, 93.0)?),
                "mapq" => filter.min_mapq = Some(value.parse().map_err(|_| "MAPQ must be 0..255")?),
                "flag" => filter.required_flags = mask(value)?,
                "noflag" => filter.excluded_flags = mask(value)?,
                _ => return Err(format!("Unknown filter {key:?}")),
            }
        }
        if filter
            .min_length
            .zip(filter.max_length)
            .is_some_and(|(a, b)| a > b)
            || filter.min_gc.zip(filter.max_gc).is_some_and(|(a, b)| a > b)
            || filter
                .min_quality
                .zip(filter.max_quality)
                .is_some_and(|(a, b)| a > b)
        {
            return Err("Minimum must not exceed maximum".into());
        }
        if filter.required_flags & filter.excluded_flags != 0 {
            return Err("flag and noflag must not require and exclude the same bit".into());
        }
        Ok(filter)
    }
}

/// A record property available for deterministic SQLite ordering.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum SortKey {
    /// Original source record ID.
    #[default]
    Source,
    /// Case-insensitive record identifier.
    Name,
    /// Completed sequence length.
    Length,
    /// GC percentage of completed DNA records.
    Gc,
    /// Mean Phred score of completed records with quality.
    Quality,
    /// SAM/BAM mapping quality.
    Mapq,
}

/// Sort direction and key; source IDs break every property tie in ascending order.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct RecordSort {
    /// Record property used as the primary ordering key.
    pub key: SortKey,
    /// Whether larger property values precede smaller values; missing values stay last.
    pub descending: bool,
}

impl RecordSort {
    /// Cycle source, name, decreasing length, GC and quality; alignment inputs also offer MAPQ.
    pub fn next(self, alignment: bool) -> Self {
        let key = match self.key {
            SortKey::Source => SortKey::Name,
            SortKey::Name => SortKey::Length,
            SortKey::Length => SortKey::Gc,
            SortKey::Gc => SortKey::Quality,
            SortKey::Quality if alignment => SortKey::Mapq,
            _ => SortKey::Source,
        };
        Self {
            key,
            descending: !matches!(key, SortKey::Source | SortKey::Name),
        }
    }

    /// Human-readable ordering label used by the record pane and status line.
    pub fn label(self) -> &'static str {
        match (self.key, self.descending) {
            (SortKey::Source, false) => "source",
            (SortKey::Source, true) => "source ↓",
            (SortKey::Name, false) => "name ↑",
            (SortKey::Name, true) => "name ↓",
            (SortKey::Length, false) => "length ↑",
            (SortKey::Length, true) => "length ↓",
            (SortKey::Gc, false) => "GC ↑",
            (SortKey::Gc, true) => "GC ↓",
            (SortKey::Quality, false) => "quality ↑",
            (SortKey::Quality, true) => "quality ↓",
            (SortKey::Mapq, false) => "MAPQ ↑",
            (SortKey::Mapq, true) => "MAPQ ↓",
        }
    }
}

/// Immutable query shared by window requests; record IDs retain their original source identity.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct RecordQuery {
    /// Predicates applied before computing result positions and page windows.
    pub filter: RecordFilter,
    /// Deterministic ordering of the filtered records.
    pub sort: RecordSort,
}

/// Parse a sequence length that SQLite can represent without wrapping a signed integer.
fn length(text: &str) -> Result<u64, String> {
    text.parse::<u64>()
        .ok()
        .filter(|&n| n <= i64::MAX as u64)
        .ok_or_else(|| "Length must be a nonnegative 64-bit signed integer".into())
}

/// Parse a finite percentage or mean quality score within its physical range.
fn measurement(text: &str, maximum: f64) -> Result<f64, String> {
    text.parse::<f64>()
        .ok()
        .filter(|n| n.is_finite() && *n >= 0.0 && *n <= maximum)
        .ok_or_else(|| format!("Value must be between 0 and {maximum}"))
}

/// Parse the unsigned 16-bit SAM flag mask used by bitwise predicates.
fn mask(text: &str) -> Result<u16, String> {
    text.strip_prefix("0x")
        .map_or_else(|| text.parse(), |hex| u16::from_str_radix(hex, 16))
        .map_err(|_| "FLAG mask must be a decimal or hexadecimal 16-bit integer".into())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Reject invalid intervals, nonfinite measurements, unknown keys and conflicting FLAG masks.
    #[test]
    fn filter_validation_keeps_queries_unambiguous() {
        let filter = RecordFilter::parse(
            "name=read minlen=10 maxlen=90 mingc=30 maxgc=60 minq=20 mapq=30 flag=0x1 noflag=0x904",
        )
        .unwrap();
        assert_eq!(filter.required_flags, 1);
        assert_eq!(filter.excluded_flags, 0x904);
        for text in [
            "minlen=10 maxlen=9",
            "mingc=NaN",
            "minq=94",
            "mapq=256",
            "flag=4 noflag=4",
            "unknown=1",
            "name=",
        ] {
            assert!(RecordFilter::parse(text).is_err(), "{text}");
        }
        assert_eq!(RecordFilter::parse("").unwrap(), RecordFilter::default());
    }
}
