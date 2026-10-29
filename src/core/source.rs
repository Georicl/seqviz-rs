//! Seekable input with on-disk gzip/BGZF restart points.
//!
//! A restart point stores the DEFLATE bit position, its 32 KiB history, and the
//! current member's checksum. Only one point is loaded at a time. Opening a
//! source reads its header; the remainder is indexed as it is read or sought.
use crc32fast::Hasher;
use libz_sys as z;
use std::fs::{self, File, OpenOptions};
use std::io::{self, BufRead, BufReader, Read, Seek, SeekFrom, Write};
use std::os::fd::AsRawFd;
use std::os::unix::ffi::OsStrExt;
use std::os::unix::fs::MetadataExt;
use std::path::Path;
use std::ptr;

/// Maximum DEFLATE history in bytes retained for a restart dictionary.
const WINDOW: usize = 32_768;
/// Compressed input buffer size and maximum output slice per inflate call.
const INPUT_BUFFER: usize = 65_536;
/// Minimum decoded-byte spacing between ordinary DEFLATE checkpoints.
const CHECKPOINT_SPAN: u64 = 1_048_576;
/// Byte length of the persisted source-index header.
const INDEX_HEADER: u64 = 96;
/// Byte length of one fixed-width checkpoint table row.
const POINT_SIZE: u64 = 64;
/// Signature and format version of the source checkpoint index.
const MAGIC: &[u8; 8] = b"SQGZIDX1";

/// All offsets are in the uncompressed byte stream, including for BAM/BGZF.
pub struct Source {
    /// Selected plain-file or compressed-stream implementation.
    inner: Input,
}

/// Either direct file I/O or a gzip/BGZF decoder with persistent restart points.
enum Input {
    /// Uncompressed file read directly without a source index.
    Plain(File),
    /// Gzip or BGZF stream with decoder state and restart cache.
    Compressed(Box<Compressed>),
}

impl Source {
    /// Detect compression from magic bytes and initialise lazy decoding without scanning the file.
    pub fn open(path: &Path, cache_dir: &Path) -> io::Result<Self> {
        let mut file = File::open(path)?;
        let mut signature = [0; 2];
        let n = file.read(&mut signature)?;
        file.rewind()?;
        if n != 2 || signature != [0x1f, 0x8b] {
            return Ok(Self {
                inner: Input::Plain(file),
            });
        }
        let identity = identity(&file)?;
        let mut input = BufReader::with_capacity(INPUT_BUFFER, file);
        let header = member_header(&mut input)?.ok_or_else(|| invalid("Missing gzip header"))?;
        let bgzf = header.end.is_some();
        let canonical = fs::canonicalize(path)?;
        let mut index = Index::open(cache_dir, &canonical, identity, bgzf)?;
        let first = Point {
            output: 0,
            input: input.stream_position()?,
            member_output: 0,
            member_end: header.end,
            crc: 0,
            bits: 0,
            window: Vec::new(),
        };
        index.append(&first)?;
        Ok(Self {
            inner: Input::Compressed(Box::new(Compressed {
                input,
                inflate: Inflater::new()?,
                index,
                position: 0,
                member_output: 0,
                member_end: header.end,
                checksum: Hasher::new(),
                window: Vec::with_capacity(WINDOW),
                trailer_pending: false,
                eof: false,
                bgzf,
                identity,
            })),
        })
    }

    /// Return the detected compression label: `none`, `gzip` or `BGZF`.
    pub fn compression(&self) -> &str {
        match &self.inner {
            Input::Plain(_) => "none",
            Input::Compressed(source) if source.bgzf => "BGZF",
            Input::Compressed(_) => "gzip",
        }
    }

    /// Complete and sync the index, decoding any suffix not covered by cached checkpoints.
    /// Restore the logical position on success. Call this on the background reader,
    /// since completing an unknown suffix requires I/O.
    pub fn finish_index(&mut self) -> io::Result<()> {
        if let Input::Compressed(source) = &mut self.inner {
            source.check_identity()?;
            let saved = source.position;
            source.scan_to_end()?;
            source.check_identity()?;
            source.index.sync()?;
            source.seek(SeekFrom::Start(saved))?;
        }
        Ok(())
    }
}

impl Read for Source {
    /// Read logical uncompressed bytes through the selected input backend.
    fn read(&mut self, buffer: &mut [u8]) -> io::Result<usize> {
        match &mut self.inner {
            Input::Plain(file) => file.read(buffer),
            Input::Compressed(source) => source.read(buffer),
        }
    }
}

impl Seek for Source {
    /// Seek in logical uncompressed byte coordinates, delegating restart handling when compressed.
    fn seek(&mut self, position: SeekFrom) -> io::Result<u64> {
        match &mut self.inner {
            Input::Plain(file) => file.seek(position),
            Input::Compressed(source) => source.seek(position),
        }
    }
}

/// Create an invalid-data error for malformed compressed input or cache metadata.
fn invalid(message: impl Into<String>) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, message.into())
}

/// Report a compressed stream ending before its required data or trailer.
fn unexpected_eof() -> io::Error {
    io::Error::new(io::ErrorKind::UnexpectedEof, "Truncated gzip stream")
}

// File size, inode and nanosecond timestamps invalidate reused indexes without
// scanning the entire input. ctime also detects equal-size rewrites whose mtime
// has deliberately been restored.
/// Read filesystem metadata used to reject stale compressed-source checkpoints.
fn identity(file: &File) -> io::Result<[u64; 7]> {
    let m = file.metadata()?;
    Ok([
        m.dev(),
        m.ino(),
        m.len(),
        m.mtime() as u64,
        m.mtime_nsec() as u64,
        m.ctime() as u64,
        m.ctime_nsec() as u64,
    ])
}

