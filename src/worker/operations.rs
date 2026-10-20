//! Search and copy jobs retain their own cancellation flag across layout refreshes.
use super::{ProgressKey, Slot, Update};
use crate::data::Dataset;
use std::{
    io,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
        mpsc,
    },
};

pub(super) struct SearchJob {
    pub operation_id: u64,
    pub file: usize,
    pub query: String,
    pub after: u64,
    pub cancel: Arc<AtomicBool>,
    pub tried: Option<ProgressKey>,
}

impl SearchJob {
    /// Returns true once a result, failure, or cancellation ends the job.
    pub fn step(&mut self, slot: &mut Slot, stopped: bool, updates: &mpsc::Sender<Update>) -> bool {
        if self.cancel.load(Ordering::Relaxed) {
            return true;
        }
        let key = ProgressKey::from(&slot.dataset.progress());
        if self.tried.as_ref().is_some_and(|last| {
            last.records == key.records
                && last.complete == key.complete
                && last.errors == key.errors
                && last.error == key.error
        }) {
            return false;
        }
        self.tried = Some(key);
        let result =
            slot.dataset
                .find_name_cancellable(&self.query, self.after, Arc::clone(&self.cancel));
        let result = match result {
            Ok(None) if stopped && self.after > 0 => {
                slot.dataset
                    .find_name_cancellable(&self.query, 0, Arc::clone(&self.cancel))
            }
            result => result,
        };
        if self.cancel.load(Ordering::Relaxed) {
            return true;
        }
        match result {
            Ok(Some(record)) => {
                let _ = updates.send(Update::Selected {
                    operation_id: self.operation_id,
                    file: self.file,
                    record,
                });
                true
            }
            Ok(None) if stopped => {
                failed(
                    updates,
                    self.operation_id,
                    self.file,
                    format!("No record matching {:?}", self.query),
                );
                true
            }
            Err(error) => {
                if error.kind() != io::ErrorKind::Interrupted {
                    failed(updates, self.operation_id, self.file, error.to_string());
                }
                true
            }
            Ok(None) => false,
        }
    }
}

pub(super) struct CopyJob {
    pub operation_id: u64,
    pub file: usize,
    pub record: u64,
    pub range: Option<(u64, u64)>,
    pub cancel: Arc<AtomicBool>,
}

impl CopyJob {
    pub fn step(&self, slot: &mut Slot, stopped: bool, updates: &mpsc::Sender<Update>) -> bool {
        if self.cancel.load(Ordering::Relaxed) {
            return true;
        }
        let result = self.read(slot, stopped);
        if self.cancel.load(Ordering::Relaxed) {
            return true;
        }
        match result {
            Ok(Some(text)) => {
                let _ = updates.send(Update::Copied {
                    operation_id: self.operation_id,
                    file: self.file,
                    text,
                });
                true
            }
            Ok(None) => false,
            Err(error) => {
                if error.kind() != io::ErrorKind::Interrupted {
                    failed(updates, self.operation_id, self.file, error.to_string());
                }
                true
            }
        }
    }

    fn read(&self, slot: &mut Slot, stopped: bool) -> io::Result<Option<String>> {
        if !slot.dataset.format().can_copy() {
            return Err(io::Error::new(
                io::ErrorKind::PermissionDenied,
                "Copy is disabled for SAM/BAM",
            ));
        }
        let Some(meta) = slot.dataset.record(self.record)? else {
            return if stopped {
                Err(io::Error::new(
                    io::ErrorKind::NotFound,
                    "Record is not available",
                ))
            } else {
                Ok(None)
            };
        };
        let ready = match self.range {
            Some((start, end)) => start <= end && (meta.length >= end || meta.complete),
            None => meta.complete,
        };
        if ready {
            let text = match self.range {
                Some((start, end)) => {
                    copy_range(&mut slot.dataset, self.record, start, end, &self.cancel)?
                }
                None => slot.dataset.copy_record(self.record, &self.cancel)?,
            };
            Ok(Some(text))
        } else if stopped || self.range.is_some_and(|(start, end)| start > end) {
            Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "Cannot copy an incomplete record or invalid range",
            ))
        } else {
            Ok(None)
        }
    }
}

fn failed(updates: &mpsc::Sender<Update>, operation_id: u64, file: usize, text: String) {
    let _ = updates.send(Update::OperationFailed {
        operation_id,
        file,
        text,
    });
}

fn copy_range(
    dataset: &mut Dataset,
    record: u64,
    start: u64,
    end: u64,
    cancel: &AtomicBool,
) -> io::Result<String> {
    let mut result = Vec::new();
    let mut offset = start;
    while offset < end {
        if cancel.load(Ordering::Relaxed) {
            return Err(io::Error::new(io::ErrorKind::Interrupted, "Copy cancelled"));
        }
        let window = dataset.read_range(record, offset, (end - offset).min(64 * 1024) as usize)?;
        if window.sequence.is_empty() {
            return Err(io::Error::new(
                io::ErrorKind::UnexpectedEof,
                "Selection is not fully available",
            ));
        }
        offset += window.sequence.len() as u64;
        result.extend(window.sequence);
    }
    String::from_utf8(result).map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))
}
