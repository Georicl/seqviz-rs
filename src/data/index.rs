//! Persistent record metadata, source spans and aggregate statistics.
//! Parsers write through RecordSink; SQL, record IDs and transactions stay here.
use super::{
    CHUNK,
    classify::detect_type,
    invalid,
    model::*,
    scan::{RecordSink, RecordState, SpanEncoding, SpanKind},
};
use rusqlite::{Connection, OptionalExtension, params};
use std::{
    io,
    path::Path,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
};

fn sql(error: rusqlite::Error) -> io::Error {
    io::Error::other(error)
}

pub(super) struct RecordIndex {
    pub(super) db: Connection,
    pub(super) format: Format,
    force_protein: bool,
    pub(super) progress: Progress,
    current: Option<Current>,
    span: Option<PendingSpan>,
}
struct Current {
    id: u64,
    length: u64,
    qlen: u64,
    gc: u64,
    sample: Vec<u8>,
    has_sequence: bool,
    has_quality: bool,
    qmin: u8,
    qmax: u8,
    qsum: u64,
    q30: u64,
    qbins: [u64; 64],
    qcounts: [u64; 64],
}

struct PendingSpan {
    record: u64,
    kind: SpanKind,
    start: u64,
    len: u64,
    offset: u64,
    raw_len: u64,
    encoding: SpanEncoding,
}

pub(super) struct StoredSpan {
    pub start: u64,
    pub len: u64,
    pub offset: u64,
    pub raw_len: u64,
    pub encoding: SpanEncoding,
}