/// Parsed gzip member metadata needed to enforce an optional BGZF block boundary.
struct MemberHeader {
    /// Exclusive compressed-file end offset declared by a BGZF BC field, if present.
    end: Option<u64>,
}

/// Parse optional gzip fields incrementally; names/comments are never retained.
fn member_header(input: &mut BufReader<File>) -> io::Result<Option<MemberHeader>> {
    if input.fill_buf()?.is_empty() {
        return Ok(None);
    }
    let start = input.stream_position()?;
    let mut bytes = [0; 10];
    input.read_exact(&mut bytes)?;
    if bytes[..3] != [0x1f, 0x8b, 8] || bytes[3] & 0xe0 != 0 {
        return Err(invalid("Invalid gzip member header"));
    }
    let flags = bytes[3];
    let mut checksum = Hasher::new();
    checksum.update(&bytes);
    let mut end = None;
    if flags & 4 != 0 {
        let mut size = [0; 2];
        input.read_exact(&mut size)?;
        checksum.update(&size);
        let mut remaining = usize::from(u16::from_le_bytes(size));
        while remaining != 0 {
            if remaining < 4 {
                return Err(invalid("Malformed gzip extra field"));
            }
            let mut field = [0; 4];
            input.read_exact(&mut field)?;
            checksum.update(&field);
            remaining -= 4;
            let length = usize::from(u16::from_le_bytes([field[2], field[3]]));
            if length > remaining {
                return Err(invalid("Truncated gzip extra field"));
            }
            let mut data = [0; 1024];
            let mut left = length;
            if &field[..2] == b"BC" {
                if length != 2 {
                    return Err(invalid("Invalid BGZF block-size field"));
                }
                input.read_exact(&mut data[..2])?;
                checksum.update(&data[..2]);
                let size = u64::from(u16::from_le_bytes([data[0], data[1]])) + 1;
                if size < 26 {
                    return Err(invalid("Invalid BGZF block size"));
                }
                end = Some(start + size);
                left = 0;
            }
            while left != 0 {
                let n = left.min(data.len());
                input.read_exact(&mut data[..n])?;
                checksum.update(&data[..n]);
                left -= n;
            }
            remaining -= length;
        }
    }
    for flag in [8, 16] {
        if flags & flag != 0 {
            loop {
                let mut byte = [0];
                input.read_exact(&mut byte)?;
                checksum.update(&byte);
                if byte[0] == 0 {
                    break;
                }
            }
        }
    }
    if flags & 2 != 0 {
        let mut expected = [0; 2];
        input.read_exact(&mut expected)?;
        if u16::from_le_bytes(expected) != checksum.finalize() as u16 {
            return Err(invalid("Gzip header checksum mismatch"));
        }
    }
    if let Some(end) = end
        && input.stream_position()? + 8 > end
    {
        return Err(invalid("BGZF header exceeds its block size"));
    }
    Ok(Some(MemberHeader { end }))
}

/// Own the pinned z_stream, since zlib retains its address internally. No input
/// or output pointer survives a call to step(), and the type is intentionally
/// not Send/Sync: a worker constructs and uses its own source.
struct Inflater {
    /// Stable heap allocation whose address remains valid for zlib's lifetime.
    stream: Box<z::z_stream>,
}

/// Allocate zeroed storage for zlib through the C allocator.
unsafe extern "C" fn zalloc(_: z::voidpf, count: z::uInt, size: z::uInt) -> z::voidpf {
    // SAFETY: calloc returns suitably aligned zeroed storage or a null pointer;
    // zlib checks allocation failure and pairs this with zfree below.
    unsafe { libc::calloc(count as usize, size as usize) }
}

/// Release storage previously allocated by the zlib allocation callback.
unsafe extern "C" fn zfree(_: z::voidpf, address: z::voidpf) {
    // SAFETY: zlib only passes allocations obtained from zalloc (or null).
    unsafe { libc::free(address) }
}

/// Progress and restart-boundary information from one raw DEFLATE decode step.
struct Step {
    /// Compressed bytes consumed from the supplied input slice.
    consumed: usize,
    /// Decoded bytes written into the supplied output slice.
    produced: usize,
    /// Whether the raw DEFLATE stream reached its end.
    ended: bool,
    /// Whether decoding stopped at a reusable non-final block boundary.
    boundary: bool,
    /// Number of unconsumed bits in the last consumed input byte.
    bits: u8,
}

impl Inflater {
    /// Initialise a pinned raw-DEFLATE zlib stream with matching allocation callbacks.
    fn new() -> io::Result<Self> {
        let mut stream = Box::new(z::z_stream {
            next_in: ptr::null_mut(),
            avail_in: 0,
            total_in: 0,
            next_out: ptr::null_mut(),
            avail_out: 0,
            total_out: 0,
            msg: ptr::null_mut(),
            state: ptr::null_mut(),
            zalloc,
            zfree,
            opaque: ptr::null_mut(),
            data_type: 0,
            adler: 0,
            reserved: 0,
        });
        // SAFETY: every field is initialized, the Box address remains stable,
        // and the version/size are from the same linked zlib binding.
        let result = unsafe {
            z::inflateInit2_(
                &mut *stream,
                -15,
                z::zlibVersion(),
                std::mem::size_of::<z::z_stream>() as i32,
            )
        };
        if result != z::Z_OK {
            return Err(io::Error::other(format!(
                "Cannot initialize zlib ({result})"
            )));
        }
        Ok(Self { stream })
    }

