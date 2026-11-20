//! Explicit record edits. The index supplies decoded record boundaries; the source
//! is replaced only after a complete, same-directory rewrite succeeds.
use super::{Dataset, Format, invalid};
use crate::core::cache::file_identity;
use std::{
    fs::OpenOptions,
    io::{self, BufRead, BufWriter, Read, Seek, SeekFrom, Write},
};
use tempfile::NamedTempFile;

/// A draft is tied to a specific record in a specific version of the source.
#[derive(Clone, Debug)]
pub struct Draft {
    pub(crate) record: u64,
    identity: [u64; 7],
    pub(crate) bases: Vec<u8>,
    /// Zero is a draft-only marker for an inserted base awaiting a Phred+33 character.
    pub(crate) quality: Option<Vec<u8>>,
}

/// The same sequence alphabet accepted by the text parser.
pub(crate) fn is_residue(b: u8) -> bool {
    b.is_ascii_alphabetic() || b"-.*".contains(&b)
}

impl Dataset {
    /// Materialize only the explicitly selected complete record.
    pub(crate) fn draft(&mut self, id: u64) -> io::Result<Draft> {
        if !matches!(self.format(), Format::Fasta | Format::Fastq) {
            return Err(invalid("Editing is available for FASTA and FASTQ"));
        }
        let meta = self
            .record(id)?
            .filter(|r| r.complete)
            .ok_or_else(|| invalid("Wait for the current record to finish indexing"))?;
        let len = usize::try_from(meta.length).map_err(|_| invalid("Record too large to edit"))?;
        let window = self.read_range(id, 0, len)?;
        let fastq = self.format() == Format::Fastq;
        if window.sequence.len() != len || (fastq && window.quality.len() != len) {
            return Err(invalid("Incomplete record in index"));
        }
        Ok(Draft {
            record: id,
            identity: self.source_identity,
            bases: window.sequence,
            quality: fastq.then_some(window.quality),
        })
    }

    /// Verify identity under a source lock, then atomically publish the completed rewrite.
    pub(crate) fn save_draft(&mut self, edit: &Draft) -> io::Result<()> {
        if self.format().is_alignment() || self.source_identity != edit.identity {
            return Err(invalid("Source changed since editing began"));
        }
        if edit.bases.iter().any(|&b| !is_residue(b))
            || edit.quality.is_some() != (self.format() == Format::Fastq)
            || edit.quality.as_ref().is_some_and(|q| {
                q.len() != edit.bases.len() || q.iter().any(|v| !(33..=126).contains(v))
            })
        {
            return Err(invalid("Invalid sequence or missing FASTQ quality"));
        }
        self.check_source()?;
        let original = OpenOptions::new()
            .read(true)
            .write(true)
            .open(&self.source_path)?;
        original.try_lock().map_err(io::Error::other)?;
        if file_identity(&original.metadata()?) != edit.identity {
            return Err(invalid("Source changed before save"));
        }
        let parent = self
            .source_path
            .parent()
            .ok_or_else(|| invalid("Missing parent directory"))?;
        let mut staged = NamedTempFile::new_in(parent)?;
        let out = BufWriter::new(staged.as_file_mut());
        let result = match self.compression() {
            "gzip" => {
                let mut gzip = flate2::write::GzEncoder::new(out, flate2::Compression::default());
                self.rewrite(edit, &mut gzip)
                    .and_then(|_| gzip.finish()?.flush())
            }
            "BGZF" => {
                let mut bgzf = noodles_bgzf::io::Writer::new(out);
                self.rewrite(edit, &mut bgzf)
                    .and_then(|_| bgzf.finish()?.flush())
            }
            _ => {
                let mut writer = out;
                self.rewrite(edit, &mut writer).and_then(|_| writer.flush())
            }
        };
        let restore = self.reader.seek(SeekFrom::Start(self.position));
        result?;
        restore?;
        staged
            .as_file()
            .set_permissions(original.metadata()?.permissions())?;
        staged.as_file().sync_all()?;
        self.check_source()?;
        staged.persist(&self.source_path)?;
        Ok(())
    }

    fn rewrite(&mut self, edit: &Draft, out: &mut dyn Write) -> io::Result<()> {
        let (body, separator, quality, end, length): (u64, Option<u64>, Option<u64>, u64, usize) =
            self.index.db.query_row(
                "SELECT body,separator,quality_start,record_end,length FROM records WHERE id=? AND complete=1",
                [edit.record],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?, r.get(4)?)),
            ).map_err(super::index::sql)?;
        let reader = &mut self.reader;
        // Recover the defline's newline convention, including a final defline without newline.
        let probe = body.min(2) as usize;
        reader.seek(SeekFrom::Start(body - probe as u64))?;
        let mut ending = [0; 2];
        reader.read_exact(&mut ending[..probe])?;
        let eol: &[u8] = if ending == *b"\r\n" { b"\r\n" } else { b"\n" };
        reader.seek(SeekFrom::Start(0))?;
        copy_exact(reader, out, body)?;
        if !ending[..probe].ends_with(b"\n") && !edit.bases.is_empty() {
            out.write_all(eol)?;
        }
        let seq_end = separator.unwrap_or(end);
        rewrite_symbols(
            reader.take(seq_end - body),
            out,
            &edit.bases,
            length,
            edit.quality.is_none(),
            eol,
        )?;
        if let (Some(start), Some(q)) = (quality, &edit.quality) {
            copy_exact(reader, out, start - seq_end)?;
            rewrite_symbols(reader.take(end - start), out, q, length, false, eol)?;
        }
        io::copy(reader, out)?;
        Ok(())
    }
}