impl RecordIndex {
    pub fn open(
        directory: &Path,
        key: u64,
        format: Format,
        force_protein: bool,
    ) -> io::Result<(Self, u64, bool)> {
        let db =
            Connection::open(directory.join(format!("{key:016x}.records.sqlite"))).map_err(sql)?;
        db.execute_batch("PRAGMA journal_mode=WAL; PRAGMA synchronous=NORMAL; PRAGMA cache_size=-2048; PRAGMA temp_store=FILE;
            CREATE TABLE IF NOT EXISTS meta(key TEXT PRIMARY KEY, value TEXT NOT NULL);
            CREATE TABLE IF NOT EXISTS records(
                id INTEGER PRIMARY KEY, name TEXT NOT NULL, header TEXT NOT NULL, plus TEXT NOT NULL DEFAULT '+',
                length INTEGER NOT NULL DEFAULT 0, qlen INTEGER NOT NULL DEFAULT 0, complete INTEGER NOT NULL DEFAULT 0,
                seqtype INTEGER NOT NULL DEFAULT 0, gc INTEGER NOT NULL DEFAULT 0,
                has_seq INTEGER NOT NULL DEFAULT 1, has_qual INTEGER NOT NULL DEFAULT 0,
                qmin INTEGER NOT NULL DEFAULT 0, qmax INTEGER NOT NULL DEFAULT 0, qsum INTEGER NOT NULL DEFAULT 0,
                q30 INTEGER NOT NULL DEFAULT 0, overview BLOB NOT NULL DEFAULT X'');
            CREATE TABLE IF NOT EXISTS spans(record INTEGER NOT NULL, kind INTEGER NOT NULL, start INTEGER NOT NULL,
                len INTEGER NOT NULL, offset INTEGER NOT NULL, raw_len INTEGER NOT NULL, encoding INTEGER NOT NULL,
                PRIMARY KEY(record,kind,start));
            CREATE TABLE IF NOT EXISTS attrs(record INTEGER NOT NULL, ordinal INTEGER NOT NULL, name TEXT NOT NULL, value TEXT NOT NULL,
                PRIMARY KEY(record,ordinal));
            CREATE TABLE IF NOT EXISTS refs(id INTEGER PRIMARY KEY, name TEXT NOT NULL, length INTEGER NOT NULL);
            CREATE TABLE IF NOT EXISTS errors(offset INTEGER NOT NULL, message TEXT NOT NULL);
            CREATE INDEX IF NOT EXISTS lengths ON records(complete,has_seq,length DESC);").map_err(sql)?;
        let get = |key: &str| -> io::Result<Option<String>> {
            db.query_row("SELECT value FROM meta WHERE key=?", [key], |r| r.get(0))
                .optional()
                .map_err(sql)
        };
        let complete = get("complete")?.as_deref() == Some("1");
        let fatal = get("fatal")?;
        let resume = get("resume")?.and_then(|v| v.parse().ok()).unwrap_or(0);
        let position = if complete || fatal.is_some() {
            get("scanned")?
                .and_then(|v| v.parse().ok())
                .unwrap_or(resume)
        } else {
            resume
        };
        // A partial record is regenerated from its defline; complete records remain reusable.
        if !complete && fatal.is_none() {
            db.execute_batch(
                "DELETE FROM spans WHERE record IN (SELECT id FROM records WHERE complete=0);
                DELETE FROM attrs WHERE record IN (SELECT id FROM records WHERE complete=0);
                DELETE FROM records WHERE complete=0;",
            )
            .map_err(sql)?;
        }
        let records = db
            .query_row("SELECT COUNT(*) FROM records", [], |r| r.get(0))
            .map_err(sql)?;
        let errors = db
            .query_row("SELECT COUNT(*) FROM errors", [], |r| r.get(0))
            .map_err(sql)?;
        let last_error = db
            .query_row(
                "SELECT offset,message FROM errors ORDER BY rowid DESC LIMIT 1",
                [],
                |row| {
                    Ok(format!(
                        "At byte {}: {}",
                        row.get::<_, u64>(0)?,
                        row.get::<_, String>(1)?
                    ))
                },
            )
            .optional()
            .map_err(sql)?;
        let header_done = get("bam_header")?.is_some();
        Ok((
            Self {
                db,
                format,
                force_protein,
                progress: Progress {
                    records,
                    complete,
                    scanned_bytes: position,
                    errors,
                    last_error,
                    error: fatal,
                },
                current: None,
                span: None,
            },
            position,
            header_done,
        ))
    }

    pub fn begin_step(&self) -> io::Result<()> {
        self.db.execute_batch("BEGIN IMMEDIATE").map_err(sql)
    }

    pub fn rollback_step(&self) {
        let _ = self.db.execute_batch("ROLLBACK");
    }

    /// Commits a bounded parser step, including partial metadata and resume state.
    /// Invalid input stops scanning but retains all successfully indexed spans.
    pub fn finish_step(&mut self, outcome: io::Result<bool>, position: u64) -> io::Result<bool> {
        let result = (|| {
            let stopped = match outcome {
                Ok(true) => {
                    self.progress.complete = true;
                    self.set_meta("complete", "1")?;
                    true
                }
                Ok(false) => false,
                Err(error) => {
                    let record = self
                        .current
                        .as_ref()
                        .map_or(self.progress.records + 1, |c| c.id + 1);
                    self.log_error(position, &format!("Record {record}: {error}"))?;
                    self.progress.error = Some(format!("At byte {position}: {error}"));
                    self.set_meta(
                        "fatal",
                        self.progress.error.as_deref().unwrap_or("Read error"),
                    )?;
                    true
                }
            };
            self.flush_span()?;
            self.save_current(false)?;
            self.progress.scanned_bytes = position;
            self.set_meta("scanned", &position.to_string())?;
            self.db.execute_batch("COMMIT").map_err(sql)?;
            Ok(stopped)
        })();
        if result.is_err() {
            self.rollback_step();
        }
        result
    }

    /// Visits intersecting spans one at a time, keeping range reads bounded even
    /// for highly fragmented indexes. The caller handles source I/O and decoding.
    pub fn visit_spans(
        &self,
        id: u64,
        start: u64,
        len: usize,
        kind: SpanKind,
        mut visit: impl FnMut(StoredSpan) -> io::Result<()>,
    ) -> io::Result<()> {
        let end = start.saturating_add(len as u64);
        let first: Option<u64> = self.db.query_row("SELECT start FROM spans WHERE record=? AND kind=? AND start<=? ORDER BY start DESC LIMIT 1",
            params![id,kind_code(kind),start], |r| r.get(0)).optional().map_err(sql)?;
        let mut query = self.db.prepare("SELECT start,len,offset,raw_len,encoding FROM spans WHERE record=? AND kind=? AND start>=? AND start<? AND start+len>? ORDER BY start").map_err(sql)?;
        let mut rows = query
            .query(params![
                id,
                kind_code(kind),
                first.unwrap_or(start),
                end,
                start
            ])
            .map_err(sql)?;
        while let Some(row) = rows.next().map_err(sql)? {
            visit(StoredSpan {
                start: row.get(0).map_err(sql)?,
                len: row.get(1).map_err(sql)?,
                offset: row.get(2).map_err(sql)?,
                raw_len: row.get(3).map_err(sql)?,
                encoding: encoding_from_code(row.get(4).map_err(sql)?)?,
            })?;
        }
        Ok(())
    }
    pub fn records(&self, offset: u64, limit: usize) -> io::Result<Vec<RecordSummary>> {
        let mut query = self
            .db
            .prepare("SELECT id,name,length,complete FROM records WHERE id>=? ORDER BY id LIMIT ?")
            .map_err(sql)?;
        query
            .query_map(params![offset, limit as u64], |row| {
                Ok(RecordSummary {
                    id: row.get(0)?,
                    name: row.get(1)?,
                    length: row.get(2)?,
                    complete: row.get(3)?,
                })
            })
            .map_err(sql)?
            .collect::<Result<_, _>>()
            .map_err(sql)
    }

    pub fn record(&self, id: u64) -> io::Result<Option<RecordMeta>> {
        let mut value = self.db.query_row("SELECT id,name,header,plus,length,qlen,complete,seqtype,gc,has_seq,has_qual,qmin,qmax,qsum,q30,overview FROM records WHERE id=?", [id], |r| {
            let length: u64 = r.get(4)?;
            let qlen: u64 = r.get(5)?;
            let complete: bool = r.get(6)?;
            let seqtype = match r.get::<_, i64>(7)? { 1 => SeqType::Protein, 2 => SeqType::Unknown, _ => SeqType::Dna };
            let has_quality = r.get::<_, bool>(10)?;
            let quality = if complete && has_quality && qlen > 0 {
                let overview: Vec<u8> = r.get(15)?;
                Some(QualityStats { min: r.get(11)?, max: r.get(12)?, mean: r.get::<_, u64>(13)? as f64 / qlen as f64,
                    q30: 100.0 * r.get::<_, u64>(14)? as f64 / qlen as f64,
                    overview: overview.into_iter().map(f64::from).collect() })
            } else { None };
            Ok(RecordMeta { id: r.get(0)?, name: r.get(1)?, header: r.get(2)?, plus: r.get(3)?, length,
                quality_length: qlen, complete, seq_type: seqtype,
                gc: if complete && seqtype == SeqType::Dna && length > 0 { Some(100.0 * r.get::<_, u64>(8)? as f64 / length as f64) } else { None },
                quality, fields: Vec::new(), has_sequence: r.get(9)?, has_quality })
        }).optional().map_err(sql)?;
        if let Some(meta) = &mut value {
            let mut query = self
                .db
                .prepare("SELECT name,value FROM attrs WHERE record=? ORDER BY ordinal")
                .map_err(sql)?;
            meta.fields = query
                .query_map([id], |r| Ok((r.get(0)?, r.get(1)?)))
                .map_err(sql)?
                .collect::<Result<_, _>>()
                .map_err(sql)?;
        }
        Ok(value)
    }

    /// Search indexed names from `after` inclusively; the caller controls wraparound.
    pub fn find_name(&self, query: &str, after: u64) -> io::Result<Option<u64>> {
        let query = query.to_ascii_lowercase();
        self.db
            .query_row(
                "SELECT id FROM records WHERE id>=? AND instr(lower(name),?)>0 ORDER BY id LIMIT 1",
                params![after, query],
                |r| r.get(0),
            )
            .optional()
            .map_err(sql)
    }

    pub fn find_name_cancellable(
        &self,
        query: &str,
        after: u64,
        cancel: Arc<AtomicBool>,
    ) -> io::Result<Option<u64>> {
        let signal = cancel.clone();
        self.db
            .progress_handler(1000, Some(move || signal.load(Ordering::Relaxed)));
        let result = self.find_name(query, after);
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

    pub fn stats_cancellable(&self, cancel: Arc<AtomicBool>) -> io::Result<Stats> {
        let signal = cancel.clone();
        self.db
            .progress_handler(1000, Some(move || signal.load(Ordering::Relaxed)));
        let result = self.stats();
        self.db.progress_handler(0, None::<fn() -> bool>);
        if cancel.load(Ordering::Relaxed) {
            Err(io::Error::new(
                io::ErrorKind::Interrupted,
                "Statistics cancelled",
            ))
        } else {
            result
        }
    }

    pub fn stats(&self) -> io::Result<Stats> {
        let (count, total, min, max, gc, protein, dna): (u64,u64,Option<u64>,Option<u64>,u64,u64,u64) = self.db.query_row(
            "SELECT COUNT(*),COALESCE(SUM(length),0),MIN(length),MAX(length),COALESCE(SUM(gc),0),COALESCE(SUM(seqtype=1),0),COALESCE(SUM(seqtype=0),0) FROM records WHERE complete=1 AND has_seq=1", [],
            |r| Ok((r.get(0)?,r.get(1)?,r.get(2)?,r.get(3)?,r.get(4)?,r.get(5)?,r.get(6)?))).map_err(sql)?;
        let mut n50 = None;
        if self.format.supports_n50() && count > 0 {
            let mut query = self.db.prepare("SELECT length FROM records WHERE complete=1 AND has_seq=1 ORDER BY length DESC").map_err(sql)?;
            let mut rows = query.query([]).map_err(sql)?;
            let mut sum = 0u64;
            while let Some(row) = rows.next().map_err(sql)? {
                let length: u64 = row.get(0).map_err(sql)?;
                sum += length;
                if sum >= total.div_ceil(2) {
                    n50 = Some(length);
                    break;
                }
            }
        }
        Ok(Stats {
            records: self.progress.records,
            sequence_records: count,
            total_length: total,
            min_length: min,
            max_length: max,
            mean_length: (count > 0).then(|| total as f64 / count as f64),
            n50,
            gc: (dna == count && total > 0).then(|| 100.0 * gc as f64 / total as f64),
            protein: protein > 0 && protein == count,
            complete: self.progress.complete && self.progress.errors == 0,
            errors: self.progress.errors,
        })
    }

    fn set_meta(&self, key: &str, value: &str) -> io::Result<()> {
        self.db
            .execute(
                "INSERT INTO meta VALUES(?,?) ON CONFLICT(key) DO UPDATE SET value=excluded.value",
                params![key, value],
            )
            .map_err(sql)?;
        Ok(())
    }

    fn log_error(&mut self, offset: u64, message: &str) -> io::Result<()> {
        self.db
            .execute("INSERT INTO errors VALUES(?,?)", params![offset, message])
            .map_err(sql)?;
        self.progress.errors += 1;
        self.progress.last_error = Some(format!("At byte {offset}: {message}"));
        Ok(())
    }

    fn begin_record(&mut self, header: String, has_sequence: bool) -> io::Result<()> {
        let name = header.split_whitespace().next().unwrap_or("").to_owned();
        if name.is_empty() {
            return Err(invalid("Empty record identifier"));
        }
        let id = self.progress.records;
        self.db
            .execute(
                "INSERT INTO records(id,name,header,has_seq) VALUES(?,?,?,?)",
                params![id, name, header, has_sequence],
            )
            .map_err(sql)?;
        if self.format == Format::Fastq {
            self.store_fields(id, &crate::headers::parse_fastq_header(&header))?;
        }
        self.current = Some(Current {
            id,
            length: 0,
            qlen: 0,
            gc: 0,
            sample: Vec::with_capacity(1000),
            has_sequence,
            has_quality: false,
            qmin: 93,
            qmax: 0,
            qsum: 0,
            q30: 0,
            qbins: [0; 64],
            qcounts: [0; 64],
        });
        self.progress.records += 1;
        Ok(())
    }

    fn store_fields(&self, id: u64, fields: &[(String, String)]) -> io::Result<()> {
        self.db
            .execute("DELETE FROM attrs WHERE record=?", [id])
            .map_err(sql)?;
        let mut query = self
            .db
            .prepare_cached("INSERT INTO attrs VALUES(?,?,?,?)")
            .map_err(sql)?;
        for (i, (name, value)) in fields.iter().enumerate() {
            query
                .execute(params![id, i as u64, name, value])
                .map_err(sql)?;
        }
        Ok(())
    }

    fn save_current(&self, complete: bool) -> io::Result<()> {
        if let Some(c) = &self.current {
            let seqtype = if self.force_protein {
                SeqType::Protein
            } else if self.format.is_alignment() {
                SeqType::Dna
            } else {
                detect_type(&c.sample)
            };
            let seqtype = match seqtype {
                SeqType::Dna => 0,
                SeqType::Protein => 1,
                SeqType::Unknown => 2,
            };
            let overview: Vec<u8> = c
                .qbins
                .iter()
                .zip(c.qcounts)
                .filter(|(_, count)| *count > 0)
                .map(|(sum, count)| (sum / count) as u8)
                .collect();
            self.db.execute("UPDATE records SET length=?,qlen=?,complete=?,seqtype=?,gc=?,has_seq=?,has_qual=?,qmin=?,qmax=?,qsum=?,q30=?,overview=? WHERE id=?",
                params![c.length,c.qlen,complete,seqtype,c.gc,c.has_sequence,c.has_quality,c.qmin,c.qmax,c.qsum,c.q30,overview,c.id]).map_err(sql)?;
        }
        Ok(())
    }

    fn end_record(&mut self, next_offset: u64) -> io::Result<()> {
        self.flush_span()?;
        self.save_current(true)?;
        self.current = None;
        self.set_meta("resume", &next_offset.to_string())
    }

    fn discard_current(&mut self) -> io::Result<()> {
        self.span = None;
        if let Some(c) = self.current.take() {
            self.db
                .execute("DELETE FROM spans WHERE record=?", [c.id])
                .map_err(sql)?;
            self.db
                .execute("DELETE FROM attrs WHERE record=?", [c.id])
                .map_err(sql)?;
            self.db
                .execute("DELETE FROM records WHERE id=?", [c.id])
                .map_err(sql)?;
            self.progress.records -= 1;
        }
        Ok(())
    }

    fn append(
        &mut self,
        raw: &[u8],
        offset: u64,
        kind: SpanKind,
        encoding: SpanEncoding,
        decoded: &[u8],
    ) -> io::Result<()> {
        if decoded.is_empty() {
            return Ok(());
        }
        let c = self
            .current
            .as_ref()
            .ok_or_else(|| invalid("Sequence before record header"))?;
        let start = if kind == SpanKind::Sequence {
            c.length
        } else {
            c.qlen
        };
        let id = c.id;
        if self.span.as_ref().is_some_and(|s| {
            s.record != id
                || s.kind != kind
                || s.encoding != encoding
                || offset + raw.len() as u64 - s.offset > CHUNK as u64
                || encoding != SpanEncoding::Text
        }) {
            self.flush_span()?;
        }
        if let Some(span) = &mut self.span {
            span.raw_len = offset + raw.len() as u64 - span.offset;
            span.len += decoded.len() as u64;
        } else {
            self.span = Some(PendingSpan {
                record: id,
                kind,
                start,
                len: decoded.len() as u64,
                offset,
                raw_len: raw.len() as u64,
                encoding,
            });
        }
        let c = self.current.as_mut().unwrap();
        if kind == SpanKind::Sequence {
            c.length += decoded.len() as u64;
            c.gc += decoded
                .iter()
                .filter(|b| matches!(b, b'G' | b'g' | b'C' | b'c'))
                .count() as u64;
            let remaining = 1000usize.saturating_sub(c.sample.len());
            c.sample
                .extend_from_slice(&decoded[..decoded.len().min(remaining)]);
        } else {
            c.has_quality = true;
            for b in decoded {
                let q = b - 33;
                c.qmin = c.qmin.min(q);
                c.qmax = c.qmax.max(q);
                c.qsum += u64::from(q);
                c.q30 += u64::from(q >= 30);
                let bin = ((c.qlen as u128 * 64) / u128::from(c.length.max(1))).min(63) as usize;
                c.qbins[bin] += u64::from(q);
                c.qcounts[bin] += 1;
                c.qlen += 1;
            }
        }
        Ok(())
    }

    fn flush_span(&mut self) -> io::Result<()> {
        if let Some(s) = self.span.take() {
            self.db
                .execute(
                    "INSERT INTO spans VALUES(?,?,?,?,?,?,?)",
                    params![
                        s.record,
                        kind_code(s.kind),
                        s.start,
                        s.len,
                        s.offset,
                        s.raw_len,
                        encoding_code(s.encoding)
                    ],
                )
                .map_err(sql)?;
        }
        Ok(())
    }
}

impl RecordSink for RecordIndex {
    fn current(&self) -> Option<RecordState> {
        self.current.as_ref().map(|c| RecordState {
            length: c.length,
            qlen: c.qlen,
        })
    }
    fn begin_record(&mut self, header: String, has_sequence: bool) -> io::Result<()> {
        self.begin_record(header, has_sequence)
    }
    fn set_plus(&mut self, plus: String) -> io::Result<()> {
        let id = self
            .current
            .as_ref()
            .ok_or_else(|| invalid("Separator before record header"))?
            .id;
        self.db
            .execute("UPDATE records SET plus=? WHERE id=?", params![plus, id])
            .map_err(sql)?;
        Ok(())
    }
    fn set_fields(&mut self, fields: &[(String, String)]) -> io::Result<()> {
        let id = self
            .current
            .as_ref()
            .ok_or_else(|| invalid("Attributes before record header"))?
            .id;
        self.store_fields(id, fields)
    }
    fn set_sequence_missing(&mut self) {
        if let Some(record) = &mut self.current {
            record.has_sequence = false;
        }
    }
    fn append(
        &mut self,
        raw: &[u8],
        offset: u64,
        kind: SpanKind,
        encoding: SpanEncoding,
        decoded: &[u8],
    ) -> io::Result<()> {
        self.append(raw, offset, kind, encoding, decoded)
    }
    fn flush_span(&mut self) -> io::Result<()> {
        self.flush_span()
    }
    fn end_record(&mut self, next_offset: u64) -> io::Result<()> {
        self.end_record(next_offset)
    }
    fn discard_current(&mut self) -> io::Result<()> {
        self.discard_current()
    }
    fn log_error(&mut self, offset: u64, message: &str) -> io::Result<()> {
        self.log_error(offset, message)
    }
    fn set_resume(&mut self, offset: u64) -> io::Result<()> {
        self.set_meta("resume", &offset.to_string())
    }
    fn clear_references(&mut self) -> io::Result<()> {
        self.db.execute("DELETE FROM refs", []).map_err(sql)?;
        Ok(())
    }
    fn add_reference(&mut self, id: i32, name: &str, length: u64) -> io::Result<()> {
        self.db
            .execute("INSERT INTO refs VALUES(?,?,?)", params![id, name, length])
            .map_err(sql)?;
        Ok(())
    }
    fn reference_name(&self, id: i32) -> io::Result<String> {
        if id == -1 {
            return Ok("*".into());
        }
        self.db
            .query_row("SELECT name FROM refs WHERE id=?", [id], |r| r.get(0))
            .optional()
            .map_err(sql)?
            .ok_or_else(|| invalid("BAM references an unknown reference sequence"))
    }
    fn finish_header(&mut self, offset: u64) -> io::Result<()> {
        self.set_meta("bam_header", &offset.to_string())?;
        self.set_resume(offset)
    }
}

// These mappings are the persisted v1 schema; enum layout is not part of that schema.
fn kind_code(kind: SpanKind) -> i64 {
    match kind {
        SpanKind::Sequence => 0,
        SpanKind::Quality => 1,
    }
}
fn encoding_code(encoding: SpanEncoding) -> i64 {
    match encoding {
        SpanEncoding::Text => 0,
        SpanEncoding::BamSequence => 1,
        SpanEncoding::BamQuality => 2,
    }
}
fn encoding_from_code(code: i64) -> io::Result<SpanEncoding> {
    match code {
        0 => Ok(SpanEncoding::Text),
        1 => Ok(SpanEncoding::BamSequence),
        2 => Ok(SpanEncoding::BamQuality),
        _ => Err(invalid("Unknown sequence span encoding")),
    }
}