    /// Reset zlib, prime any unconsumed bits and restore the preceding history dictionary.
    fn restore(&mut self, bits: u8, byte: u8, dictionary: &[u8]) -> io::Result<()> {
        // SAFETY: the stream is initialized and dictionary remains live during
        // the call; zlib copies it into its own sliding window.
        let reset = unsafe { z::inflateReset(&mut *self.stream) };
        if reset != z::Z_OK {
            return Err(invalid("Cannot reset zlib stream"));
        }
        if bits != 0 {
            let result = unsafe {
                z::inflatePrime(
                    &mut *self.stream,
                    i32::from(bits),
                    i32::from(byte >> (8 - bits)),
                )
            };
            if result != z::Z_OK {
                return Err(invalid("Invalid DEFLATE restart bits"));
            }
        }
        if !dictionary.is_empty() {
            let result = unsafe {
                z::inflateSetDictionary(
                    &mut *self.stream,
                    dictionary.as_ptr(),
                    dictionary.len() as u32,
                )
            };
            if result != z::Z_OK {
                return Err(invalid("Invalid DEFLATE restart dictionary"));
            }
        }
        Ok(())
    }

    /// Decode to the next block boundary or buffer limit and detach borrowed slice pointers.
    fn step(&mut self, input: &[u8], output: &mut [u8]) -> io::Result<Step> {
        self.stream.next_in = input.as_ptr().cast_mut();
        self.stream.avail_in = input.len() as u32;
        self.stream.next_out = output.as_mut_ptr();
        self.stream.avail_out = output.len() as u32;
        // SAFETY: zlib only reads the input and writes within the supplied output
        // bounds, neither slice is moved or otherwise accessed during the call.
        let result = unsafe { z::inflate(&mut *self.stream, z::Z_BLOCK) };
        let step = Step {
            consumed: input.len() - self.stream.avail_in as usize,
            produced: output.len() - self.stream.avail_out as usize,
            ended: result == z::Z_STREAM_END,
            boundary: self.stream.data_type & 0xc0 == 0x80,
            bits: (self.stream.data_type & 7) as u8,
        };
        self.stream.next_in = ptr::null_mut();
        self.stream.next_out = ptr::null_mut();
        self.stream.avail_in = 0;
        self.stream.avail_out = 0;
        match result {
            z::Z_OK | z::Z_STREAM_END | z::Z_BUF_ERROR => Ok(step),
            _ => Err(invalid(format!("Invalid DEFLATE data (zlib {result})"))),
        }
    }
}

impl Drop for Inflater {
    /// Release zlib state before freeing the pinned stream storage.
    fn drop(&mut self) {
        // SAFETY: successful initialization is paired with exactly one end call.
        unsafe {
            z::inflateEnd(&mut *self.stream);
        }
    }
}

/// Incremental gzip/BGZF reader with member validation and disk-backed seeking.
struct Compressed {
    /// Buffered compressed-file reader.
    input: BufReader<File>,
    /// Raw DEFLATE decoder for the current gzip member.
    inflate: Inflater,
    /// Disk-backed restart table and dictionaries.
    index: Index,
    /// Current logical offset in the uncompressed stream.
    position: u64,
    /// Uncompressed bytes produced in the current member.
    member_output: u64,
    /// Expected exclusive compressed offset of the current BGZF member.
    member_end: Option<u64>,
    /// Running CRC32 of the current member's uncompressed bytes.
    checksum: Hasher,
    /// Most recent bytes in this member, capped at the DEFLATE history size.
    window: Vec<u8>,
    /// Whether the next read must validate the completed member trailer.
    trailer_pending: bool,
    /// Whether reads are at or beyond the known end of the decoded stream.
    eof: bool,
    /// Whether the initial header identified this source as BGZF.
    bgzf: bool,
    /// File metadata captured at open for detecting stale source data.
    identity: [u64; 7],
}

impl Compressed {
    /// Reject operations when the open file's current metadata differs from its initial identity.
    fn check_identity(&self) -> io::Result<()> {
        if identity(self.input.get_ref())? != self.identity {
            return Err(invalid(
                "Source file changed; reopen it to rebuild the index",
            ));
        }
        Ok(())
    }

    /// Advance logical offsets, update the member CRC and retain at most 32 KiB of history.
    fn remember(&mut self, bytes: &[u8]) {
        self.checksum.update(bytes);
        self.member_output += bytes.len() as u64;
        self.position += bytes.len() as u64;
        if bytes.len() >= WINDOW {
            self.window.clear();
            self.window
                .extend_from_slice(&bytes[bytes.len() - WINDOW..]);
        } else {
            let excess = (self.window.len() + bytes.len()).saturating_sub(WINDOW);
            if excess != 0 {
                self.window.copy_within(excess.., 0);
                self.window.truncate(self.window.len() - excess);
            }
            self.window.extend_from_slice(bytes);
        }
    }

    /// Persist a new member start or a sufficiently distant DEFLATE block boundary.
    fn checkpoint(&mut self, bits: u8, member_start: bool) -> io::Result<()> {
        let input = self.input.stream_position()?;
        if input <= self.index.last_input
            || (!member_start
                && self.position.saturating_sub(self.index.last_output) < CHECKPOINT_SPAN)
        {
            return Ok(());
        }
        self.index.append(&Point {
            output: self.position,
            input,
            member_output: self.member_output,
            member_end: self.member_end,
            crc: self.checksum.clone().finalize(),
            bits,
            window: self.window.clone(),
        })
    }

