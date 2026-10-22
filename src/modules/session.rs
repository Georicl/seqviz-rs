//! File ownership across tabs and the application's background session handle.
use super::sequence::backend::Backend;
use super::sequence::protocol::{Request, Update};
use crate::core::tasks::TaskWorker;
use std::collections::HashMap;
use std::ops::Deref;
use std::path::{Path, PathBuf};

/// Alias tabs keep independent view state and share a single backend dataset.
pub(crate) struct SessionFiles {
    paths: Vec<PathBuf>,
    owners: Vec<usize>,
    closed: Vec<bool>,
}

impl SessionFiles {
    pub(crate) fn new(paths: Vec<PathBuf>) -> Self {
        let mut canonical_owners = HashMap::new();
        let owners = paths
            .iter()
            .enumerate()
            .map(|(file, path)| {
                let canonical = std::fs::canonicalize(path).unwrap_or_else(|_| path.clone());
                *canonical_owners.entry(canonical).or_insert(file)
            })
            .collect();
        let closed = vec![false; paths.len()];
        Self {
            paths,
            owners,
            closed,
        }
    }

    pub(crate) fn len(&self) -> usize {
        self.paths.len()
    }

    pub(crate) fn path(&self, file: usize) -> &Path {
        &self.paths[file]
    }

    pub(crate) fn owner(&self, file: usize) -> usize {
        self.owners[file]
    }

    pub(crate) fn is_open(&self, file: usize) -> bool {
        self.closed.get(file).is_some_and(|closed| !closed)
    }

    /// Return the owner and whether closing this tab removes its final alias.
    pub(crate) fn close(&mut self, file: usize) -> Option<(usize, bool)> {
        *self.closed.get_mut(file)? = true;
        let owner = self.owner(file);
        let last_alias = !self
            .owners
            .iter()
            .enumerate()
            .any(|(other, &candidate)| candidate == owner && !self.closed[other]);
        Some((owner, last_alias))
    }
}

pub struct Worker {
    tasks: TaskWorker<Request, Update>,
}

impl Worker {
    pub fn start(paths: Vec<PathBuf>, cache: PathBuf) -> Self {
        Self {
            tasks: TaskWorker::spawn(Request::Quit, move |updates, cancel| {
                Backend::new(SessionFiles::new(paths), cache, updates, cancel)
            }),
        }
    }

    #[cfg(test)]
    pub(crate) fn start_with_requests(
        paths: Vec<PathBuf>,
        cache: PathBuf,
        initial: Vec<Request>,
    ) -> Self {
        Self {
            tasks: TaskWorker::spawn_with_requests(
                Request::Quit,
                initial,
                move |updates, cancel| {
                    Backend::new(SessionFiles::new(paths), cache, updates, cancel)
                },
            ),
        }
    }
}

impl Deref for Worker {
    type Target = TaskWorker<Request, Update>;

    fn deref(&self) -> &Self::Target {
        &self.tasks
    }
}
