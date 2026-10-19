//! Incremental, disk-backed record and sequence indexing.
//!
//! Coordinates in this layer are zero based. Sequence text is never stored in
//! SQLite: spans refer to the source's uncompressed logical byte positions.

mod bam;
#[cfg(test)]
mod tests;
mod text;

use std::{
    collections::hash_map::DefaultHasher,
    fs::{self, File, OpenOptions},
    hash::{Hash, Hasher},
    io::{self, BufRead, BufReader, Read, Seek, SeekFrom},
    os::{fd::AsRawFd, unix::fs::MetadataExt},
    path::{Path, PathBuf},
    sync::{
        Arc,
        atomic::{AtomicBool, AtomicU64, Ordering},
    },
    time::UNIX_EPOCH,
};

use rusqlite::{Connection, OptionalExtension, params};

use crate::source::Source;

const CHUNK: usize = 64 * 1024;
const SCHEMA: u64 = 1;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Format {
    Fasta,
    Fastq,
    Sam,
    Bam,
}
impl Format {
    pub fn can_copy(self) -> bool {
        matches!(self, Self::Fasta | Self::Fastq)
    }
    pub fn name(self) -> &'static str {
        match self {
            Self::Fasta => "FASTA",
            Self::Fastq => "FASTQ",
            Self::Sam => "SAM",
            Self::Bam => "BAM",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SeqType {
    Dna,
    Protein,
    Unknown,
}

#[derive(Debug, Clone)]
pub struct RecordSummary {
    pub id: u64,
    pub name: String,
    pub length: u64,
    pub complete: bool,
}

#[derive(Debug, Clone)]
pub struct RecordMeta {
    pub id: u64,
    pub name: String,
    /// The complete defline without the leading `>` or `@`.
    pub header: String,
    /// FASTQ's complete separator line, including its leading `+`.
    pub plus: String,
    pub length: u64,
    pub quality_length: u64,
    pub complete: bool,
    pub seq_type: SeqType,
    pub gc: Option<f64>,
    pub quality: Option<QualityStats>,
    pub fields: Vec<(String, String)>,
    pub has_sequence: bool,
    pub has_quality: bool,
}

#[derive(Debug, Clone)]
pub struct QualityStats {
    pub min: u8,
    pub max: u8,
    pub mean: f64,
    /// Percentage, in the range 0..=100.
    pub q30: f64,
    /// Mean Phred score in each successive positional bin.
    pub overview: Vec<f64>,
}

#[derive(Debug, Clone, Default)]
pub struct SequenceWindow {
    pub start: u64,
    pub sequence: Vec<u8>,
    /// Phred+33 ASCII characters, including for BAM input.
    pub quality: Vec<u8>,
}

#[derive(Debug, Clone, Default)]
pub struct Progress {
    pub records: u64,
    pub complete: bool,
    pub scanned_bytes: u64,
    pub errors: u64,
    /// The most recent input error, including recoverable SAM row errors.
    pub last_error: Option<String>,
    /// A fatal error stops this file. Recoverable SAM errors only increment errors.
    pub error: Option<String>,
}

#[derive(Debug, Clone, Default)]
pub struct Stats {
    pub records: u64,
    pub sequence_records: u64,
    pub total_length: u64,
    pub min_length: Option<u64>,
    pub max_length: Option<u64>,
    pub mean_length: Option<f64>,
    pub n50: Option<u64>,
    pub gc: Option<f64>,
    pub protein: bool,
    pub complete: bool,
    pub errors: u64,
}

enum Scanner {
    Text(text::TextScanner),
    Bam(bam::BamScanner),
    Done,
}

pub struct Dataset {
    source_path: PathBuf,
    source_identity: [u64; 7],
    reader: BufReader<Source>,
    db: Connection,
    format: Format,
    force_protein: bool,
    scanner: Scanner,
    progress: Progress,
    position: u64,
    current: Option<Current>,
    span: Option<PendingSpan>,
    _cache_lock: File,
    private_cache: Option<PathBuf>,
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
    kind: i64,
    start: u64,
    len: u64,
    offset: u64,
    raw_len: u64,
    encoding: i64,
}

pub(super) fn invalid(message: impl Into<String>) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, message.into())
}
fn sql(error: rusqlite::Error) -> io::Error {
    io::Error::other(error)
}

