//! FASTA, FASTQ, SAM and BAM document support.
/// Incremental scanning, snapshot publication and sequence job scheduling.
pub(crate) mod backend;
/// Sequence display settings and their defaults.
pub mod config;
/// Format parsers, persistent indexes and bounded sequence range reads.
pub mod data;
/// Derived properties from recognised sequencing-platform FASTQ headers.
pub mod headers;
/// Cancellable name-search and copy jobs over indexed records.
pub(crate) mod operations;
/// Streaming FASTA output for redirected command-line use.
pub mod output;
/// Nucleotide, amino-acid and quality-score colour mappings.
pub mod palette;
/// Typed requests and updates exchanged with the sequence backend.
pub mod protocol;
/// Per-document interaction, layout and rendering state.
pub(crate) mod view;
/// Sequence-specific drawing components.
mod widgets;
pub use data::{Dataset, Format, RecordMeta, RecordSummary, SequenceWindow, Stats};
