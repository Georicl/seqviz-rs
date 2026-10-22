//! Disk-backed, bounded VCF indexing. Raw record/sample text stays in the source.
use super::model::*;
use super::reader::{invalid, parse_record, read_header};
use crate::core::{
    cache::{file_identity, lock_cache},
    source::Source,
};
use rusqlite::{Connection, OptionalExtension, params};
use std::{
    collections::{HashMap, hash_map::DefaultHasher},
    fs::{self, File},
    hash::{Hash, Hasher},
    io::{self, BufRead, BufReader, Read, Seek, SeekFrom},
    path::{Path, PathBuf},
    sync::Arc,
};

const CHUNK: usize = 64 * 1024;
const SCHEMA: u64 = 3;

fn sql(error: rusqlite::Error) -> io::Error {
    io::Error::other(error)
}

struct Checkpoint {
    progress: Progress,
    contig_counts: HashMap<String, u64>,
    next_contig: u64,
}

impl Checkpoint {
    fn load(db: &Connection, header_end: u64, header_lines: u64) -> io::Result<Self> {
        let meta = |key: &str| -> io::Result<Option<String>> {
            db.query_row("SELECT value FROM meta WHERE key=?1", [key], |row| {
                row.get(0)
            })
            .optional()
            .map_err(sql)
        };
        let resume = meta("resume")?
            .and_then(|s| s.parse().ok())
            .unwrap_or(header_end);
        let line = meta("line")?
            .and_then(|s| s.parse().ok())
            .unwrap_or(header_lines);
        let complete = meta("complete")?.as_deref() == Some("1");
        let error = meta("error")?;
        let records = match meta("records")?.and_then(|value| value.parse().ok()) {
            Some(records) => records,
            None => db
                .query_row("SELECT COUNT(*) FROM records", [], |row| {
                    row.get::<_, u64>(0)
                })
                .map_err(sql)?,
        };
        let mut contig_counts = HashMap::new();
        let mut statement = db
            .prepare("SELECT name,records FROM contigs")
            .map_err(sql)?;
        for row in statement
            .query_map([], |row| {
                Ok((row.get::<_, String>(0)?, row.get::<_, u64>(1)?))
            })
            .map_err(sql)?
        {
            let (name, count) = row.map_err(sql)?;
            contig_counts.insert(name, count);
        }
        Ok(Self {
            progress: Progress {
                records,
                scanned_bytes: resume,
                complete,
                errors: u64::from(error.is_some()),
                error,
                line,
            },
            next_contig: contig_counts.len() as u64,
            contig_counts,
        })
    }
}

pub(crate) struct Dataset {
    source_path: PathBuf,
    source_identity: [u64; 7],
    reader: BufReader<Source>,
    db: Connection,
    header: Arc<VcfHeader>,
    progress: Progress,
    /// Decoded read position, including pending bytes; restored after reading a selected record.
    position: u64,
    /// End of the last fully indexed line; the offset persisted for scan recovery.
    resume: u64,
    pending: Vec<u8>,
    contig_counts: HashMap<String, u64>,
    next_contig: u64,
    selected: Option<(u64, Arc<VcfEntry>)>,
    _lock: File,
    private_cache: Option<PathBuf>,
}

