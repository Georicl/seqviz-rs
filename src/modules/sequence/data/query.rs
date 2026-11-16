//! Parameterised record predicates, deterministic ordering and bounded result windows.
use super::{
    index::{RecordIndex, sql},
    model::RecordSummary,
};
use crate::modules::sequence::query::{RecordQuery, RecordSort, SortKey};
use rusqlite::{OptionalExtension, params_from_iter, types::Value};
use std::{
    io,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
};

/// Measurements become sortable/filterable only when their final denominator is known.
const LENGTH: &str = "CASE WHEN r.complete=1 AND r.has_seq=1 THEN r.length END";
/// Empty and non-DNA sequences have no GC percentage.
const GC: &str = "CASE WHEN r.complete=1 AND r.has_seq=1 AND r.seqtype=0 AND r.length>0 THEN 100.0*r.gc/r.length END";
/// Missing quality is distinct from a measured mean Phred score of zero.
const QUALITY: &str =
    "CASE WHEN r.complete=1 AND r.has_qual=1 AND r.qlen>0 THEN 1.0*r.qsum/r.qlen END";
/// Parsed alignment attributes retain their original record ID.
const MAPQ: &str = "(SELECT CAST(a.value AS INTEGER) FROM attrs a WHERE a.record=r.id AND a.name='MAPQ' ORDER BY a.ordinal LIMIT 1)";
/// FLAG predicates read the same ordered attributes as the record details panel.
const FLAGS: &str = "(SELECT CAST(a.value AS INTEGER) FROM attrs a WHERE a.record=r.id AND a.name='FLAG' ORDER BY a.ordinal LIMIT 1)";

/// One bounded window into a filtered and sorted list, retaining original source IDs.
#[derive(Clone, Debug, Default)]
pub struct RecordPage {
    /// Number of currently indexed records matching the query, including other pages.
    pub total: u64,
    /// Effective zero-based result offset after clamping to the last available position.
    pub offset: u64,
    /// At most 256 summaries in result order; each ID still addresses its source record.
    pub records: Vec<RecordSummary>,
}

/// SQL fragments contain only application-owned expressions; user values remain bound parameters.
struct SqlQuery {
    /// Predicates joined with AND, or the constant true for an unrestricted query.
    predicate: String,
    /// Values in the order their placeholders occur in the predicate.
    values: Vec<Value>,
    /// Complete stable ordering expression, always including the original record ID.
    order: String,
}

impl SqlQuery {
    /// Translate typed predicates without interpolating user-supplied names or numeric bounds.
    fn new(query: &RecordQuery, alignment: bool) -> io::Result<Self> {
        let filter = &query.filter;
        let mapq = if alignment { MAPQ } else { "NULL" };
        let flags = if alignment { FLAGS } else { "NULL" };
        let mut predicates = Vec::new();
        let mut values = Vec::new();
        if !filter.name.is_empty() {
            predicates.push("instr(lower(r.name),?)>0".into());
            values.push(Value::Text(filter.name.to_ascii_lowercase()));
        }
        for (expression, comparison, bound) in [
            (LENGTH, ">=", filter.min_length),
            (LENGTH, "<=", filter.max_length),
        ] {
            if let Some(bound) = bound {
                let bound = i64::try_from(bound).map_err(|_| {
                    io::Error::new(
                        io::ErrorKind::InvalidInput,
                        "Length exceeds SQLite integer range",
                    )
                })?;
                predicates.push(format!("({expression}){comparison}?"));
                values.push(Value::Integer(bound));
            }
        }
        for (expression, comparison, bound) in [
            (GC, ">=", filter.min_gc),
            (GC, "<=", filter.max_gc),
            (QUALITY, ">=", filter.min_quality),
            (QUALITY, "<=", filter.max_quality),
        ] {
            if let Some(bound) = bound {
                if !bound.is_finite() {
                    return Err(io::Error::new(
                        io::ErrorKind::InvalidInput,
                        "Measurement must be finite",
                    ));
                }
                predicates.push(format!("({expression}){comparison}?"));
                values.push(Value::Real(bound));
            }
        }
        if let Some(minimum) = filter.min_mapq {
            predicates.push(format!("({mapq})>=?"));
            values.push(Value::Integer(i64::from(minimum)));
        }
        if filter.required_flags != 0 {
            predicates.push(format!("(({flags}) & ?)=?"));
            values.push(Value::Integer(i64::from(filter.required_flags)));
            values.push(Value::Integer(i64::from(filter.required_flags)));
        }
        if filter.excluded_flags != 0 {
            predicates.push(format!("(({flags}) & ?)=0"));
            values.push(Value::Integer(i64::from(filter.excluded_flags)));
        }
        Ok(Self {
            predicate: if predicates.is_empty() {
                "1".into()
            } else {
                predicates.join(" AND ")
            },
            values,
            order: Self::order(query.sort, alignment),
        })
    }

