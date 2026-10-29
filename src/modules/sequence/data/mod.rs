//! Incremental, disk-backed record and sequence access.
//!
//! Coordinates are zero based. Source handles logical-byte I/O, scanners decode
//! format structure, and RecordIndex owns persistence and record aggregates.
/// Incremental binary alignment decoding after source decompression.
mod bam;
/// Format recognition and sequence-alphabet classification.
mod classify;
/// Persistent records, source spans and aggregate statistics.
mod index;
/// Public record, progress and statistics values.
mod model;
/// Storage-independent scanner tests across format and chunk boundaries.
#[cfg(test)]
mod parser_tests;
/// Logical-byte parser input and the record-sink contract.
mod scan;
/// Dataset integration tests for indexes, range reads and source changes.
#[cfg(test)]
mod tests;
/// Incremental FASTA, FASTQ and SAM parsing.
mod text;

use crate::core::{cache, source::Source};
use cache::{file_identity, lock_cache};
use classify::detect_format;
#[cfg(test)]
use classify::detect_type;
use index::RecordIndex;
pub use model::*;
use scan::{ScanInput, SpanEncoding, SpanKind};
use std::{
    collections::hash_map::DefaultHasher,
    fs::{self, File},
    hash::{Hash, Hasher},
    io::{self, BufRead, BufReader, Read, Seek, SeekFrom},
    os::unix::fs::MetadataExt,
    path::{Path, PathBuf},
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    time::UNIX_EPOCH,
};

/// Maximum source fragment and coalesced text-span size in bytes.
const CHUNK: usize = 64 * 1024;
/// Record-cache schema version included in the cache identity.
const SCHEMA: u64 = 1;
/// BAM four-bit nucleotide codes in nibble-value order.
const BAM_BASES: &[u8; 16] = b"=ACMGRSVTWYHKDBN";

/// Construct an invalid-data error for malformed input or a stale index.
pub(super) fn invalid(message: impl Into<String>) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, message.into())
}

/// Active incremental parser, or a terminal state after completion or fatal input failure.
enum Scanner {
    /// Stateful parser for FASTA, FASTQ or SAM.
    Text(text::TextScanner),
    /// Stateful parser for decoded BAM bytes.
    Bam(bam::BamScanner),
    /// Scanning has completed or stopped on a fatal input error.
    Done,
}

/// A sequence source with a persistent record index and an independently restorable scan cursor.
pub struct Dataset {
    /// Canonical source path used for identity checks.
    source_path: PathBuf,
    /// File metadata fingerprint captured when the source opened.
    source_identity: [u64; 7],
    /// Buffered logical-byte source shared by scanning and interval reads.
    reader: BufReader<Source>,
    /// Persistent records, source spans and scan progress.
    index: RecordIndex,
    /// Format-specific parser state for the next bounded scan step.
    scanner: Scanner,
    /// Logical-byte scan cursor restored after interval reads and copying.
    position: u64,
    /// Held for the dataset lifetime to reserve its cache directory.
    _cache_lock: File,
    /// Temporary directory to remove when another handle owns the reusable cache.
    private_cache: Option<PathBuf>,
}