impl Dataset {
    pub(crate) fn open(path: &Path, cache: &Path) -> io::Result<Self> {
        let canonical = fs::canonicalize(path)?;
        let identity = file_identity(&fs::metadata(&canonical)?);
        let mut hasher = DefaultHasher::new();
        "seqviz-vcf".hash(&mut hasher);
        SCHEMA.hash(&mut hasher);
        canonical.hash(&mut hasher);
        identity.hash(&mut hasher);
        let key = hasher.finish();
        let directory = cache.join("vcf").join(format!("{key:016x}"));
        fs::create_dir_all(&directory)?;
        let (directory, lock, private) = lock_cache(&directory, key)?;
        let source = Source::open(&canonical, &directory)?;
        let mut reader = BufReader::with_capacity(CHUNK, source);
        let (header, header_end, header_lines) = read_header(&mut reader)?;
        let db = Connection::open(directory.join("vcf.sqlite")).map_err(sql)?;
        db.execute_batch(
            "PRAGMA journal_mode=WAL;
            PRAGMA synchronous=NORMAL;
            PRAGMA cache_size=-2048;
            PRAGMA temp_store=FILE;
            CREATE TABLE IF NOT EXISTS meta (
                key TEXT PRIMARY KEY,
                value TEXT NOT NULL
            );
            CREATE TABLE IF NOT EXISTS contigs (
                name TEXT PRIMARY KEY,
                ordinal INTEGER NOT NULL UNIQUE,
                records INTEGER NOT NULL DEFAULT 0
            );
            CREATE TABLE IF NOT EXISTS records (
                id INTEGER PRIMARY KEY,
                contig TEXT NOT NULL,
                ordinal INTEGER NOT NULL,
                pos INTEGER NOT NULL,
                offset INTEGER NOT NULL,
                bytes INTEGER NOT NULL,
                ids TEXT NOT NULL,
                reference TEXT NOT NULL,
                alternates TEXT NOT NULL,
                qual REAL,
                filter TEXT NOT NULL,
                kind INTEGER NOT NULL
            );
            CREATE UNIQUE INDEX IF NOT EXISTS contig_order ON records(contig, ordinal);
            CREATE INDEX IF NOT EXISTS loci ON records(contig, pos, id);",
        )
        .map_err(sql)?;
        for (ordinal, name) in header.contigs.iter().enumerate() {
            db.execute(
                "INSERT OR IGNORE INTO contigs(name,ordinal,records) VALUES(?1,?2,0)",
                params![name, ordinal as u64],
            )
            .map_err(sql)?;
        }
        let checkpoint = Checkpoint::load(&db, header_end, header_lines)?;
        let resume = checkpoint.progress.scanned_bytes;
        reader.seek(SeekFrom::Start(resume))?;
        let dataset = Self {
            source_path: canonical,
            source_identity: identity,
            reader,
            db,
            header: Arc::new(header),
            progress: checkpoint.progress,
            position: resume,
            resume,
            pending: Vec::new(),
            contig_counts: checkpoint.contig_counts,
            next_contig: checkpoint.next_contig,
            selected: None,
            _lock: lock,
            private_cache: private.then_some(directory),
        };
        dataset.check_source()?;
        Ok(dataset)
    }

    pub(crate) fn header(&self) -> Arc<VcfHeader> {
        Arc::clone(&self.header)
    }
    pub(crate) fn progress(&self) -> Progress {
        self.progress.clone()
    }
    pub(crate) fn compression(&self) -> &str {
        self.reader.get_ref().compression()
    }

    pub(crate) fn step(&mut self, byte_budget: usize) -> io::Result<Progress> {
        self.check_source()?;
        if self.progress.complete || self.progress.error.is_some() {
            return Ok(self.progress());
        }
        if let Err(error) = self.db.execute_batch("BEGIN IMMEDIATE") {
            return self.failed_step(sql(error));
        }
        let outcome = self.advance(byte_budget.max(1));
        if let Err(error) = self.check_source() {
            return self.failed_step(error);
        }
        if let Err(error) = outcome {
            if error
                .get_ref()
                .is_some_and(|source| source.is::<rusqlite::Error>())
            {
                return self.failed_step(error);
            }
            self.progress.errors += 1;
            self.progress.error = Some(format!(
                "At VCF line {}, decoded byte {}: {error}",
                self.progress.line, self.resume
            ));
        }
        let result = (|| {
            self.set_meta("resume", &self.resume.to_string())?;
            self.set_meta("line", &self.progress.line.to_string())?;
            self.set_meta("records", &self.progress.records.to_string())?;
            if self.progress.complete {
                self.set_meta("complete", "1")?;
            }
            if let Some(error) = &self.progress.error {
                self.set_meta("error", error)?;
            }
            self.db.execute_batch("COMMIT").map_err(sql)
        })();
        if let Err(error) = result {
            return self.failed_step(error);
        }
        self.progress.scanned_bytes = self.position;
        Ok(self.progress())
    }

