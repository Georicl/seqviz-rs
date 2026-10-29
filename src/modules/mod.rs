//! Built-in document modules and their application-facing contracts.
/// A document can be hosted before its format has been inspected by the worker.
pub(crate) mod loading;
/// Domain routing lives here rather than in the generic task runtime.
pub mod protocol;
/// One assembly point for module metadata, probes and page factories.
pub mod registry;
/// FASTA, FASTQ, SAM and BAM document support.
pub mod sequence;
/// File ownership across tabs and the application's background session handle.
pub mod session;
/// Incremental VCF data access and its document backend.
pub mod vcf;
/// The application hosts this interface; concrete pages own their data semantics.
pub mod view;