    /// Validate the trailer, then initialise the next member or mark the index complete.
    fn next_member(&mut self) -> io::Result<()> {
        let mut trailer = [0; 8];
        self.input.read_exact(&mut trailer)?;
        let crc = u32::from_le_bytes(trailer[..4].try_into().unwrap());
        let size = u32::from_le_bytes(trailer[4..].try_into().unwrap());
        if crc != self.checksum.clone().finalize() {
            return Err(invalid("Gzip data checksum mismatch"));
        }
        if size != self.member_output as u32 {
            return Err(invalid("Gzip uncompressed-size mismatch"));
        }
        if let Some(end) = self.member_end
            && (self.input.stream_position()? != end || self.member_output > 65_536)
        {
            return Err(invalid("BGZF block size mismatch"));
        }
        self.trailer_pending = false;
        match member_header(&mut self.input)? {
            None => {
                self.eof = true;
                self.index.complete(self.position)?;
            }
            Some(header) => {
                if self.bgzf && header.end.is_none() {
                    return Err(invalid("Missing BGZF block-size field"));
                }
                self.member_end = header.end;
                self.member_output = 0;
                self.checksum = Hasher::new();
                self.window.clear();
                self.inflate.restore(0, 0, &[])?;
                self.checkpoint(0, true)?;
            }
        }
        Ok(())
    }

    /// Reposition compressed input and reconstruct decoder, checksum and member state from a point.
    fn restore(&mut self, point: Point) -> io::Result<()> {
        let offset = point
            .input
            .checked_sub(u64::from(point.bits != 0))
            .ok_or_else(|| invalid("Invalid checkpoint offset"))?;
        self.input.seek(SeekFrom::Start(offset))?;
        let mut byte = [0];
        if point.bits != 0 {
            self.input.read_exact(&mut byte)?;
        }
        self.inflate.restore(point.bits, byte[0], &point.window)?;
        self.position = point.output;
        self.member_output = point.member_output;
        self.member_end = point.member_end;
        self.checksum = Hasher::new_with_initial(point.crc);
        self.window = point.window;
        self.trailer_pending = false;
        self.eof = false;
        Ok(())
    }

    /// Decode and discard bytes up to a logical offset, allowing positions beyond EOF.
    fn skip_to(&mut self, target: u64) -> io::Result<()> {
        let mut discard = [0; WINDOW];
        while self.position < target && !self.eof {
            let count = (target - self.position).min(discard.len() as u64) as usize;
            if self.read(&mut discard[..count])? == 0 {
                break;
            }
        }
        // Seek permits positions beyond EOF, matching ordinary File behavior.
        if self.eof && self.position < target {
            self.position = target;
        }
        Ok(())
    }

    /// Determine decoded length from the furthest useful checkpoint when it is still unknown.
    fn scan_to_end(&mut self) -> io::Result<()> {
        if self.index.length.is_some() {
            return Ok(());
        }
        if self.index.count != 0 {
            let last = self.index.read_point(self.index.count - 1)?;
            if last.output > self.position {
                self.restore(last)?;
            }
        }
        let mut discard = [0; WINDOW];
        while self.read(&mut discard)? != 0 {}
        Ok(())
    }
}

impl Read for Compressed {
    /// Decode a bounded chunk, creating restart points and validating completed member trailers.
    fn read(&mut self, buffer: &mut [u8]) -> io::Result<usize> {
        if buffer.is_empty() || self.eof {
            return Ok(0);
        }
        let limit = buffer.len().min(INPUT_BUFFER);
        loop {
            if self.trailer_pending {
                self.next_member()?;
                if self.eof {
                    return Ok(0);
                }
            }
            let input = self.input.fill_buf()?;
            let input_empty = input.is_empty();
            let step = self.inflate.step(input, &mut buffer[..limit])?;
            self.input.consume(step.consumed);
            self.remember(&buffer[..step.produced]);
            if step.boundary {
                self.checkpoint(step.bits, false)?;
            }
            if step.ended {
                self.trailer_pending = true;
            }
            if step.produced != 0 {
                return Ok(step.produced);
            }
            if step.consumed == 0 && !step.ended && (input_empty || !step.boundary) {
                return Err(if input_empty {
                    unexpected_eof()
                } else {
                    invalid("DEFLATE decoder made no progress")
                });
            }
        }
    }
}

impl Seek for Compressed {
    /// Resolve a logical target, restore a preceding checkpoint when needed and decode up to it.
    fn seek(&mut self, from: SeekFrom) -> io::Result<u64> {
        self.check_identity()?;
        let target = match from {
            SeekFrom::Start(target) => target,
            SeekFrom::Current(delta) => {
                self.position.checked_add_signed(delta).ok_or_else(|| {
                    io::Error::new(io::ErrorKind::InvalidInput, "Negative or overflowing seek")
                })?
            }
            SeekFrom::End(delta) => {
                self.scan_to_end()?;
                self.index
                    .length
                    .unwrap_or(self.position)
                    .checked_add_signed(delta)
                    .ok_or_else(|| {
                        io::Error::new(io::ErrorKind::InvalidInput, "Negative or overflowing seek")
                    })?
            }
        };
        if target == self.position {
            return Ok(target);
        }
        if let Some(length) = self.index.length
            && target >= length
        {
            self.position = target;
            self.eof = true;
            return Ok(target);
        }
        let point = self.index.find(target)?;
        if target < self.position || self.eof || point.output > self.position {
            self.restore(point)?;
        }
        self.skip_to(target)?;
        Ok(self.position)
    }
}

