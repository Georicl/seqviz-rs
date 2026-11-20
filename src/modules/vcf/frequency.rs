//! Allele counts per VCF record. Sample indices always refer to the original header.
use super::model::VcfRecord;

/// Visit selected sample columns in source order. None includes all samples.
pub(crate) fn genotypes<'a>(
    record: &'a VcfRecord,
    samples: Option<&'a [usize]>,
) -> impl Iterator<Item = Option<&'a str>> {
    let field = record.format.iter().position(|key| key == "GT");
    let limit = samples.map_or(usize::MAX, |indices| indices.last().map_or(0, |i| i + 1));
    let mut selected = samples.unwrap_or_default().iter().copied().peekable();
    record
        .raw
        .split('\t')
        .skip(9)
        .take(limit)
        .enumerate()
        .filter(move |(index, _)| samples.is_none() || selected.next_if_eq(index).is_some())
        .map(move |(_, value)| field.map(|i| value.split(':').nth(i).unwrap_or(".")))
}

/// Frequencies in ALT order; the denominator includes known reference copies.
pub(crate) struct Frequencies {
    /// Count of each alternate allele in called copies.
    pub counts: Vec<u64>,
    /// Count of known, valid allele copies.
    pub total: u64,
    /// Frequencies supplied by the file, independent of sample selection.
    pub info: Vec<Option<f64>>,
}

impl Frequencies {
    /// Calculate per-ALT counts for all or explicitly selected samples.
    pub fn read(record: &VcfRecord, samples: Option<&[usize]>) -> Self {
        let mut counts = vec![0; record.alternates.len()];
        let mut total = 0;
        for gt in genotypes(record, samples).flatten() {
            for allele in gt.split(['/', '|']) {
                if let Ok(index) = allele.parse::<usize>() {
                    if index == 0 {
                        total += 1;
                    } else if let Some(count) = counts.get_mut(index - 1) {
                        *count += 1;
                        total += 1;
                    }
                }
            }
        }
        let raw = record
            .raw
            .split('\t')
            .nth(7)
            .and_then(|info| info.split(';').find_map(|field| field.strip_prefix("AF=")));
        let mut values = raw.into_iter().flat_map(|value| value.split(','));
        let info = (0..counts.len())
            .map(|_| {
                values
                    .next()?
                    .parse::<f64>()
                    .ok()
                    .filter(|v| (0.0..=1.0).contains(v))
            })
            .collect();
        Self {
            counts,
            total,
            info,
        }
    }

    /// Fraction of known copies carrying the given ALT, or None for an empty denominator.
    pub fn gt_frequency(&self, index: usize) -> Option<f64> {
        (self.total > 0).then(|| self.counts[index] as f64 / self.total as f64)
    }

    /// Text rows for the AF detail panel, with each source explicitly labelled by its heading.
    pub fn detail(&self, record: &VcfRecord) -> Vec<String> {
        let percent =
            |v: Option<f64>| v.map_or_else(|| ".".into(), |v| format!("{:.3}%", v * 100.0));
        record
            .alternates
            .iter()
            .enumerate()
            .map(|(i, alt)| {
                format!(
                    "{alt}\t{}\t{}\t{}\t{}",
                    percent(self.info[i]),
                    self.counts[i],
                    self.total,
                    percent(self.gt_frequency(i))
                )
            })
            .collect()
    }
}

/// Distribution bins in ascending order, followed by unavailable.
pub(crate) fn bin(value: Option<f64>) -> usize {
    match value {
        Some(0.0) => 0,
        Some(v) if v < 0.01 => 1,
        Some(v) if v < 0.05 => 2,
        Some(_) => 3,
        None => 4,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::modules::vcf::{model::VcfHeader, reader::parse_record};

    /// Missing alleles never inflate AN; ploidy and ALT order come from each GT.
    #[test]
    fn counts_partial_haploid_polyploid_and_missing_calls() {
        let header = VcfHeader {
            samples: ["A", "B", "C", "D"].map(str::to_owned).into(),
            ..Default::default()
        };
        let entry = parse_record(
            b"chr1\t1\t.\tG\tA,C\t.\t.\tAF=.,0.01\tGT\t./1\t2\t0|2|1\t./.",
            &header,
        )
        .unwrap();
        let all = Frequencies::read(&entry.record, None);
        assert_eq!((all.counts.as_slice(), all.total), (&[2, 2][..], 5));
        assert_eq!(all.info, [None, Some(0.01)]);
        let subset = Frequencies::read(&entry.record, Some(&[0, 2]));
        assert_eq!((subset.counts.as_slice(), subset.total), (&[2, 1][..], 4));
        let absent = Frequencies::read(&entry.record, Some(&[3]));
        assert_eq!(absent.gt_frequency(0), None);
        assert_eq!(absent.info, all.info);
        assert_eq!(
            [Some(0.0), Some(0.009), Some(0.01), Some(0.05), None].map(bin),
            [0, 1, 2, 3, 4]
        );
    }
}
