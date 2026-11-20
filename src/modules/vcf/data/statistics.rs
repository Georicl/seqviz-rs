//! Aggregate SQL and scan-time genotype counters; raw sample values remain in the source.
use super::super::{model::VcfRecord, query::Query, reader::invalid, statistics::Statistics};
use super::{Dataset, decode_kind, query::predicate, sql};
use rusqlite::params_from_iter;
use std::{
    io,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
};

impl Dataset {
    /// Aggregate one filtered indexed prefix, with cancellation checked by SQLite's progress hook.
    /// Genotype summaries are indexed per record, so this never materializes all sample values.
    pub(crate) fn statistics(
        &mut self,
        contig: Option<&str>,
        query: &Query,
        samples: Option<&[usize]>,
        cancel: Arc<AtomicBool>,
    ) -> io::Result<Statistics> {
        self.check_source()?;
        query.validate().map_err(invalid)?;
        if cancel.load(Ordering::Relaxed) || self.query_cancel.load(Ordering::Relaxed) {
            return Err(io::Error::new(
                io::ErrorKind::Interrupted,
                "Statistics cancelled",
            ));
        }
        let flag = Arc::clone(&cancel);
        let worker = Arc::clone(&self.query_cancel);
        self.db.progress_handler(
            1000,
            Some(move || flag.load(Ordering::Relaxed) || worker.load(Ordering::Relaxed)),
        );
        let result = self
            .aggregate_statistics(contig, query)
            .and_then(|mut statistics| {
                self.frequency_distribution(contig, query, samples, &cancel, &mut statistics)?;
                Ok(statistics)
            });
        self.db.progress_handler(0, None::<fn() -> bool>);
        if cancel.load(Ordering::Relaxed) || self.query_cancel.load(Ordering::Relaxed) {
            return Err(io::Error::new(
                io::ErrorKind::Interrupted,
                "Statistics cancelled",
            ));
        }
        self.check_source()?;
        result
    }

    /// Use indexed ALT bins for whole-cohort summaries. For an explicit subset, reread
    /// matching records in source order in bounded batches, checking cancellation per row.
    fn frequency_distribution(
        &mut self,
        contig: Option<&str>,
        query: &Query,
        samples: Option<&[usize]>,
        cancel: &AtomicBool,
        stats: &mut Statistics,
    ) -> io::Result<()> {
        let (predicate, values) = predicate(query, contig);
        let sql_text = format!(
            "SELECT gt_bin,info_bin,COUNT(*) FROM records JOIN allele_bins ON id=record_id
                               WHERE {predicate} GROUP BY gt_bin,info_bin"
        );
        let mut statement = self.db.prepare(&sql_text).map_err(sql)?;
        let bins = statement
            .query_map(params_from_iter(&values), |row| {
                Ok((
                    row.get::<_, usize>(0)?,
                    row.get::<_, usize>(1)?,
                    row.get::<_, u64>(2)?,
                ))
            })
            .map_err(sql)?;
        for group in bins {
            let (gt, info, count) = group.map_err(sql)?;
            stats.gt_af[gt] += count;
            stats.info_af[info] += count;
        }
        drop(statement);
        stats.af_scope = format!("all {} samples", self.header.samples.len());
        let Some(indices) = samples else {
            return Ok(());
        };
        stats.af_scope = format!("{} pinned samples", indices.len());
        stats.gt_af = [0; 5];
        if indices.is_empty() {
            stats.gt_af[4] = stats.info_af.iter().sum();
            return Ok(());
        }
        let batch_sql = format!(
            "SELECT id,offset,bytes FROM records WHERE {predicate} AND id>? ORDER BY id LIMIT 128"
        );
        let mut last_id = -1i64;
        loop {
            let mut parameters = values.clone();
            parameters.push(last_id.into());
            let batch = self
                .db
                .prepare_cached(&batch_sql)
                .map_err(sql)?
                .query_map(params_from_iter(parameters), |row| {
                    Ok((row.get(0)?, row.get(1)?, row.get(2)?))
                })
                .map_err(sql)?
                .collect::<Result<Vec<_>, _>>()
                .map_err(sql)?;
            if batch.is_empty() {
                break;
            }
            for range in batch {
                if cancel.load(Ordering::Relaxed) || self.query_cancel.load(Ordering::Relaxed) {
                    return Err(io::Error::new(
                        io::ErrorKind::Interrupted,
                        "Statistics cancelled",
                    ));
                }
                last_id = range.0 as i64;
                if let Some(entry) = self.entry_range(Some(range))? {
                    let frequencies =
                        super::super::frequency::Frequencies::read(&entry.record, Some(indices));
                    for i in 0..frequencies.counts.len() {
                        stats.gt_af[super::super::frequency::bin(frequencies.gt_frequency(i))] += 1;
                    }
                }
            }
        }
        Ok(())
    }