/// Decoder restart state pairing compressed offsets with uncompressed stream progress.
struct Point {
    /// Logical uncompressed byte offset at this restart point.
    output: u64,
    /// Compressed offset after the last consumed byte; subtract one when `bits` is nonzero.
    input: u64,
    /// Decoded byte count within the current gzip member.
    member_output: u64,
    /// Exclusive compressed end of the BGZF member, when present.
    member_end: Option<u64>,
    /// CRC32 of member bytes decoded before this point.
    crc: u32,
    /// Unconsumed high bits in the byte immediately before `input`.
    bits: u8,
    /// DEFLATE dictionary preceding this point; empty at a member start.
    window: Vec<u8>,
}

/// The point table has fixed 64-byte rows. Dictionaries live separately so a
/// BGZF block restart (empty dictionary) needs only its table row, not 32 KiB.
struct Index {
    /// Checkpoint table file, also holding the exclusive writer lock when acquired.
    table: File,
    /// Separate file containing variable-length DEFLATE dictionaries.
    windows: File,
    /// Whether this reader owns the lock needed to append or rebuild the cache.
    writable: bool,
    /// Number of committed checkpoint rows visible to this instance.
    count: u64,
    /// Known uncompressed stream length after successful completion.
    length: Option<u64>,
    /// Compressed input offset of the latest committed checkpoint.
    last_input: u64,
    /// Uncompressed output offset of the latest committed checkpoint.
    last_output: u64,
}

impl Index {
    /// Open the path-keyed index, reuse matching metadata or rebuild it under an exclusive writer lock.
    fn open(directory: &Path, path: &Path, identity: [u64; 7], bgzf: bool) -> io::Result<Self> {
        fs::create_dir_all(directory)?;
        let hash = path
            .as_os_str()
            .as_bytes()
            .iter()
            .fold(0xcbf29ce484222325u64, |hash, byte| {
                (hash ^ u64::from(*byte)).wrapping_mul(0x100000001b3)
            });
        let name = format!("source-{hash:016x}");
        let table = OpenOptions::new()
            .create(true)
            .truncate(false)
            .read(true)
            .write(true)
            .open(directory.join(format!("{name}.sqi")))?;
        // An already-open tab may own the writer. Other readers can reuse its
        // committed rows, but do not race to append or truncate the same files.
        let writable =
            unsafe { libc::flock(table.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) } == 0;
        let windows = OpenOptions::new()
            .create(true)
            .truncate(false)
            .read(true)
            .write(true)
            .open(directory.join(format!("{name}.sqw")))?;
        let mut index = Self {
            table,
            windows,
            writable,
            count: 0,
            length: None,
            last_input: 0,
            last_output: 0,
        };
        let valid = index.load(&identity, bgzf).unwrap_or(false);
        if !valid {
            if !writable {
                return Err(io::Error::new(
                    io::ErrorKind::WouldBlock,
                    "Source cache is being rebuilt by another reader",
                ));
            }
            index.count = 0;
            index.length = None;
            index.last_input = 0;
            index.last_output = 0;
            index.table.set_len(0)?;
            index.windows.set_len(0)?;
            let mut header = [0; INDEX_HEADER as usize];
            header[..8].copy_from_slice(MAGIC);
            for (i, value) in identity.iter().enumerate() {
                header[8 + i * 8..16 + i * 8].copy_from_slice(&value.to_le_bytes());
            }
            header[72..80].copy_from_slice(&u64::MAX.to_le_bytes());
            header[80] = u8::from(bgzf);
            index.table.rewind()?;
            index.table.write_all(&header)?;
        }
        Ok(index)
    }

    /// Validate index header, source identity and the last committed point before reuse.
    fn load(&mut self, identity: &[u64; 7], bgzf: bool) -> io::Result<bool> {
        let mut header = [0; INDEX_HEADER as usize];
        self.table.rewind()?;
        if self.table.read_exact(&mut header).is_err()
            || &header[..8] != MAGIC
            || header[80] != u8::from(bgzf)
        {
            return Ok(false);
        }
        for (i, value) in identity.iter().enumerate() {
            if header[8 + i * 8..16 + i * 8] != value.to_le_bytes() {
                return Ok(false);
            }
        }
        let count = u64::from_le_bytes(header[64..72].try_into().unwrap());
        let length = u64::from_le_bytes(header[72..80].try_into().unwrap());
        if count == 0
            || count > self.table.metadata()?.len().saturating_sub(INDEX_HEADER) / POINT_SIZE
        {
            return Ok(false);
        }
        self.count = count;
        self.length = (length != u64::MAX).then_some(length);
        let last = self.read_point(count - 1)?;
        self.last_input = last.input;
        self.last_output = last.output;
        Ok(true)
    }