    fn failed_step(&mut self, error: io::Error) -> io::Result<Progress> {
        let restored = (|| {
            // SQLITE_FULL may already have rolled the transaction back itself.
            if !self.db.is_autocommit() {
                self.db.execute_batch("ROLLBACK").map_err(sql)?;
            }
            let checkpoint = Checkpoint::load(
                &self.db,
                self.header.raw.len() as u64,
                self.header.raw.lines().count() as u64,
            )?;
            self.progress = checkpoint.progress;
            self.resume = self.progress.scanned_bytes;
            self.position = self.resume;
            self.contig_counts = checkpoint.contig_counts;
            self.next_contig = checkpoint.next_contig;
            self.pending.clear();
            self.selected = None;
            self.reader.seek(SeekFrom::Start(self.resume))?;
            Ok::<_, io::Error>(())
        })();
        match restored {
            Ok(()) => Err(error),
            Err(recovery) => Err(io::Error::other(format!(
                "{error}; restoring VCF cache checkpoint failed: {recovery}"
            ))),
        }
    }

    fn advance(&mut self, budget: usize) -> io::Result<()> {
        let mut used = 0;
        while used < budget {
            let available = self.reader.fill_buf()?;
            if available.is_empty() {
                if !self.pending.is_empty() {
                    self.finish_line()?;
                }
                self.reader.get_mut().finish_index()?;
                self.progress.complete = true;
                return Ok(());
            }
            let bound = available.len().min(budget - used);
            let take = available[..bound]
                .iter()
                .position(|b| *b == b'\n')
                .map_or(bound, |n| n + 1);
            let newline = available[take - 1] == b'\n';
            self.pending.extend_from_slice(&available[..take]);
            self.reader.consume(take);
            self.position += take as u64;
            used += take;
            if newline {
                self.finish_line()?;
            }
        }
        Ok(())
    }