impl Dataset {
    /// Open the source and reuse its index, resuming incomplete scans at a record boundary.
    /// Canonical path, file identity and schema version select the cache; contending writers
    /// receive a private cache. `.faa` and `.pep` inputs are classified as protein.
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
        let (index, position, header_done) =
            RecordIndex::open(&directory, key, format, force_protein)?;
        let scanner = if index.progress.complete || index.progress.error.is_some() {
            Scanner::Done
        } else if format == Format::Bam {
            Scanner::Bam(bam::BamScanner::new(header_done))
        } else {
            Scanner::Text(text::TextScanner::new(format))
        };
        reader.seek(SeekFrom::Start(position))?;
        let dataset = Self {
            source_path: canonical,
            source_identity,
            reader,
            index,
            scanner,
            position,
            _cache_lock: cache_lock,
            private_cache: private_cache.then_some(directory),
        };
        dataset.check_source()?;
        Ok(dataset)
    }

    /// Return the format detected from decoded input bytes.
    pub fn format(&self) -> Format {
        self.index.format
    }
    /// Return the source compression label used by the interface.
    pub fn compression(&self) -> &str {
        self.reader.get_ref().compression()
    }
    /// Clone the most recently committed scan progress.
    pub fn progress(&self) -> Progress {
        self.index.progress.clone()
    }

    /// Performs bounded scan work; background workers can preempt between calls.
    pub fn step(&mut self, byte_budget: usize) -> io::Result<Progress> {
        self.check_source()?;
        if matches!(self.scanner, Scanner::Done) {
            return Ok(self.progress());
        }
        self.index.begin_step()?;
        let mut input = ScanInput::new(&mut self.reader, &mut self.position);
        let outcome = match &mut self.scanner {
            Scanner::Text(scanner) => scanner.step(&mut input, &mut self.index, byte_budget.max(1)),
            Scanner::Bam(scanner) => scanner.step(&mut input, &mut self.index, byte_budget.max(1)),
            Scanner::Done => Ok(true),
        }
        .and_then(|done| {
            self.check_source()?;
            Ok(done)
        });
        if matches!(outcome, Ok(true))
            && let Err(error) = self.reader.get_mut().finish_index()
        {
            self.index.rollback_step();
            return Err(error);
        }
        if self.index.finish_step(outcome, self.position)? {
            self.scanner = Scanner::Done;
        }
        Ok(self.progress())
    }

    /// Return at most `limit` summaries with zero-based IDs at or beyond `offset`.
    pub fn records(&self, offset: u64, limit: usize) -> io::Result<Vec<RecordSummary>> {
        self.check_source()?;
        self.index.records(offset, limit)
    }
    /// Read indexed metadata for a zero-based record ID, or `None` if absent.
    pub fn record(&self, id: u64) -> io::Result<Option<RecordMeta>> {
        self.check_source()?;
        self.index.record(id)
    }
    /// Search indexed names inclusively from `after`; callers decide when to wrap.
    pub fn find_name(&self, query: &str, after: u64) -> io::Result<Option<u64>> {
        self.check_source()?;
        self.index.find_name(query, after)
    }
    /// Search names from `after` inclusively, interrupting SQLite when cancellation is set.
    pub fn find_name_cancellable(
        &self,
        query: &str,
        after: u64,
        cancel: Arc<AtomicBool>,
    ) -> io::Result<Option<u64>> {
        self.check_source()?;
        self.index.find_name_cancellable(query, after, cancel)
    }
    /// Aggregate completed sequence-bearing records and verify the source before and after the query.
    pub fn stats(&self) -> io::Result<Stats> {
        self.check_source()?;
        let stats = self.index.stats()?;
        self.check_source()?;
        Ok(stats)
    }
    /// Compute aggregate statistics with SQLite cancellation and source-identity checks.
    pub fn stats_cancellable(&self, cancel: Arc<AtomicBool>) -> io::Result<Stats> {
        self.check_source()?;
        let stats = self.index.stats_cancellable(cancel)?;
        self.check_source()?;
        Ok(stats)
    }
    /// Read indexed source spans overlapping this zero-based sequence interval.
    pub fn read_range(&mut self, id: u64, start: u64, len: usize) -> io::Result<SequenceWindow> {
        self.check_source()?;
        let result = (|| {
            let mut result = SequenceWindow {
                start,
                ..Default::default()
            };
            if len > 0 {
                result.sequence = self.range_kind(id, start, len, SpanKind::Sequence)?;
                result.quality = self.range_kind(id, start, len, SpanKind::Quality)?;
            }
            self.check_source()?;
            Ok(result)
        })();
        let restored = self.reader.seek(SeekFrom::Start(self.position));
        result.and_then(|value| restored.map(|_| value))
    }

    /// Decode indexed spans for one sequence or quality interval without restoring the scan cursor.
    fn range_kind(
        &mut self,
        id: u64,
        start: u64,
        len: usize,
        kind: SpanKind,
    ) -> io::Result<Vec<u8>> {
        let end = start.saturating_add(len as u64);
        let reader = &mut self.reader;
        let mut output = Vec::with_capacity(len.min(CHUNK));
        self.index.visit_spans(id, start, len, kind, |span| {
            reader.seek(SeekFrom::Start(span.offset))?;
            let mut bytes = vec![0; span.raw_len as usize];
            reader.read_exact(&mut bytes)?;
            let decoded: Vec<u8> = match span.encoding {
                SpanEncoding::BamSequence => bytes
                    .into_iter()
                    .flat_map(|b| [BAM_BASES[(b >> 4) as usize], BAM_BASES[(b & 15) as usize]])
                    .take(span.len as usize)
                    .collect(),
                SpanEncoding::BamQuality => {
                    bytes.into_iter().map(|q| q.saturating_add(33)).collect()
                }
                SpanEncoding::Text => bytes
                    .into_iter()
                    .filter(|b| !b.is_ascii_whitespace())
                    .collect(),
            };
            let from = start.saturating_sub(span.start) as usize;
            let to = (end - span.start).min(span.len) as usize;
            if from < to && to <= decoded.len() {
                output.extend_from_slice(&decoded[from..to]);
            }
            Ok(())
        })?;
        Ok(output)
    }
    /// Explicit copying may allocate the complete record; scanning and browsing do not.
    pub fn copy_record(&mut self, id: u64, cancel: &AtomicBool) -> io::Result<String> {
        self.check_source()?;
        let result = self.copy_record_inner(id, cancel).and_then(|value| {
            self.check_source()?;
            Ok(value)
        });
        let restored = self.reader.seek(SeekFrom::Start(self.position));
        result.and_then(|value| restored.map(|_| value))
    }

    /// Reconstruct a complete FASTA or FASTQ record, checking cancellation between chunks.
    /// FASTA sequence lines wrap at 60 characters; FASTQ preserves its separator text.
    fn copy_record_inner(&mut self, id: u64, cancel: &AtomicBool) -> io::Result<String> {
        if !self.format().can_copy() {
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
            if self.format() == Format::Fasta {
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
            let data = self.range_kind(id, pos, CHUNK, SpanKind::Sequence)?;
            if data.is_empty() {
                return Err(invalid("Incomplete sequence index"));
            }
            for (i, b) in data.iter().enumerate() {
                output.push(char::from(*b));
                if self.format() == Format::Fasta && (pos + i as u64 + 1).is_multiple_of(60) {
                    output.push('\n');
                }
            }
            pos += data.len() as u64;
        }
        if !output.ends_with('\n') || meta.length == 0 {
            output.push('\n');
        }
        if self.format() == Format::Fastq {
            output.push_str(&meta.plus);
            output.push('\n');
            pos = 0;
            while pos < meta.quality_length {
                if cancel.load(Ordering::Relaxed) {
                    return Err(io::Error::new(io::ErrorKind::Interrupted, "Copy cancelled"));
                }
                let data = self.range_kind(id, pos, CHUNK, SpanKind::Quality)?;
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

    /// Reject input that no longer has the file identity captured when the dataset opened.
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
}

impl Drop for Dataset {
    /// Remove the temporary cache allocated when another dataset owns the reusable cache.
    fn drop(&mut self) {
        // Private concurrent-reader caches do not replace the reusable writer index.
        // macOS/Linux permit unlinking these files while their final handles close.
        if let Some(directory) = &self.private_cache {
            let _ = fs::remove_dir_all(directory);
        }
    }
}

/// Recognize supplied decoded bytes without opening input or creating caches.
pub fn probe_format(prefix: &[u8], path: &std::path::Path) -> std::io::Result<Format> {
    classify::detect_format(prefix, path)
}
