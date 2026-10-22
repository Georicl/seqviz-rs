//! One assembly point for module metadata, probes and page factories.
use super::{
    loading::LoadingView,
    protocol::ModuleKind,
    sequence::{config::SequenceConfig, view::SequenceView},
    vcf::{config::VcfConfig, view::VcfView},
    view::DocumentView,
};
use crate::widgets::theme::Theme;
use std::{
    path::{Path, PathBuf},
    sync::{Arc, atomic::AtomicU64},
};

#[derive(Clone, Copy)]
pub struct ModuleDescriptor {
    pub kind: ModuleKind,
    pub id: &'static str,
    pub name: &'static str,
    pub formats: &'static [&'static str],
}
pub const SEQUENCE: ModuleDescriptor = ModuleDescriptor {
    kind: ModuleKind::Sequence,
    id: "sequence",
    name: "Sequence",
    formats: &["FASTA", "FASTQ", "SAM", "BAM"],
};
pub const VCF: ModuleDescriptor = ModuleDescriptor {
    kind: ModuleKind::Vcf,
    id: "vcf",
    name: "VCF",
    formats: &["VCF"],
};
pub fn modules() -> &'static [ModuleDescriptor] {
    &[SEQUENCE, VCF]
}

/// Probing consumes only supplied decoded bytes; it never opens a file or builds an index.
pub fn probe(prefix: &[u8], path: &Path) -> Option<&'static ModuleDescriptor> {
    // VCF headers must be recognised before Sequence's permissive SAM probe.
    if prefix.starts_with(b"##fileformat=VCF")
        || prefix
            .split(|&byte| byte == b'\n')
            .any(|line| line.starts_with(b"#CHROM\tPOS\tID\tREF\tALT\tQUAL\tFILTER\tINFO"))
        || (prefix.is_empty() && path.extension().is_some_and(|extension| extension == "vcf"))
    {
        return Some(&VCF);
    }
    super::sequence::data::probe_format(prefix, path)
        .ok()
        .map(|_| &SEQUENCE)
}

/// Built-in page factories share only module-owned UI preferences, not document data.
pub(crate) struct ViewFactory {
    theme: Theme,
    settings: SequenceConfig,
    vcf: VcfConfig,
    clock: Arc<AtomicU64>,
    shared: std::rc::Rc<std::cell::RefCell<super::sequence::view::SharedInteraction>>,
}
impl ViewFactory {
    pub(crate) fn new(
        theme: Theme,
        settings: SequenceConfig,
        vcf: VcfConfig,
        clock: Arc<AtomicU64>,
    ) -> Self {
        let shared = std::rc::Rc::new(std::cell::RefCell::new(
            super::sequence::view::SharedInteraction {
                focus: super::sequence::view::Focus::Records,
                show_quality: settings.show_quality,
            },
        ));
        Self {
            theme,
            settings,
            vcf,
            clock,
            shared,
        }
    }
    pub(crate) fn loading(&self, path: PathBuf, file: usize) -> Box<dyn DocumentView> {
        Box::new(LoadingView::new(path, file, self.theme.clone()))
    }

    /// Module identity comes from the background session, never UI-thread file I/O.
    pub(crate) fn create(
        &self,
        path: PathBuf,
        file: usize,
        module: ModuleKind,
    ) -> Box<dyn DocumentView> {
        match module {
            ModuleKind::Vcf => Box::new(VcfView::new(
                path,
                file,
                self.vcf.clone(),
                self.theme.clone(),
                Arc::clone(&self.clock),
            )),
            ModuleKind::Sequence => Box::new(SequenceView::new(
                path,
                file,
                self.settings.clone(),
                self.theme.clone(),
                Arc::clone(&self.clock),
                std::rc::Rc::clone(&self.shared),
            )),
        }
    }
}

/// The redirected-output capability belongs to the module, not the app shell.
pub(crate) fn write_redirected(
    path: &Path,
    cache: &Path,
    output: &mut impl std::io::Write,
    width: impl FnMut() -> usize,
) -> std::io::Result<()> {
    use std::io::Read;
    std::fs::create_dir_all(cache)?;
    let source = crate::core::source::Source::open(path, cache)?;
    let mut prefix = Vec::with_capacity(8192);
    source.take(8192).read_to_end(&mut prefix)?;
    if probe(&prefix, path).is_some_and(|module| module.kind == ModuleKind::Vcf) {
        return Err(std::io::Error::new(
            std::io::ErrorKind::Unsupported,
            "VCF browsing requires an interactive terminal",
        ));
    }
    let mut dataset = super::sequence::data::Dataset::open(path, cache)?;
    super::sequence::output::write_fasta(&mut dataset, output, width)
}
