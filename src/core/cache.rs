//! Cache identity and writer ownership for persistent record indexes.
use std::{
    fs::{self, File, OpenOptions},
    io,
    os::{fd::AsRawFd, unix::fs::MetadataExt},
    path::{Path, PathBuf},
    sync::atomic::{AtomicU64, Ordering},
};

pub(crate) fn file_identity(metadata: &fs::Metadata) -> [u64; 7] {
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
pub(crate) fn lock_cache(directory: &Path, key: u64) -> io::Result<(PathBuf, File, bool)> {
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