    /// Use a total order so paging, source-ID lookup and name search share identical positions.
    fn order(sort: RecordSort, alignment: bool) -> String {
        let expression = match sort.key {
            SortKey::Source => "r.id",
            SortKey::Name => "lower(r.name)",
            SortKey::Length => LENGTH,
            SortKey::Gc => GC,
            SortKey::Quality => QUALITY,
            SortKey::Mapq if alignment => MAPQ,
            SortKey::Mapq => "NULL",
        };
        let direction = if sort.descending { "DESC" } else { "ASC" };
        format!("({expression}) IS NULL ASC, ({expression}) {direction}, r.id ASC")
    }
}

impl RecordIndex {
    /// Read a filtered/sorted page in SQLite, returning only the requested bounded window.
    /// Source IDs are never renumbered; `offset` is a position in the query result instead.
    pub fn query_records(
        &self,
        query: &RecordQuery,
        offset: u64,
        limit: usize,
    ) -> io::Result<RecordPage> {
        if query == &RecordQuery::default() {
            let total = self.progress.records;
            let offset = offset.min(total.saturating_sub(1));
            return Ok(RecordPage {
                total,
                offset,
                records: self.records(offset, limit.min(256))?,
            });
        }
        let mut query_sql = SqlQuery::new(query, self.format.is_alignment())?;
        let total: u64 = self
            .db
            .query_row(
                &format!(
                    "SELECT COUNT(*) FROM records r WHERE {}",
                    query_sql.predicate
                ),
                params_from_iter(query_sql.values.iter()),
                |row| row.get(0),
            )
            .map_err(sql)?;
        let offset = offset.min(total.saturating_sub(1));
        let statement = format!(
            "SELECT r.id,r.name,r.length,r.complete FROM records r WHERE {} ORDER BY {} LIMIT ? OFFSET ?",
            query_sql.predicate, query_sql.order
        );
        query_sql.values.push(Value::Integer(limit.min(256) as i64));
        query_sql.values.push(Value::Integer(offset as i64));
        let mut statement = self.db.prepare(&statement).map_err(sql)?;
        let records = statement
            .query_map(params_from_iter(query_sql.values.iter()), |row| {
                Ok(RecordSummary {
                    id: row.get(0)?,
                    name: row.get(1)?,
                    length: row.get(2)?,
                    complete: row.get(3)?,
                })
            })
            .map_err(sql)?
            .collect::<Result<Vec<_>, _>>()
            .map_err(sql)?;
        Ok(RecordPage {
            total,
            offset,
            records,
        })
    }

    /// Locate one stable source ID in the currently indexed query result, or return none if excluded.
    pub fn record_position(&self, query: &RecordQuery, id: u64) -> io::Result<Option<u64>> {
        if id > i64::MAX as u64 {
            return Ok(None);
        }
        if query == &RecordQuery::default() {
            return self
                .db
                .query_row("SELECT id FROM records WHERE id=?", [id], |row| row.get(0))
                .optional()
                .map_err(sql);
        }
        let mut query_sql = SqlQuery::new(query, self.format.is_alignment())?;
        let statement = format!(
            "WITH ranked AS (SELECT r.id,ROW_NUMBER() OVER (ORDER BY {})-1 AS position \
             FROM records r WHERE {}) SELECT position FROM ranked WHERE id=?",
            query_sql.order, query_sql.predicate,
        );
        query_sql.values.push(Value::Integer(id as i64));
        self.db
            .query_row(
                &statement,
                params_from_iter(query_sql.values.iter()),
                |row| row.get(0),
            )
            .optional()
            .map_err(sql)
    }