impl Dataset {
    pub fn open(path: &Path, cache_dir: &Path) -> io::Result<Self> {
        fs::create_dir_all(cache_dir)?;
        let canonical = fs::canonicalize(path)?;
        let stat = fs::metadata(&canonical)?;
        let source_identity = file_identity(&stat);
        let modified = stat
            .modified()?
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_nanos();
        let mut key = DefaultHasher::new();
        canonical.hash(&mut key);
        stat.len().hash(&mut key);
        modified.hash(&mut key);
        stat.dev().hash(&mut key);
        stat.ino().hash(&mut key);
        stat.ctime().hash(&mut key);
        stat.ctime_nsec().hash(&mut key);
        SCHEMA.hash(&mut key);
        let key = key.finish();
        let (directory, cache_lock, private_cache) = lock_cache(cache_dir, key)?;
        let mut reader = BufReader::with_capacity(CHUNK, Source::open(&canonical, &directory)?);
        let prefix = reader.fill_buf()?;
        let format = detect_format(prefix, path)?;
        let extension = path
            .file_name()
            .and_then(|s| s.to_str())
            .unwrap_or("")
            .to_ascii_lowercase();
        let extension = extension.strip_suffix(".gz").unwrap_or(&extension);
        let force_protein = extension.ends_with(".faa") || extension.ends_with(".pep");
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
        let scanner = if complete || fatal.is_some() {
            Scanner::Done
        } else if format == Format::Bam {
            let header_done = get("bam_header")?.is_some();
            Scanner::Bam(bam::BamScanner::new(header_done))
        } else {
            Scanner::Text(text::TextScanner::new(format))
        };
        reader.seek(SeekFrom::Start(position))?;
        let dataset = Self {
            source_path: canonical,
            source_identity,
            reader,
            db,
            format,
            force_protein,
            scanner,
            progress: Progress {
                records,
                complete,
                scanned_bytes: position,
                errors,
                last_error,
                error: fatal,
            },
            position,
            current: None,
            span: None,
            _cache_lock: cache_lock,
            private_cache: private_cache.then_some(directory),
        };
        dataset.check_source()?;
        Ok(dataset)
    }

    pub fn format(&self) -> Format {
        self.format
    }
    pub fn compression(&self) -> &str {
        self.reader.get_ref().compression()
    }
    pub fn progress(&self) -> Progress {
        self.progress.clone()
    }

    /// Perform bounded scan work; the UI worker can preempt between calls.
    pub fn step(&mut self, byte_budget: usize) -> io::Result<Progress> {
        self.check_source()?;
        if matches!(self.scanner, Scanner::Done) {
            return Ok(self.progress());
        }
        self.db.execute_batch("BEGIN IMMEDIATE").map_err(sql)?;
        let mut scanner = std::mem::replace(&mut self.scanner, Scanner::Done);
        let outcome = match &mut scanner {
            Scanner::Text(scan) => scan.step(self, byte_budget.max(1)),
            Scanner::Bam(scan) => scan.step(self, byte_budget.max(1)),
            Scanner::Done => Ok(true),
        }
        .and_then(|done| {
            self.check_source()?;
            Ok(done)
        });
        let result = (|| {
            match outcome {
                Ok(true) => {
                    self.reader.get_mut().finish_index()?;
                    self.progress.complete = true;
                    self.set_meta("complete", "1")?;
                    scanner = Scanner::Done;
                }
                Ok(false) => {}
                Err(error) => {
                    let record = self
                        .current
                        .as_ref()
                        .map_or(self.progress.records + 1, |c| c.id + 1);
                    self.log_error(self.position, &format!("Record {record}: {error}"))?;
                    self.progress.error = Some(format!("At byte {}: {error}", self.position));
                    self.set_meta(
                        "fatal",
                        self.progress.error.as_deref().unwrap_or("Read error"),
                    )?;
                    scanner = Scanner::Done;
                }
            }
            self.flush_span()?;
            self.save_current(false)?;
            self.progress.scanned_bytes = self.position;
            self.set_meta("scanned", &self.position.to_string())?;
            self.db.execute_batch("COMMIT").map_err(sql)
        })();
        if result.is_err() {
            let _ = self.db.execute_batch("ROLLBACK");
        }
        self.scanner = scanner;
        result?;
        Ok(self.progress())
    }

