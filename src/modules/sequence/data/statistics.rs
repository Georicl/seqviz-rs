//! Bounded SQLite aggregates for sequence distributions and alignment summaries.
use super::{
    index::{RecordIndex, sql},
    model::{FileQualityStats, HistogramBin, MappingStats, Stats},
};
use std::io;

/// Half-open length intervals, with the final interval extending to infinity.
const LENGTH_LABELS: [&str; 6] = [
    "[0, 100)",
    "[100, 1,000)",
    "[1,000, 10,000)",
    "[10,000, 100,000)",
    "[100,000, 1,000,000)",
    "[1,000,000, inf)",
];
/// GC intervals include 100 percent in the final interval.
const GC_LABELS: [&str; 10] = [
    "[0, 10)",
    "[10, 20)",
    "[20, 30)",
    "[30, 40)",
    "[40, 50)",
    "[50, 60)",
    "[60, 70)",
    "[70, 80)",
    "[80, 90)",
    "[90, 100]",
];
/// Mean Phred intervals reflect records, rather than individual bases or scores.
const QUALITY_LABELS: [&str; 5] = ["[0, 10)", "[10, 20)", "[20, 30)", "[30, 40)", "[40, 93]"];

/// Add fixed-size summaries without materialising record rows in application memory.
/// The caller's SQLite progress handler also covers every query in this function.
pub(super) fn extend(index: &RecordIndex, stats: &mut Stats) -> io::Result<()> {
    stats.length_distribution = histogram(
        index,
        "SELECT CASE WHEN length<100 THEN 0 WHEN length<1000 THEN 1 \
         WHEN length<10000 THEN 2 WHEN length<100000 THEN 3 \
         WHEN length<1000000 THEN 4 ELSE 5 END AS bin, COUNT(*) \
         FROM records WHERE complete=1 AND has_seq=1 GROUP BY bin",
        &LENGTH_LABELS,
    )?;
    stats.gc_distribution = histogram(
        index,
        "SELECT MIN(9, CAST(10.0*gc/length AS INTEGER)) AS bin, COUNT(*) \
         FROM records WHERE complete=1 AND has_seq=1 AND seqtype=0 AND length>0 GROUP BY bin",
        &GC_LABELS,
    )?;
    stats.quality_distribution = histogram(
        index,
        "SELECT MIN(4, CAST(1.0*qsum/qlen/10 AS INTEGER)) AS bin, COUNT(*) \
         FROM records WHERE complete=1 AND has_qual=1 AND qlen>0 GROUP BY bin",
        &QUALITY_LABELS,
    )?;
    stats.quality = quality(index)?;
    stats.mapping = if index.format.is_alignment() {
        Some(mapping(index)?)
    } else {
        None
    };
    Ok(())
}

/// Query only grouped counts; the result size is bounded by the fixed label count.
fn histogram(
    index: &RecordIndex,
    query: &str,
    labels: &[&'static str],
) -> io::Result<Vec<HistogramBin>> {
    let mut result: Vec<_> = labels
        .iter()
        .map(|&label| HistogramBin { label, count: 0 })
        .collect();
    let mut statement = index.db.prepare(query).map_err(sql)?;
    let mut rows = statement.query([]).map_err(sql)?;
    while let Some(row) = rows.next().map_err(sql)? {
        let bin: usize = row.get(0).map_err(sql)?;
        let count = row.get(1).map_err(sql)?;
        let Some(target) = result.get_mut(bin) else {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "Indexed measurement is outside histogram intervals",
            ));
        };
        target.count = count;
    }
    Ok(result)
}

/// Weight quality means and Q30 by score count, including records of unequal lengths.
fn quality(index: &RecordIndex) -> io::Result<Option<FileQualityStats>> {
    let (records, scores, sum, q30): (u64, u64, f64, u64) = index
        .db
        .query_row(
            "SELECT COUNT(*),COALESCE(SUM(qlen),0),COALESCE(SUM(1.0*qsum),0),\
             COALESCE(SUM(q30),0) FROM records WHERE complete=1 AND has_qual=1 AND qlen>0",
            [],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?)),
        )
        .map_err(sql)?;
    Ok((scores > 0).then(|| FileQualityStats {
        records,
        scores,
        mean: sum / scores as f64,
        q30: 100.0 * q30 as f64 / scores as f64,
    }))
}

/// Count independent FLAG categories over completed alignments; categories may overlap.
fn mapping(index: &RecordIndex) -> io::Result<MappingStats> {
    index
        .db
        .query_row(
            "SELECT COUNT(*),COALESCE(SUM((flag & 4)=0),0),\
             COALESCE(SUM((flag & 4)!=0),0),COALESCE(SUM((flag & 2304)=0),0),\
             COALESCE(SUM((flag & 256)!=0),0),COALESCE(SUM((flag & 2048)!=0),0),\
             COALESCE(SUM((flag & 1024)!=0),0) FROM \
             (SELECT CAST(a.value AS INTEGER) AS flag FROM records r \
             JOIN attrs a ON a.record=r.id AND a.name='FLAG' WHERE r.complete=1)",
            [],
            |row| {
                Ok(MappingStats {
                    alignments: row.get(0)?,
                    mapped: row.get(1)?,
                    unmapped: row.get(2)?,
                    primary: row.get(3)?,
                    secondary: row.get(4)?,
                    supplementary: row.get(5)?,
                    duplicate: row.get(6)?,
                })
            },
        )
        .map_err(sql)
}