    /// Search names from an inclusive result position while retaining the query's filter and order.
    /// The returned value is a source ID; wrapping is controlled by the caller, and cancellation
    /// permanently interrupts this operation without leaving a SQLite progress handler installed.
    pub fn find_name_in_query(
        &self,
        query: &RecordQuery,
        name: &str,
        after: u64,
        cancel: Arc<AtomicBool>,
    ) -> io::Result<Option<u64>> {
        if cancel.load(Ordering::Relaxed) {
            return Err(io::Error::new(
                io::ErrorKind::Interrupted,
                "Search cancelled",
            ));
        }
        if after > i64::MAX as u64 {
            return Ok(None);
        }
        if query == &RecordQuery::default() {
            return self.find_name_cancellable(name, after, cancel);
        }
        let mut query_sql = SqlQuery::new(query, self.format.is_alignment())?;
        let statement = format!(
            "WITH ranked AS (SELECT r.id,r.name,ROW_NUMBER() OVER (ORDER BY {})-1 AS position \
             FROM records r WHERE {}) SELECT id FROM ranked \
             WHERE position>=? AND instr(lower(name),?)>0 ORDER BY position LIMIT 1",
            query_sql.order, query_sql.predicate,
        );
        query_sql.values.push(Value::Integer(after as i64));
        query_sql
            .values
            .push(Value::Text(name.to_ascii_lowercase()));
        let signal = Arc::clone(&cancel);
        self.db
            .progress_handler(1000, Some(move || signal.load(Ordering::Relaxed)));
        let result = self
            .db
            .query_row(
                &statement,
                params_from_iter(query_sql.values.iter()),
                |row| row.get(0),
            )
            .optional()
            .map_err(sql);
        self.db.progress_handler(0, None::<fn() -> bool>);
        if cancel.load(Ordering::Relaxed) {
            Err(io::Error::new(
                io::ErrorKind::Interrupted,
                "Search cancelled",
            ))
        } else {
            result
        }
    }
}

/// Query integration regressions use the real SQLite index and source record IDs.
#[cfg(test)]
mod tests {
    use super::*;
    use crate::modules::sequence::{
        data::Dataset,
        query::{RecordFilter, RecordSort},
    };
    use std::fs;