    /// Write a new checkpoint dictionary and row, then publish its count if this reader owns the lock.
    fn append(&mut self, point: &Point) -> io::Result<()> {
        if !self.writable || (self.count != 0 && point.input <= self.last_input) {
            return Ok(());
        }
        let offset = self.windows.seek(SeekFrom::End(0))?;
        self.windows.write_all(&point.window)?;
        let mut row = [0; POINT_SIZE as usize];
        for (offset, value) in [
            (0, point.output),
            (8, point.input),
            (16, point.member_output),
            (24, offset),
            (32, point.member_end.unwrap_or(u64::MAX)),
        ] {
            row[offset..offset + 8].copy_from_slice(&value.to_le_bytes());
        }
        row[40..44].copy_from_slice(&point.crc.to_le_bytes());
        row[44..48].copy_from_slice(&(point.window.len() as u32).to_le_bytes());
        row[48] = point.bits;
        let mut checksum = Hasher::new();
        checksum.update(&row[..60]);
        checksum.update(&point.window);
        row[60..64].copy_from_slice(&checksum.finalize().to_le_bytes());
        self.table
            .seek(SeekFrom::Start(INDEX_HEADER + self.count * POINT_SIZE))?;
        self.table.write_all(&row)?;
        self.count += 1;
        self.last_input = point.input;
        self.last_output = point.output;
        self.table.seek(SeekFrom::Start(64))?;
        self.table.write_all(&self.count.to_le_bytes())?;
        Ok(())
    }

    /// Load one checkpoint and dictionary, validating size, bit count and checksum.
    fn read_point(&mut self, number: u64) -> io::Result<Point> {
        let mut row = [0; POINT_SIZE as usize];
        self.table
            .seek(SeekFrom::Start(INDEX_HEADER + number * POINT_SIZE))?;
        self.table.read_exact(&mut row)?;
        let get = |offset| u64::from_le_bytes(row[offset..offset + 8].try_into().unwrap());
        let size = u32::from_le_bytes(row[44..48].try_into().unwrap()) as usize;
        if size > WINDOW || row[48] > 7 {
            return Err(invalid("Invalid gzip checkpoint"));
        }
        let mut window = vec![0; size];
        self.windows.seek(SeekFrom::Start(get(24)))?;
        self.windows.read_exact(&mut window)?;
        let mut checksum = Hasher::new();
        checksum.update(&row[..60]);
        checksum.update(&window);
        if checksum.finalize() != u32::from_le_bytes(row[60..64].try_into().unwrap()) {
            return Err(invalid("Gzip checkpoint checksum mismatch"));
        }
        Ok(Point {
            output: get(0),
            input: get(8),
            member_output: get(16),
            member_end: (get(32) != u64::MAX).then(|| get(32)),
            crc: u32::from_le_bytes(row[40..44].try_into().unwrap()),
            bits: row[48],
            window,
        })
    }

    /// Binary-search the last checkpoint at or before a logical byte offset.
    fn find(&mut self, target: u64) -> io::Result<Point> {
        let mut low = 0;
        let mut high = self.count;
        while low < high {
            let mid = low + (high - low) / 2;
            self.table
                .seek(SeekFrom::Start(INDEX_HEADER + mid * POINT_SIZE))?;
            let mut offset = [0; 8];
            self.table.read_exact(&mut offset)?;
            if u64::from_le_bytes(offset) <= target {
                low = mid + 1;
            } else {
                high = mid;
            }
        }
        self.read_point(low.saturating_sub(1))
    }

    /// Record the decoded stream length in memory and in the index when writable.
    fn complete(&mut self, length: u64) -> io::Result<()> {
        self.length = Some(length);
        if self.writable {
            self.table.seek(SeekFrom::Start(72))?;
            self.table.write_all(&length.to_le_bytes())?;
        }
        Ok(())
    }

    /// Flush checkpoint dictionaries before the table when this reader owns the writer lock.
    fn sync(&mut self) -> io::Result<()> {
        if self.writable {
            self.windows.sync_data()?;
            self.table.sync_data()?;
        }
        Ok(())
    }
}

/// Compressed-source integrity, random access and checkpoint reuse tests.
#[cfg(test)]
mod tests {
    use super::*;
    use flate2::Compression;
    use flate2::write::{DeflateEncoder, GzEncoder};
    use std::path::PathBuf;
    use std::sync::atomic::{AtomicU64, Ordering};

    /// Unique suffix for temporary source fixtures created in this process.
    static NEXT: AtomicU64 = AtomicU64::new(0);

    /// Temporary input and cache paths removed together after each source test.
    struct Fixture {
        /// Temporary fixture root, removed on drop.
        directory: PathBuf,
        /// Input file containing the supplied fixture bytes.
        path: PathBuf,
        /// Directory where source checkpoints may be created.
        cache: PathBuf,
    }

    impl Fixture {
        /// Write fixture bytes to an isolated temporary directory with a separate cache path.
        fn new(bytes: &[u8]) -> Self {
            let directory = std::env::temp_dir().join(format!(
                "seqviz-source-{}-{}",
                std::process::id(),
                NEXT.fetch_add(1, Ordering::Relaxed)
            ));
            fs::create_dir_all(&directory).unwrap();
            let path = directory.join("input");
            fs::write(&path, bytes).unwrap();
            let cache = directory.join("cache");
            Self {
                directory,
                path,
                cache,
            }
        }
        /// Open the fixture through the same source abstraction used by datasets.
        fn open(&self) -> Source {
            Source::open(&self.path, &self.cache).unwrap()
        }
    }

