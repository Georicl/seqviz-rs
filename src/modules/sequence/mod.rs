//! FASTA, FASTQ, SAM and BAM document support.
pub(crate) mod backend;
pub mod config;
pub mod data;
pub mod headers;
pub(crate) mod operations;
pub mod output;
pub mod palette;
pub mod protocol;
pub(crate) mod view;
mod widgets;
pub use data::{Dataset, Format, RecordMeta, RecordSummary, SequenceWindow, Stats};