    fn finish_line(&mut self) -> io::Result<()> {
        self.progress.line += 1;
        if self.pending.iter().all(|b| matches!(*b, b'\n' | b'\r')) {
            self.pending.clear();
            self.resume = self.position;
            return Ok(());
        }
        let entry = parse_record(&self.pending, &self.header)?;
        if entry.record.pos > i64::MAX as u64 {
            return Err(invalid("POS exceeds supported integer range"));
        }
        let record = &entry.record;
        let ordinal = self.contig_counts.get(&record.chrom).copied().unwrap_or(0);
        if !self.contig_counts.contains_key(&record.chrom) {
            self.db
                .execute(
                    "INSERT INTO contigs(name,ordinal,records) VALUES(?1,?2,0)",
                    params![record.chrom, self.next_contig],
                )
                .map_err(sql)?;
            self.next_contig += 1;
        }
        let kind = entry
            .variants
            .first()
            .map_or(VariantKind::Other, |variant| {
                if entry
                    .variants
                    .iter()
                    .any(|other| other.kind != variant.kind)
                {
                    VariantKind::Mixed
                } else {
                    variant.kind
                }
            });
        self.db
            .execute(
                "INSERT INTO records (
                    id, contig, ordinal, pos, offset, bytes,
                    ids, reference, alternates, qual, filter, kind
                ) VALUES (
                    ?1, ?2, ?3, ?4, ?5, ?6,
                    ?7, ?8, ?9, ?10, ?11, ?12
                )",
                params![
                    self.progress.records,
                    record.chrom,
                    ordinal,
                    record.pos,
                    self.resume,
                    self.pending.len() as u64,
                    short(&record.ids.join(";")),
                    short(&record.reference),
                    short(&record.alternates.join(",")),
                    record.qual,
                    short(&record.filter),
                    kind_code(kind),
                ],
            )
            .map_err(sql)?;
        self.db
            .execute(
                "UPDATE contigs SET records=records+1 WHERE name=?1",
                [&record.chrom],
            )
            .map_err(sql)?;
        self.contig_counts.insert(record.chrom.clone(), ordinal + 1);
        self.progress.records += 1;
        self.pending.clear();
        self.resume = self.position;
        Ok(())
    }

    pub(crate) fn total(&self, contig: Option<&str>) -> io::Result<u64> {
        self.check_source()?;
        Ok(contig.map_or(self.progress.records, |name| {
            self.contig_counts.get(name).copied().unwrap_or(0)
        }))
    }

    pub(crate) fn contigs(&self) -> io::Result<Vec<ContigSummary>> {
        self.check_source()?;
        let mut statement = self
            .db
            .prepare("SELECT name,records FROM contigs ORDER BY ordinal")
            .map_err(sql)?;
        statement
            .query_map([], |row| {
                Ok(ContigSummary {
                    name: row.get(0)?,
                    records: row.get(1)?,
                })
            })
            .map_err(sql)?
            .map(|row| row.map_err(sql))
            .collect()
    }

    pub(crate) fn rows(
        &self,
        contig: Option<&str>,
        start: u64,
        count: usize,
    ) -> io::Result<Vec<RecordSummary>> {
        self.check_source()?;
        let count = count.min(512) as u64;
        let columns = "SELECT id,contig,pos,ids,reference,alternates,qual,filter,kind,";
        let (query, first) = if let Some(contig) = contig {
            (
                format!(
                    "{columns}ordinal FROM records WHERE contig=?1 AND ordinal>=?2 ORDER BY ordinal LIMIT ?3"
                ),
                Some(contig),
            )
        } else {
            (
                format!("{columns}id FROM records WHERE id>=?2 ORDER BY id LIMIT ?3"),
                None,
            )
        };
        let mut statement = self.db.prepare(&query).map_err(sql)?;
        let mapper = |row: &rusqlite::Row<'_>| -> rusqlite::Result<RecordSummary> {
            Ok(RecordSummary {
                id: row.get(0)?,
                chrom: row.get(1)?,
                pos: row.get(2)?,
                ids: row.get(3)?,
                reference: row.get(4)?,
                alternates: row.get(5)?,
                qual: row.get(6)?,
                filter: row.get(7)?,
                kind: decode_kind(row.get(8)?),
                ordinal: row.get(9)?,
            })
        };
        // Numbered placeholders intentionally keep identical binding positions in both views.
        statement
            .query_map(params![first, start, count], mapper)
            .map_err(sql)?
            .map(|row| row.map_err(sql))
            .collect()
    }

    pub(crate) fn entry(
        &mut self,
        contig: Option<&str>,
        ordinal: u64,
    ) -> io::Result<Option<Arc<VcfEntry>>> {
        self.check_source()?;
        let row: Option<(u64, u64, u64)> = if let Some(contig) = contig {
            self.db
                .query_row(
                    "SELECT id,offset,bytes FROM records WHERE contig=?1 AND ordinal=?2",
                    params![contig, ordinal],
                    |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
                )
                .optional()
                .map_err(sql)?
        } else {
            self.db
                .query_row(
                    "SELECT id,offset,bytes FROM records WHERE id=?1",
                    [ordinal],
                    |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
                )
                .optional()
                .map_err(sql)?
        };
        let Some((id, offset, length)) = row else {
            return Ok(None);
        };
        if let Some((cached, entry)) = &self.selected
            && *cached == id
        {
            return Ok(Some(Arc::clone(entry)));
        }
        let result = (|| {
            let length = usize::try_from(length)
                .map_err(|_| invalid("VCF record length exceeds addressable memory"))?;
            let mut raw = Vec::new();
            raw.try_reserve_exact(length)
                .map_err(|e| io::Error::other(e.to_string()))?;
            raw.resize(length, 0);
            self.reader.seek(SeekFrom::Start(offset))?;
            self.reader.read_exact(&mut raw)?;
            let entry = Arc::new(parse_record(&raw, &self.header)?);
            self.check_source()?;
            Ok(entry)
        })();
        let restored = self.reader.seek(SeekFrom::Start(self.position));
        let entry = result.and_then(|entry| restored.map(|_| entry))?;
        self.selected = Some((id, Arc::clone(&entry)));
        Ok(Some(entry))
    }

    /// Find an exact position immediately; otherwise use the first position >= target
    /// only after indexing completes, so unsorted input cannot produce a false early hit.
    pub(crate) fn locate(&self, contig: &str, position: u64) -> io::Result<Option<u64>> {
        self.check_source()?;
        let position = i64::try_from(position)
            .map_err(|_| invalid("Position exceeds supported integer range"))?;
        let exact = self
            .db
            .query_row(
                "SELECT ordinal FROM records WHERE contig=?1 AND pos=?2 ORDER BY id LIMIT 1",
                params![contig, position],
                |row| row.get(0),
            )
            .optional()
            .map_err(sql)?;
        if exact.is_some() || !self.progress.complete {
            return Ok(exact);
        }
        let next = self
            .db
            .query_row(
                "SELECT ordinal FROM records WHERE contig=?1 AND pos>=?2 ORDER BY pos,id LIMIT 1",
                params![contig, position],
                |row| row.get(0),
            )
            .optional()
            .map_err(sql)?;
        if next.is_some() {
            return Ok(next);
        }
        self.db
            .query_row(
                "SELECT ordinal FROM records WHERE contig=?1 ORDER BY pos DESC,id DESC LIMIT 1",
                [contig],
                |row| row.get(0),
            )
            .optional()
            .map_err(sql)
    }

    fn set_meta(&self, key: &str, value: &str) -> io::Result<()> {
        self.db
            .execute(
                "INSERT OR REPLACE INTO meta(key,value) VALUES(?1,?2)",
                [key, value],
            )
            .map_err(sql)?;
        Ok(())
    }

    fn check_source(&self) -> io::Result<()> {
        if file_identity(
            &fs::metadata(&self.source_path)
                .map_err(|e| invalid(format!("Source is no longer available: {e}")))?,
        ) != self.source_identity
        {
            return Err(invalid(
                "Source changed while open; reopen the VCF to rebuild its index",
            ));
        }
        Ok(())
    }
}