    /// Cancelling read-only SQL leaves the progress hook removed before subsequent scanner writes.
    #[test]
    fn cancelled_query_scope_does_not_poison_scan_transactions_or_later_queries() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("records.fa");
        let input = (0..3_000)
            .map(|id| format!(">record{id:04}\nACGT\n"))
            .collect::<String>();
        fs::write(&path, input).unwrap();
        let mut dataset = Dataset::open(&path, &directory.path().join("cache")).unwrap();
        dataset.step(8_000).unwrap();
        assert!(!dataset.progress().complete);
        let query = RecordQuery {
            filter: RecordFilter::parse("name=record").unwrap(),
            ..Default::default()
        };
        let cancel = Arc::new(AtomicBool::new(false));
        let error = dataset
            .cancellable(Arc::clone(&cancel), |data| {
                cancel.store(true, Ordering::Relaxed);
                data.query_records(&query, 0, 10)
            })
            .unwrap_err();
        assert_eq!(error.kind(), io::ErrorKind::Interrupted);
        // The scanner's write transaction must not inherit that read-query cancellation hook.
        while !dataset.progress().complete {
            dataset.step(64 * 1024).unwrap();
        }
        assert!(dataset.progress().error.is_none());
        assert_eq!(dataset.query_records(&query, 0, 10).unwrap().total, 3_000);
    }

    /// Fully scan a temporary input while retaining its cache and source for the test lifetime.
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

    /// Length ties retain source identity, and searches begin at result positions rather than IDs.
    #[test]
    fn sorted_pages_positions_and_searches_share_one_result_order() {
        let (_directory, dataset) = scan(b">zeta\nA\n>alpha\nACGT\n>beta\nGGGG\n>tiny\nAA\n", "fa");
        let query = RecordQuery {
            filter: RecordFilter::parse("minlen=2").unwrap(),
            sort: RecordSort {
                key: SortKey::Length,
                descending: true,
            },
        };
        let page = dataset.index.query_records(&query, 0, 2).unwrap();
        assert_eq!(page.total, 3);
        assert_eq!(
            page.records.iter().map(|r| r.id).collect::<Vec<_>>(),
            [1, 2]
        );
        assert_eq!(dataset.index.record_position(&query, 3).unwrap(), Some(2));
        assert_eq!(dataset.index.record_position(&query, 0).unwrap(), None);
        let cancel = Arc::new(AtomicBool::new(false));
        assert_eq!(
            dataset
                .index
                .find_name_in_query(&query, "A", 1, Arc::clone(&cancel))
                .unwrap(),
            Some(2)
        );
        assert_eq!(
            dataset
                .index
                .find_name_in_query(&query, "alpha", 1, cancel)
                .unwrap(),
            None
        );
        let last = dataset.index.query_records(&query, 99, 20).unwrap();
        assert_eq!(last.offset, 2);
        assert_eq!(last.records[0].id, 3);
    }

    /// GC/quality filters reject missing measurements, and missing values sort after measured values.
    #[test]
    fn measurement_filters_and_null_last_order_match_completed_record_semantics() {
        let (_directory, dataset) = scan(
            b"@low\nAAAA\n+\n!!!!\n@high\nGGGG\n+\nIIII\n@mid\nAC\n+\n55\n",
            "fq",
        );
        let query = RecordQuery {
            filter: RecordFilter::parse("mingc=40 maxgc=60 minq=20").unwrap(),
            ..Default::default()
        };
        let page = dataset.index.query_records(&query, 0, 20).unwrap();
        assert_eq!(page.total, 1);
        assert_eq!(page.records[0].id, 2);

        let (_directory, sam) = scan(b"@HD\tVN:1.6\n@SQ\tSN:chr1\tLN:100\nmissing\t4\t*\t0\t0\t*\t*\t0\t0\t*\t*\nshort\t0\tchr1\t1\t60\t1M\t*\t0\t0\tA\tI\nlong\t256\tchr1\t1\t60\t2M\t*\t0\t0\tAA\tII\n", "sam");
        for descending in [false, true] {
            let query = RecordQuery {
                sort: RecordSort {
                    key: SortKey::Length,
                    descending,
                },
                ..Default::default()
            };
            let page = sam.index.query_records(&query, 0, 20).unwrap();
            assert_eq!(page.records.last().unwrap().id, 0);
        }
        let query = RecordQuery {
            filter: RecordFilter::parse("mapq=30 noflag=4").unwrap(),
            ..Default::default()
        };
        assert_eq!(sam.index.query_records(&query, 0, 20).unwrap().total, 2);
        assert_eq!(dataset.index.query_records(&query, 0, 20).unwrap().total, 0);
        let query = RecordQuery {
            filter: RecordFilter::parse("flag=0x100").unwrap(),
            ..Default::default()
        };
        let page = sam.index.query_records(&query, 0, 20).unwrap();
        assert_eq!(page.total, 1);
        assert_eq!(page.records[0].id, 2);
    }

    /// Names are literal bound parameters, and an already cancelled search leaves later SQL usable.
    #[test]
    fn literal_names_and_cancelled_query_search_leave_the_index_usable() {
        let (_directory, dataset) = scan(b">a%_'\nA\n>other\nC\n", "fa");
        let query = RecordQuery {
            filter: RecordFilter::parse("name=%_'").unwrap(),
            ..Default::default()
        };
        assert_eq!(
            dataset.index.query_records(&query, 0, 999).unwrap().total,
            1
        );
        let error = dataset
            .index
            .find_name_in_query(&query, "a", 0, Arc::new(AtomicBool::new(true)))
            .unwrap_err();
        assert_eq!(error.kind(), io::ErrorKind::Interrupted);
        assert_eq!(
            dataset
                .index
                .query_records(&RecordQuery::default(), 0, 20)
                .unwrap()
                .total,
            2
        );
    }

    /// Unfiltered browsing retains partial records, but measurement bounds exclude them until complete.
    #[test]
    fn partial_records_remain_visible_without_measurement_predicates() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("partial.fa");
        fs::write(
            &path,
            format!(">first\nAC\n>second\n{}\n", "G".repeat(1_000)),
        )
        .unwrap();
        let mut dataset = Dataset::open(&path, &directory.path().join("cache")).unwrap();
        dataset.step(30).unwrap();
        assert_eq!(
            dataset
                .index
                .query_records(&RecordQuery::default(), 0, 20)
                .unwrap()
                .total,
            2
        );
        let query = RecordQuery {
            filter: RecordFilter::parse("minlen=0").unwrap(),
            ..Default::default()
        };
        assert_eq!(dataset.index.query_records(&query, 0, 20).unwrap().total, 1);
    }

    /// Large requested windows remain capped for default and sorted result paths alike.
    #[test]
    fn page_materialisation_is_bounded_and_empty_results_have_zero_offset() {
        let input = (0..300)
            .map(|id| format!(">record{id:03}\nA\n"))
            .collect::<String>();
        let (_directory, dataset) = scan(input.as_bytes(), "fa");
        for key in [SortKey::Source, SortKey::Name] {
            let query = RecordQuery {
                sort: RecordSort {
                    key,
                    descending: false,
                },
                ..Default::default()
            };
            let page = dataset.index.query_records(&query, 0, usize::MAX).unwrap();
            assert_eq!(page.total, 300);
            assert_eq!(page.records.len(), 256);
        }
        let query = RecordQuery {
            filter: RecordFilter::parse("name=absent").unwrap(),
            ..Default::default()
        };
        let page = dataset.index.query_records(&query, u64::MAX, 10).unwrap();
        assert_eq!(page.total, 0);
        assert_eq!(page.offset, 0);
        assert!(page.records.is_empty());
    }
}