    pub fn records(&self, offset: u64, limit: usize) -> io::Result<Vec<RecordSummary>> {
        self.check_source()?;
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
        self.check_source()?;
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

    /// Reads only indexed spans overlapping the requested interval.
    pub fn read_range(&mut self, id: u64, start: u64, len: usize) -> io::Result<SequenceWindow> {
        self.check_source()?;
        let result = (|| {
            let mut result = SequenceWindow {
                start,
                ..Default::default()
            };
            if len > 0 {
                result.sequence = self.range_kind(id, start, len, 0)?;
                result.quality = self.range_kind(id, start, len, 1)?;
            }
            self.check_source()?;
            Ok(result)
        })();
        let restored = self.reader.seek(SeekFrom::Start(self.position));
        result.and_then(|value| restored.map(|_| value))
    }

    fn range_kind(&mut self, id: u64, start: u64, len: usize, kind: i64) -> io::Result<Vec<u8>> {
        let end = start.saturating_add(len as u64);
        let first: Option<u64> = self.db.query_row("SELECT start FROM spans WHERE record=? AND kind=? AND start<=? ORDER BY start DESC LIMIT 1",
            params![id,kind,start], |r| r.get(0)).optional().map_err(sql)?;
        let mut query = self.db.prepare("SELECT start,len,offset,raw_len,encoding FROM spans WHERE record=? AND kind=? AND start>=? AND start<? AND start+len>? ORDER BY start").map_err(sql)?;
        let mut rows = query
            .query(params![id, kind, first.unwrap_or(start), end, start])
            .map_err(sql)?;
        let mut output = Vec::with_capacity(len.min(CHUNK));
        while let Some(row) = rows.next().map_err(sql)? {
            let span_start: u64 = row.get(0).map_err(sql)?;
            let span_len: u64 = row.get(1).map_err(sql)?;
            let offset: u64 = row.get(2).map_err(sql)?;
            let raw_len: u64 = row.get(3).map_err(sql)?;
            let encoding: i64 = row.get(4).map_err(sql)?;
            self.reader.seek(SeekFrom::Start(offset))?;
            let mut bytes = vec![0; raw_len as usize];
            self.reader.read_exact(&mut bytes)?;
            let decoded: Vec<u8> = match encoding {
                1 => bytes
                    .into_iter()
                    .flat_map(|b| [BAM_BASES[(b >> 4) as usize], BAM_BASES[(b & 15) as usize]])
                    .take(span_len as usize)
                    .collect(),
                2 => bytes.into_iter().map(|q| q.saturating_add(33)).collect(),
                _ => bytes
                    .into_iter()
                    .filter(|b| !b.is_ascii_whitespace())
                    .collect(),
            };
            let from = start.saturating_sub(span_start) as usize;
            let to = (end - span_start).min(span_len) as usize;
            if from < to && to <= decoded.len() {
                output.extend_from_slice(&decoded[from..to]);
            }
        }
        Ok(output)
    }

    /// Search indexed names from `after` inclusively; the caller controls wraparound.
    pub fn find_name(&self, query: &str, after: u64) -> io::Result<Option<u64>> {
        self.check_source()?;
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
        self.check_source()?;
        let (count, total, min, max, gc, protein, dna): (u64,u64,Option<u64>,Option<u64>,u64,u64,u64) = self.db.query_row(
            "SELECT COUNT(*),COALESCE(SUM(length),0),MIN(length),MAX(length),COALESCE(SUM(gc),0),COALESCE(SUM(seqtype=1),0),COALESCE(SUM(seqtype=0),0) FROM records WHERE complete=1 AND has_seq=1", [],
            |r| Ok((r.get(0)?,r.get(1)?,r.get(2)?,r.get(3)?,r.get(4)?,r.get(5)?,r.get(6)?))).map_err(sql)?;
        let mut n50 = None;
        if self.format.can_copy() && count > 0 {
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
        self.check_source()?;
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

    /// Explicit copying may allocate the requested complete record. Scanning and browsing do not.
    pub fn copy_record(&mut self, id: u64, cancel: &AtomicBool) -> io::Result<String> {
        self.check_source()?;
        let result = self.copy_record_inner(id, cancel).and_then(|value| {
            self.check_source()?;
            Ok(value)
        });
        let restored = self.reader.seek(SeekFrom::Start(self.position));
        result.and_then(|value| restored.map(|_| value))
    }

    fn copy_record_inner(&mut self, id: u64, cancel: &AtomicBool) -> io::Result<String> {
        if !self.format.can_copy() {
            return Err(io::Error::new(
                io::ErrorKind::PermissionDenied,
                "Copying is disabled for SAM/BAM files",
            ));
        }
        let meta = self
            .record(id)?
            .ok_or_else(|| invalid("Record is not indexed"))?;
        if !meta.complete {
            return Err(io::Error::new(
                io::ErrorKind::WouldBlock,
                "Record indexing is not complete",
            ));
        }
        let mut output = format!(
            "{}{}\n",
            if self.format == Format::Fasta {
                '>'
            } else {
                '@'
            },
            meta.header
        );
        let mut pos = 0;
        while pos < meta.length {
            if cancel.load(Ordering::Relaxed) {
                return Err(io::Error::new(io::ErrorKind::Interrupted, "Copy cancelled"));
            }
            let data = self.range_kind(id, pos, CHUNK, 0)?;
            if data.is_empty() {
                return Err(invalid("Incomplete sequence index"));
            }
            for (i, b) in data.iter().enumerate() {
                output.push(char::from(*b));
                if self.format == Format::Fasta && (pos + i as u64 + 1).is_multiple_of(60) {
                    output.push('\n');
                }
            }
            pos += data.len() as u64;
        }
        if !output.ends_with('\n') || meta.length == 0 {
            output.push('\n');
        }
        if self.format == Format::Fastq {
            output.push_str(&meta.plus);
            output.push('\n');
            pos = 0;
            while pos < meta.quality_length {
                if cancel.load(Ordering::Relaxed) {
                    return Err(io::Error::new(io::ErrorKind::Interrupted, "Copy cancelled"));
                }
                let data = self.range_kind(id, pos, CHUNK, 1)?;
                if data.is_empty() {
                    return Err(invalid("Incomplete quality index"));
                }
                output.extend(data.iter().map(|b| char::from(*b)));
                pos += data.len() as u64;
            }
            output.push('\n');
        }
        Ok(output)
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

    fn check_source(&self) -> io::Result<()> {
        let metadata = fs::metadata(&self.source_path).map_err(|error| {
            invalid(format!("Source changed or is no longer available: {error}"))
        })?;
        if file_identity(&metadata) != self.source_identity {
            return Err(invalid(
                "Source changed while open; reopen the file to rebuild its index",
            ));
        }
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
            self.set_fields(id, &crate::headers::parse_fastq_header(&header))?;
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

    fn set_fields(&self, id: u64, fields: &[(String, String)]) -> io::Result<()> {
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
            } else if !self.format.can_copy() {
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
        kind: i64,
        encoding: i64,
        decoded: &[u8],
    ) -> io::Result<()> {
        if decoded.is_empty() {
            return Ok(());
        }
        let c = self
            .current
            .as_ref()
            .ok_or_else(|| invalid("Sequence before record header"))?;
        let start = if kind == 0 { c.length } else { c.qlen };
        let id = c.id;
        if self.span.as_ref().is_some_and(|s| {
            s.record != id
                || s.kind != kind
                || s.encoding != encoding
                || offset + raw.len() as u64 - s.offset > CHUNK as u64
                || encoding != 0
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
        if kind == 0 {
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
                        s.record, s.kind, s.start, s.len, s.offset, s.raw_len, s.encoding
                    ],
                )
                .map_err(sql)?;
        }
        Ok(())
    }

    fn take_bytes(&mut self, max: usize) -> io::Result<Vec<u8>> {
        let available = self.reader.fill_buf()?;
        let count = max.min(CHUNK).min(available.len());
        let bytes = available[..count].to_vec();
        self.reader.consume(count);
        self.position += count as u64;
        Ok(bytes)
    }
}

fn file_identity(metadata: &fs::Metadata) -> [u64; 7] {
    [
        metadata.dev(),
        metadata.ino(),
        metadata.len(),
        metadata.mtime() as u64,
        metadata.mtime_nsec() as u64,
        metadata.ctime() as u64,
        metadata.ctime_nsec() as u64,
    ]
}

impl Drop for Dataset {
    fn drop(&mut self) {
        // Private concurrent-reader caches do not replace the reusable writer index.
        // macOS/Linux permit unlinking these files while their final handles close.
        if let Some(directory) = &self.private_cache {
            let _ = fs::remove_dir_all(directory);
        }
    }
}

fn lock_cache(directory: &Path, key: u64) -> io::Result<(PathBuf, File, bool)> {
    let open_lock = |directory: &Path| {
        OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(false)
            .open(directory.join(format!("{key:016x}.records.lock")))
    };
    let lock = open_lock(directory)?;
    if unsafe { libc::flock(lock.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) } == 0 {
        return Ok((directory.to_owned(), lock, false));
    }
    let error = io::Error::last_os_error();
    if error.kind() != io::ErrorKind::WouldBlock {
        return Err(error);
    }
    static NEXT_READER: AtomicU64 = AtomicU64::new(0);
    loop {
        let private = directory.join(format!(
            "reader-{key:016x}-{}-{}",
            std::process::id(),
            NEXT_READER.fetch_add(1, Ordering::Relaxed)
        ));
        match fs::create_dir(&private) {
            Ok(()) => {
                let lock = open_lock(&private)?;
                if unsafe { libc::flock(lock.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) } != 0 {
                    return Err(io::Error::last_os_error());
                }
                return Ok((private, lock, true));
            }
            Err(error) if error.kind() == io::ErrorKind::AlreadyExists => continue,
            Err(error) => return Err(error),
        }
    }
}

const BAM_BASES: &[u8; 16] = b"=ACMGRSVTWYHKDBN";

fn detect_format(prefix: &[u8], path: &Path) -> io::Result<Format> {
    if prefix.starts_with(b"BAM\x01") {
        return Ok(Format::Bam);
    }
    let mut content = prefix;
    let mut comment = false;
    loop {
        if content.starts_with(b"#") {
            comment = true;
            content = content
                .iter()
                .position(|b| *b == b'\n')
                .map_or(&[][..], |n| &content[n + 1..]);
            continue;
        }
        if let Some(position) = content.iter().position(|b| !b.is_ascii_whitespace()) {
            if position > 0 {
                content = &content[position..];
                continue;
            }
        } else {
            content = &[];
        }
        break;
    }
    let prefix = content;
    if prefix.starts_with(b">") {
        return Ok(Format::Fasta);
    }
    if [b"@HD\t", b"@SQ\t", b"@RG\t", b"@PG\t", b"@CO\t"]
        .iter()
        .any(|mark| prefix.starts_with(*mark))
    {
        return Ok(Format::Sam);
    }
    if prefix.starts_with(b"@") {
        return Ok(Format::Fastq);
    }
    if prefix
        .iter()
        .take_while(|b| **b != b'\n')
        .filter(|b| **b == b'\t')
        .count()
        >= 2
    {
        return Ok(Format::Sam);
    }
    if prefix.is_empty() {
        if comment {
            return Ok(Format::Fasta);
        }
        let name = path.to_string_lossy().to_ascii_lowercase();
        let name = name.strip_suffix(".gz").unwrap_or(&name);
        if name.ends_with(".sam") {
            return Ok(Format::Sam);
        }
        if name.ends_with(".fq") || name.ends_with(".fastq") {
            return Ok(Format::Fastq);
        }
        if [".fa", ".fasta", ".fna", ".fas", ".fsa", ".faa", ".pep"]
            .iter()
            .any(|ext| name.ends_with(ext))
        {
            return Ok(Format::Fasta);
        }
    }
    Err(invalid(
        "Input is not recognised as FASTA, FASTQ, SAM or BAM",
    ))
}

fn detect_type(sample: &[u8]) -> SeqType {
    let non_dna: Vec<u8> = sample
        .iter()
        .map(u8::to_ascii_uppercase)
        .filter(|b| !b"ATCGUNRYWSMKBDHV-.".contains(b))
        .collect();
    if non_dna.is_empty() {
        SeqType::Dna
    } else if non_dna.iter().all(|b| b"EFILPQXJZO*".contains(b))
        && non_dna.iter().any(|b| b"EFILPQ".contains(b))
    {
        SeqType::Protein
    } else {
        SeqType::Unknown
    }
}