impl Drop for Dataset {
    fn drop(&mut self) {
        if let Some(directory) = &self.private_cache {
            let _ = fs::remove_dir_all(directory);
        }
    }
}

fn short(value: &str) -> String {
    let mut chars = value.chars();
    let mut out: String = chars.by_ref().take(63).collect();
    if let Some(last) = chars.next() {
        if chars.next().is_some() {
            out.push('…');
        } else {
            out.push(last);
        }
    }
    out
}

fn kind_code(kind: VariantKind) -> u8 {
    match kind {
        VariantKind::Snv => 0,
        VariantKind::Mnv => 1,
        VariantKind::Insertion => 2,
        VariantKind::Deletion => 3,
        VariantKind::Symbolic => 4,
        VariantKind::Breakend => 5,
        VariantKind::SpanningDeletion => 6,
        VariantKind::Other => 7,
        VariantKind::Mixed => 8,
    }
}

fn decode_kind(code: u8) -> VariantKind {
    match code {
        0 => VariantKind::Snv,
        1 => VariantKind::Mnv,
        2 => VariantKind::Insertion,
        3 => VariantKind::Deletion,
        4 => VariantKind::Symbolic,
        5 => VariantKind::Breakend,
        6 => VariantKind::SpanningDeletion,
        8 => VariantKind::Mixed,
        _ => VariantKind::Other,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn indexed_count(data: &Dataset) -> u64 {
        data.db
            .query_row("SELECT COUNT(*) FROM records", [], |row| row.get(0))
            .unwrap()
    }

    #[test]
    fn sqlite_full_restores_committed_checkpoint_and_can_resume_without_lost_records() {
        for reopen_after_failure in [false, true] {
            let directory = tempfile::tempdir().unwrap();
            let path = directory.path().join("variants.vcf");
            let cache = directory.path().join("cache");
            let mut raw =
                "##fileformat=VCFv4.3\n#CHROM\tPOS\tID\tREF\tALT\tQUAL\tFILTER\tINFO\n".to_owned();
            for pos in 1..=10_000 {
                raw.push_str(&format!(
                    "chr{}\t{pos}\tlong_identifier_for_filling_cache\tA\tC\t40\tPASS\t.\n",
                    pos % 4
                ));
            }
            fs::write(&path, raw).unwrap();
            let mut data = Dataset::open(&path, &cache).unwrap();
            data.step(128).unwrap();
            assert!(
                !data.pending.is_empty(),
                "Fixture must include an uncommitted partial line"
            );
            let expected = Checkpoint::load(
                &data.db,
                data.header.raw.len() as u64,
                data.header.raw.lines().count() as u64,
            )
            .unwrap();
            let maximum: u64 = data
                .db
                .query_row("PRAGMA max_page_count", [], |row| row.get(0))
                .unwrap();
            let pages: u64 = data
                .db
                .query_row("PRAGMA page_count", [], |row| row.get(0))
                .unwrap();
            data.db
                .execute_batch(&format!("PRAGMA max_page_count={pages}"))
                .unwrap();

            let error = data.step(CHUNK).unwrap_err();
            assert!(
                matches!(error.get_ref().and_then(|source| source.downcast_ref::<rusqlite::Error>()),
                Some(rusqlite::Error::SqliteFailure(code, _)) if code.code == rusqlite::ErrorCode::DiskFull),
                "{error:?}"
            );
            assert!(data.db.is_autocommit());
            assert_eq!(data.progress(), expected.progress);
            assert_eq!(data.position, expected.progress.scanned_bytes);
            assert_eq!(data.resume, expected.progress.scanned_bytes);
            assert_eq!(
                data.reader.stream_position().unwrap(),
                expected.progress.scanned_bytes
            );
            assert!(data.pending.is_empty());
            assert_eq!(data.contig_counts, expected.contig_counts);
            assert_eq!(data.next_contig, expected.next_contig);
            assert_eq!(indexed_count(&data), expected.progress.records);
            assert_eq!(data.total(None).unwrap(), expected.progress.records);
            assert_eq!(
                data.rows(None, 0, 512).unwrap().len() as u64,
                expected.progress.records
            );
            let stored_error: Option<String> = data
                .db
                .query_row("SELECT value FROM meta WHERE key='error'", [], |row| {
                    row.get(0)
                })
                .optional()
                .unwrap();
            assert!(
                stored_error.is_none(),
                "Storage failures must not become persistent input errors"
            );

            data.db
                .execute_batch(&format!("PRAGMA max_page_count={maximum}"))
                .unwrap();
            if reopen_after_failure {
                drop(data);
                data = Dataset::open(&path, &cache).unwrap();
                assert_eq!(data.progress(), expected.progress);
                assert_eq!(indexed_count(&data), expected.progress.records);
                assert_eq!(data.contig_counts, expected.contig_counts);
            }
            while !data.progress().complete {
                let progress = data.step(CHUNK).unwrap();
                assert!(progress.error.is_none());
            }
            assert_eq!(data.total(None).unwrap(), 10_000);
            assert_eq!(indexed_count(&data), 10_000);
            assert!(data.contig_counts.values().all(|&records| records == 2_500));
            let positions: (u64, u64) = data
                .db
                .query_row(
                    "SELECT COUNT(DISTINCT pos), SUM(pos) FROM records",
                    [],
                    |row| Ok((row.get(0)?, row.get(1)?)),
                )
                .unwrap();
            assert_eq!(positions, (10_000, 10_000 * 10_001 / 2));
            assert_eq!(data.entry(None, 9_999).unwrap().unwrap().record.pos, 10_000);
            drop(data);
            let reopened = Dataset::open(&path, &cache).unwrap();
            assert!(reopened.progress().complete && reopened.progress().error.is_none());
            assert_eq!(reopened.total(None).unwrap(), 10_000);
            assert_eq!(indexed_count(&reopened), 10_000);
        }
    }
}