    impl Drop for Fixture {
        /// Remove the fixture input and generated cache files after the test.
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.directory);
        }
    }

    /// Encode fixture bytes as one gzip member.
    fn gzip(bytes: &[u8]) -> Vec<u8> {
        let mut encoder = GzEncoder::new(Vec::new(), Compression::default());
        encoder.write_all(bytes).unwrap();
        encoder.finish().unwrap()
    }

    /// Build one BGZF member with an explicit block size, CRC and uncompressed length.
    fn bgzf_block(bytes: &[u8]) -> Vec<u8> {
        let mut encoder = DeflateEncoder::new(Vec::new(), Compression::default());
        encoder.write_all(bytes).unwrap();
        let compressed = encoder.finish().unwrap();
        let size = compressed.len() + 26;
        assert!(size <= 65_536);
        let mut result = vec![31, 139, 8, 4, 0, 0, 0, 0, 0, 255, 6, 0, b'B', b'C', 2, 0];
        result.extend_from_slice(&((size - 1) as u16).to_le_bytes());
        result.extend_from_slice(&compressed);
        result.extend_from_slice(&crc32fast::hash(bytes).to_le_bytes());
        result.extend_from_slice(&(bytes.len() as u32).to_le_bytes());
        result
    }

    /// Seek to a logical byte offset and compare the exact range with expected data.
    fn read_at(source: &mut Source, offset: u64, expected: &[u8]) {
        source.seek(SeekFrom::Start(offset)).unwrap();
        let mut actual = vec![0; expected.len()];
        source.read_exact(&mut actual).unwrap();
        assert_eq!(actual, expected);
    }

    /// Generate deterministic xorshift bytes to exercise checkpoint restoration.
    fn pseudo_random(length: usize) -> Vec<u8> {
        let mut state = 0x13579bdfu32;
        (0..length)
            .map(|_| {
                state ^= state << 13;
                state ^= state >> 17;
                state ^= state << 5;
                (state & 255) as u8
            })
            .collect()
    }

    /// Verify plain files use direct seeks and need no compressed-source cache.
    #[test]
    fn plain_input_seeks_without_creating_cache() {
        let fixture = Fixture::new(b">record\nACGT\n");
        let mut source = fixture.open();
        assert_eq!(source.compression(), "none");
        read_at(&mut source, 8, b"ACGT");
        assert_eq!(source.seek(SeekFrom::End(-1)).unwrap(), 12);
        source.finish_index().unwrap();
        assert!(!fixture.cache.exists());
    }

    /// Check lazy opening, backward reads, partial index reuse and position-preserving completion.
    #[test]
    fn gzip_opens_lazily_and_reuses_partial_checkpoints() {
        let data = pseudo_random(3_500_000);
        let fixture = Fixture::new(&gzip(&data));
        {
            let mut source = fixture.open();
            assert_eq!(source.compression(), "gzip");
            let Input::Compressed(inner) = &source.inner else {
                panic!()
            };
            assert_eq!(inner.position, 0);
            assert_eq!(inner.index.count, 1);
            assert!(inner.index.length.is_none());
            assert!(inner.input.get_ref().stream_position().unwrap() <= INPUT_BUFFER as u64);
            read_at(&mut source, 2_200_000, &data[2_200_000..2_200_100]);
            read_at(&mut source, 12, &data[12..76]);
            let Input::Compressed(inner) = &source.inner else {
                panic!()
            };
            assert!(inner.index.count >= 3);
            assert!(inner.index.length.is_none());
        }
        let mut source = fixture.open();
        let Input::Compressed(inner) = &mut source.inner else {
            panic!()
        };
        let point = inner.index.find(2_200_000).unwrap();
        assert!(point.output > 1_000_000);
        assert_eq!(point.window.len(), WINDOW);
        read_at(&mut source, 2_200_000, &data[2_200_000..2_200_200]);
        let saved = source.stream_position().unwrap();
        source.finish_index().unwrap();
        assert_eq!(source.stream_position().unwrap(), saved);
        assert_eq!(source.seek(SeekFrom::End(0)).unwrap(), data.len() as u64);
    }

    /// Exercise reopened checkpoints whose DEFLATE restart positions are not byte-aligned.
    #[test]
    fn gzip_full_index_survives_reopen_and_restores_bit_positions() {
        // Partially compressible blocks exercise non-byte-aligned dynamic trees.
        let mut data = pseudo_random(4_000_000);
        for (i, byte) in data.iter_mut().enumerate() {
            if i % 3 != 0 {
                *byte = b"ACGT"[i % 4];
            }
        }
        let fixture = Fixture::new(&gzip(&data));
        {
            let mut source = fixture.open();
            source.finish_index().unwrap();
            let Input::Compressed(inner) = &mut source.inner else {
                panic!()
            };
            assert!(inner.index.count > 2);
            let mut nonzero_bits = false;
            for n in 1..inner.index.count {
                nonzero_bits |= inner.index.read_point(n).unwrap().bits != 0;
            }
            assert!(
                nonzero_bits,
                "Fixture must test an unaligned DEFLATE checkpoint"
            );
        }
        let mut source = fixture.open();
        for offset in [3_000_000, 1_300_000, 50, 2_400_000, 3_999_900] {
            read_at(
                &mut source,
                offset,
                &data[offset as usize..offset as usize + 100],
            );
        }
        assert_eq!(
            source.seek(SeekFrom::End(-5)).unwrap(),
            data.len() as u64 - 5
        );
        let mut tail = Vec::new();
        source.read_to_end(&mut tail).unwrap();
        assert_eq!(tail, data[data.len() - 5..]);
    }

    /// Read and seek across concatenated members, including an empty member.
    #[test]
    fn reads_concatenated_gzip_members_including_empty_ones() {
        let mut bytes = gzip(b"first");
        bytes.extend(gzip(b""));
        bytes.extend(gzip(b"SECOND"));
        let fixture = Fixture::new(&bytes);
        {
            let mut source = fixture.open();
            let mut decoded = Vec::new();
            source.read_to_end(&mut decoded).unwrap();
            assert_eq!(decoded, b"firstSECOND");
            source.finish_index().unwrap();
            read_at(&mut source, 3, b"stSECO");
        }
        read_at(&mut fixture.open(), 5, b"SECOND");
    }

    /// Verify BGZF block checkpoints need no dictionaries and survive reopening.
    #[test]
    fn bgzf_seeks_by_block_and_keeps_only_disk_index_rows() {
        let data = pseudo_random(200_000);
        let mut bytes = Vec::new();
        for chunk in data.chunks(32_000) {
            bytes.extend(bgzf_block(chunk));
        }
        bytes.extend(bgzf_block(b""));
        let fixture = Fixture::new(&bytes);
        {
            let mut source = fixture.open();
            assert_eq!(source.compression(), "BGZF");
            read_at(&mut source, 63_980, &data[63_980..64_080]);
            source.finish_index().unwrap();
            let Input::Compressed(inner) = &source.inner else {
                panic!()
            };
            assert_eq!(inner.index.count, 8);
            assert_eq!(inner.index.windows.metadata().unwrap().len(), 0);
        }
        let mut source = fixture.open();
        read_at(&mut source, 159_990, &data[159_990..160_020]);
        read_at(&mut source, 2, &data[2..102]);
    }

    /// Rebuild cached checkpoints after the compressed file is replaced.
    #[test]
    fn file_change_invalidates_the_saved_index() {
        let fixture = Fixture::new(&gzip(b"old sequence"));
        {
            let mut source = fixture.open();
            source.finish_index().unwrap();
        }
        fs::write(&fixture.path, gzip(b"new sequence with a different length")).unwrap();
        let mut source = fixture.open();
        let Input::Compressed(inner) = &source.inner else {
            panic!()
        };
        assert_eq!(inner.index.count, 1);
        assert!(inner.index.length.is_none());
        let mut actual = String::new();
        source.read_to_string(&mut actual).unwrap();
        assert_eq!(actual, "new sequence with a different length");
    }

    /// Reject bad gzip trailers and truncated streams without marking the index complete.
    #[test]
    fn checksum_size_and_truncation_errors_are_reported() {
        let original = gzip(&pseudo_random(100_000));
        let mut wrong_crc = original.clone();
        let len = wrong_crc.len();
        wrong_crc[len - 8] ^= 1;
        let mut wrong_size = original.clone();
        wrong_size[len - 4] ^= 1;
        for bytes in [
            wrong_crc,
            wrong_size,
            original[..len - 3].to_vec(),
            original[..len / 2].to_vec(),
        ] {
            let fixture = Fixture::new(&bytes);
            let mut source = fixture.open();
            assert!(source.finish_index().is_err());
            let Input::Compressed(inner) = &source.inner else {
                panic!()
            };
            assert!(inner.index.length.is_none());
        }
    }

    /// Report a malformed later member even when an earlier member decoded successfully.
    #[test]
    fn truncated_later_member_is_not_mistaken_for_eof() {
        let mut bytes = gzip(b"good first member");
        bytes.extend_from_slice(&[31, 139, 8]);
        let fixture = Fixture::new(&bytes);
        let mut source = fixture.open();
        let mut first = [0; 17];
        source.read_exact(&mut first).unwrap();
        assert_eq!(&first, b"good first member");
        assert!(source.read(&mut [0; 1]).is_err());
    }

    /// Check empty reads, seeks beyond EOF, backward recovery and negative-seek rejection.
    #[test]
    fn empty_reads_and_seek_past_eof_follow_read_seek_contracts() {
        let fixture = Fixture::new(&gzip(b"ABC"));
        let mut source = fixture.open();
        assert_eq!(source.read(&mut []).unwrap(), 0);
        assert_eq!(source.stream_position().unwrap(), 0);
        assert_eq!(source.seek(SeekFrom::Start(100)).unwrap(), 100);
        assert_eq!(source.read(&mut [0; 1]).unwrap(), 0);
        read_at(&mut source, 0, b"ABC");
        assert_eq!(
            source.seek(SeekFrom::Current(-4)).unwrap_err().kind(),
            io::ErrorKind::InvalidInput
        );
    }

    /// Reject seeks when metadata shows the source changed after it was opened.
    #[test]
    fn detects_changes_while_a_compressed_source_is_open() {
        let fixture = Fixture::new(&gzip(b"first"));
        let mut source = fixture.open();
        fs::write(&fixture.path, gzip(b"replacement")).unwrap();
        assert_eq!(
            source.seek(SeekFrom::Start(0)).unwrap_err().kind(),
            io::ErrorKind::InvalidData
        );
    }

    /// Rebuild truncated checkpoint-table or dictionary files and retain correct range reads.
    #[test]
    fn truncated_cache_is_rebuilt_instead_of_rejecting_the_source() {
        let data = pseudo_random(2_200_000);
        let fixture = Fixture::new(&gzip(&data));
        for extension in ["sqi", "sqw"] {
            {
                let mut source = fixture.open();
                source.finish_index().unwrap();
            }
            let cached = fs::read_dir(&fixture.cache)
                .unwrap()
                .map(|entry| entry.unwrap().path())
                .find(|path| path.extension().is_some_and(|value| value == extension))
                .unwrap();
            OpenOptions::new()
                .write(true)
                .open(cached)
                .unwrap()
                .set_len(2)
                .unwrap();
            let mut source = fixture.open();
            let Input::Compressed(inner) = &source.inner else {
                panic!()
            };
            assert_eq!(inner.index.count, 1);
            assert!(inner.index.length.is_none());
            read_at(&mut source, 1_100_000, &data[1_100_000..1_100_100]);
        }
    }
}