    /// Execute bounded-size SQL aggregates; the progress hook is owned by `statistics`.
    fn aggregate_statistics(&self, contig: Option<&str>, query: &Query) -> io::Result<Statistics> {
        let (predicate, values) = predicate(query, contig);
        let statement = format!(
            "SELECT COUNT(*),
                    COALESCE(SUM(filter='PASS'),0),
                    COALESCE(SUM(filter='.'),0),
                    COALESCE(SUM(filter NOT IN ('PASS','.')),0),
                    MIN(qual),MAX(qual),AVG(qual),
                    COALESCE(SUM(qual IS NULL),0),
                    COALESCE(SUM(qual<20),0),
                    COALESCE(SUM(qual>=20 AND qual<30),0),
                    COALESCE(SUM(qual>=30 AND qual<60),0),
                    COALESCE(SUM(qual>=60),0),
                    COALESCE(SUM(gt_called),0),
                    COALESCE(SUM(gt_missing),0),
                    COALESCE(SUM(gt_partial),0),
                    COALESCE(SUM(gt_unavailable),0)
             FROM records WHERE {predicate}"
        );
        let mut statistics = self
            .db
            .query_row(&statement, params_from_iter(values.clone()), |row| {
                Ok(Statistics {
                    gt_af: [0; 5],
                    info_af: [0; 5],
                    af_scope: String::new(),
                    records: row.get(0)?,
                    pass: row.get(1)?,
                    filter_missing: row.get(2)?,
                    filtered: row.get(3)?,
                    qual_min: row.get(4)?,
                    qual_max: row.get(5)?,
                    qual_mean: row.get(6)?,
                    qual_bins: [
                        row.get(7)?,
                        row.get(8)?,
                        row.get(9)?,
                        row.get(10)?,
                        row.get(11)?,
                    ],
                    called_genotypes: row.get(12)?,
                    missing_genotypes: row.get(13)?,
                    partial_genotypes: row.get(14)?,
                    unavailable_genotypes: row.get(15)?,
                    complete: self.progress.complete && self.progress.error.is_none(),
                    scope: format!("{}; {}", contig.unwrap_or("All contigs"), query.summary()),
                    kinds: Vec::new(),
                })
            })
            .map_err(sql)?;
        let mut counts = [0; 9];
        let mut statement = self
            .db
            .prepare(&format!(
                "SELECT kind,COUNT(*) FROM records WHERE {predicate} GROUP BY kind"
            ))
            .map_err(sql)?;
        for row in statement
            .query_map(params_from_iter(values), |row| {
                Ok((row.get::<_, usize>(0)?, row.get::<_, u64>(1)?))
            })
            .map_err(sql)?
        {
            let (kind, count) = row.map_err(sql)?;
            if let Some(value) = counts.get_mut(kind) {
                *value = count;
            }
        }
        statistics.kinds = counts
            .into_iter()
            .enumerate()
            .map(|(code, count)| (decode_kind(code as u8), count))
            .collect();
        Ok(statistics)
    }
}

/// Scan-time counters persisted per record, so reports need not decode every sample again.
#[derive(Default)]
pub(super) struct GenotypeCounts {
    pub(super) called: u64,
    pub(super) missing: u64,
    pub(super) partial: u64,
    pub(super) unavailable: u64,
}

/// Count GT states in one raw record without allocating sample objects.
pub(super) fn genotype_counts(record: &VcfRecord) -> GenotypeCounts {
    let gt = record.format.iter().position(|field| field == "GT");
    let mut counts = GenotypeCounts::default();
    for sample in record.raw.split('\t').skip(9) {
        let Some(index) = gt else {
            counts.unavailable += 1;
            continue;
        };
        let value = sample.split(':').nth(index).unwrap_or(".");
        let mut present = false;
        let mut missing = false;
        for allele in value.split(['/', '|']) {
            if allele.is_empty() || allele == "." {
                missing = true;
            } else {
                present = true;
            }
        }
        match (present, missing) {
            (true, false) => counts.called += 1,
            (false, _) => counts.missing += 1,
            (true, true) => counts.partial += 1,
        }
    }
    counts
}