/// Regression fixtures for bin boundaries, missing values and independent FLAG categories.
#[cfg(test)]
mod tests {
    use crate::modules::sequence::data::{Dataset, Format};
    use std::fs;

    /// Open and fully scan a temporary input without changing its source bytes.
    fn scan(input: &[u8], extension: &str) -> (tempfile::TempDir, Dataset) {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join(format!("records.{extension}"));
        fs::write(&path, input).unwrap();
        let mut dataset = Dataset::open(&path, &directory.path().join("cache")).unwrap();
        while !dataset.progress().complete {
            let progress = dataset.step(64 * 1024).unwrap();
            assert!(progress.error.is_none(), "{:?}", progress.error);
        }
        (directory, dataset)
    }

    /// Empty records count toward lengths, while GC excludes empty and protein records.
    #[test]
    fn distributions_keep_boundaries_and_only_include_completed_applicable_records() {
        let input = format!(
            ">empty\n>low\n{}\n>half\n{}\n>high\n{}\n",
            "A".repeat(100),
            "AC".repeat(500),
            "G".repeat(10_000)
        );
        let (_directory, dataset) = scan(input.as_bytes(), "fa");
        let stats = dataset.stats().unwrap();
        assert_eq!(
            stats
                .length_distribution
                .iter()
                .map(|b| b.count)
                .collect::<Vec<_>>(),
            [1, 1, 1, 1, 0, 0]
        );
        assert_eq!(stats.gc_distribution[0].count, 1);
        assert_eq!(stats.gc_distribution[5].count, 1);
        assert_eq!(stats.gc_distribution[9].count, 1);
        assert_eq!(
            stats.gc_distribution.iter().map(|b| b.count).sum::<u64>(),
            3
        );
        assert!(stats.quality.is_none());
        assert!(stats.mapping.is_none());

        let (_directory, protein) = scan(b">protein\nACDEFGHIK\n", "faa");
        assert_eq!(
            protein
                .stats()
                .unwrap()
                .gc_distribution
                .iter()
                .map(|b| b.count)
                .sum::<u64>(),
            0
        );
    }

    /// File means are weighted by bases; histogram counts remain weighted by records.
    #[test]
    fn quality_distribution_and_weighted_file_aggregates_use_different_denominators() {
        let (_directory, dataset) = scan(b"@short\nA\n+\n!\n@long\nAAA\n+\nIII\n", "fq");
        let stats = dataset.stats().unwrap();
        let quality = stats.quality.unwrap();
        assert_eq!(quality.records, 2);
        assert_eq!(quality.scores, 4);
        assert_eq!(quality.mean, 30.0);
        assert_eq!(quality.q30, 75.0);
        assert_eq!(
            stats
                .quality_distribution
                .iter()
                .map(|b| b.count)
                .collect::<Vec<_>>(),
            [1, 0, 0, 0, 1]
        );
    }

    /// Missing SEQ remains in alignment counts, and duplicate flags do not remove records.
    #[test]
    fn mapping_categories_include_missing_sequence_and_preserve_overlap() {
        let input = b"@HD\tVN:1.6\n@SQ\tSN:chr1\tLN:100\nprimary\t0\tchr1\t1\t60\t1M\t*\t0\t0\tA\tI\nmissing\t4\t*\t0\t0\t*\t*\t0\t0\t*\t*\nsecondary\t1280\tchr1\t2\t10\t1M\t*\t0\t0\tC\tI\nsupplementary\t2048\tchr1\t3\t20\t1M\t*\t0\t0\tG\tI\n";
        let (_directory, dataset) = scan(input, "sam");
        assert_eq!(dataset.format(), Format::Sam);
        let stats = dataset.stats().unwrap();
        let mapping = stats.mapping.unwrap();
        assert_eq!(mapping.alignments, 4);
        assert_eq!(mapping.mapped, 3);
        assert_eq!(mapping.unmapped, 1);
        assert_eq!(mapping.primary, 2);
        assert_eq!(mapping.secondary, 1);
        assert_eq!(mapping.supplementary, 1);
        assert_eq!(mapping.duplicate, 1);
        assert_eq!(stats.sequence_records, 3);
    }

    /// Partial records cannot leak unstable lengths or qualities into completed-record bins.
    #[test]
    fn partial_records_are_excluded_until_their_end_is_indexed() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("partial.fa");
        fs::write(
            &path,
            format!(">first\nAC\n>second\n{}\n", "G".repeat(1_000)),
        )
        .unwrap();
        let mut dataset = Dataset::open(&path, &directory.path().join("cache")).unwrap();
        dataset.step(30).unwrap();
        let partial = dataset.stats().unwrap();
        assert!(!partial.complete);
        assert_eq!(
            partial
                .length_distribution
                .iter()
                .map(|b| b.count)
                .sum::<u64>(),
            1
        );
        assert_eq!(partial.gc_distribution[5].count, 1);
    }
}
