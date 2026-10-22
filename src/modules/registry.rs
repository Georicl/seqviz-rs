//! One assembly point for module metadata, probes and page factories.
use super::{
    sequence::{config::SequenceConfig, view::SequenceView},
    view::DocumentView,
};
use crate::widgets::theme::Theme;
use std::{
    path::{Path, PathBuf},
    sync::{Arc, atomic::AtomicU64},
};

pub struct ModuleDescriptor {
    pub id: &'static str,
    pub name: &'static str,
    pub formats: &'static [&'static str],
}
pub const SEQUENCE: ModuleDescriptor = ModuleDescriptor {
    id: "sequence",
    name: "Sequence",
    formats: &["FASTA", "FASTQ", "SAM", "BAM"],
};
pub fn modules() -> &'static [ModuleDescriptor] {
    std::slice::from_ref(&SEQUENCE)
}

/// Probing consumes only supplied decoded bytes; it never opens a file or builds an index.
pub fn probe(prefix: &[u8], path: &Path) -> Option<&'static ModuleDescriptor> {
    super::sequence::data::probe_format(prefix, path)
        .ok()
        .map(|_| &SEQUENCE)
}

/// Built-in page factories share only module-owned UI preferences, not document data.
pub(crate) struct ViewFactory {
    theme: Theme,
    settings: SequenceConfig,
    clock: Arc<AtomicU64>,
    shared: std::rc::Rc<std::cell::RefCell<super::sequence::view::SharedInteraction>>,
}
impl ViewFactory {
    pub(crate) fn new(theme: Theme, settings: SequenceConfig, clock: Arc<AtomicU64>) -> Self {
        let shared = std::rc::Rc::new(std::cell::RefCell::new(
            super::sequence::view::SharedInteraction {
                focus: super::sequence::view::Focus::Records,
                show_quality: settings.show_quality,
            },
        ));
        Self {
            theme,
            settings,
            clock,
            shared,
        }
    }
    /// The sole installed module can create its page before I/O; validation stays on the worker.
    pub(crate) fn create(&self, path: PathBuf, file: usize) -> Box<dyn DocumentView> {
        Box::new(SequenceView::new(
            path,
            file,
            self.settings.clone(),
            self.theme.clone(),
            Arc::clone(&self.clock),
            std::rc::Rc::clone(&self.shared),
        ))
    }
}

/// The redirected-output capability belongs to the module, not the app shell.
pub(crate) fn write_redirected(
    path: &Path,
    cache: &Path,
    output: &mut impl std::io::Write,
    width: impl FnMut() -> usize,
) -> std::io::Result<()> {
    let mut dataset = super::sequence::data::Dataset::open(path, cache)?;
    super::sequence::output::write_fasta(&mut dataset, output, width)
}