fn copy_exact(input: &mut impl Read, out: &mut dyn Write, len: u64) -> io::Result<()> {
    if io::copy(&mut input.take(len), out)? != len {
        return Err(invalid("Source shortened during save"));
    }
    Ok(())
}

/// Preserve wrapping, comments and whitespace. Extra symbols continue immediately
/// after the last original symbol, before its line ending.
fn rewrite_symbols(
    mut input: impl BufRead,
    out: &mut dyn Write,
    new: &[u8],
    old_len: usize,
    fasta: bool,
    eol: &[u8],
) -> io::Result<()> {
    if old_len == 0 && !new.is_empty() {
        out.write_all(new)?;
        out.write_all(eol)?;
    }
    let mut buffered = BufWriter::new(out);
    let out = &mut buffered;
    let mut seen = 0;
    let mut line = Vec::new();
    while input.read_until(b'\n', &mut line)? > 0 {
        if fasta && line.starts_with(b"#") {
            out.write_all(&line)?;
        } else {
            for &byte in &line {
                if byte.is_ascii_whitespace() {
                    out.write_all(&[byte])?;
                } else {
                    if let Some(replacement) = new.get(seen) {
                        out.write_all(std::slice::from_ref(replacement))?;
                    }
                    seen += 1;
                    if seen == old_len && new.len() > seen {
                        out.write_all(&new[seen..])?;
                    }
                }
            }
        }
        line.clear();
    }
    if seen != old_len {
        return Err(invalid("Record length changed during save"));
    }
    out.flush()
}

#[cfg(test)]
mod tests {
    use super::*;
    use flate2::{Compression, read::MultiGzDecoder, write::GzEncoder};
    use std::{fs, path::Path};

    fn indexed(path: &Path) -> Dataset {
        let mut data = Dataset::open(path, &path.parent().unwrap().join("cache")).unwrap();
        while !data.progress().complete {
            assert!(data.step(17).unwrap().error.is_none());
        }
        data
    }

    #[test]
    fn rewrites_wrapped_fastq_in_each_container_and_reopens() {
        let input = b"@one description\r\nAC\r\nGT\r\n+one\r\n@+\r\nIJ\r\n@two\nNN\n+\n!!";
        let expected = b"@one description\r\nTG\r\nACG\r\n+one\r\nI!\r\n+@J\r\n@two\nNN\n+\n!!";
        for codec in ["none", "gzip", "BGZF"] {
            let tmp = tempfile::tempdir().unwrap();
            let path = tmp.path().join("reads.fq");
            let bytes = match codec {
                "gzip" => {
                    let mut w = GzEncoder::new(Vec::new(), Compression::fast());
                    w.write_all(input).unwrap();
                    w.finish().unwrap()
                }
                "BGZF" => {
                    let mut w = noodles_bgzf::io::Writer::new(Vec::new());
                    w.write_all(input).unwrap();
                    w.finish().unwrap()
                }
                _ => input.to_vec(),
            };
            fs::write(&path, bytes).unwrap();
            let mut ds = indexed(&path);
            let mut draft = ds.draft(0).unwrap();
            draft.bases = b"TGACG".to_vec();
            draft.quality = Some(b"I!+@J".to_vec());
            ds.save_draft(&draft).unwrap();
            let file = fs::read(&path).unwrap();
            let result = if codec == "none" {
                file
            } else {
                let mut decoded = Vec::new();
                MultiGzDecoder::new(file.as_slice())
                    .read_to_end(&mut decoded)
                    .unwrap();
                decoded
            };
            assert_eq!(result, expected);
            let mut reopened = indexed(&path);
            assert_eq!(reopened.compression(), codec);
            assert_eq!(reopened.draft(0).unwrap().bases, draft.bases);
            assert_eq!(reopened.draft(1).unwrap().quality.unwrap(), b"!!");
        }
    }

    #[test]
    fn preserves_fasta_comments_and_handles_empty_records() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("seq.fa");
        fs::write(&path, b">first\r\nAC\r\n# note\r\nTT\r\n>empty\r\n>last").unwrap();
        for (id, bases) in [(0, &b"T"[..]), (1, b"AG"), (2, b"TC")] {
            let mut ds = indexed(&path);
            let mut draft = ds.draft(id).unwrap();
            draft.bases = bases.into();
            ds.save_draft(&draft).unwrap();
        }
        assert_eq!(
            fs::read(&path).unwrap(),
            b">first\r\nT\r\n# note\r\n\r\n>empty\r\nAG\r\n>last\nTC\n"
        );
    }

    #[test]
    fn rejects_invalid_quality_and_external_changes_without_overwriting() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("reads.fq");
        let input = b"@one\nAC\n+\n!!\n";
        fs::write(&path, input).unwrap();
        let mut ds = indexed(&path);
        let mut draft = ds.draft(0).unwrap();
        draft.quality.as_mut().unwrap()[1] = 0;
        assert!(ds.save_draft(&draft).is_err());
        assert_eq!(fs::read(&path).unwrap(), input);
        draft.quality.as_mut().unwrap()[1] = b'I';
        let other = b"@another\nT\n+\nH\n";
        fs::write(&path, other).unwrap();
        assert!(ds.save_draft(&draft).is_err());
        assert_eq!(fs::read(&path).unwrap(), other);
    }
}
