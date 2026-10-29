//! Incremental VCF data access and its document backend.
/// Coordinate indexing and navigation requests on the session thread.
pub(crate) mod backend;
/// Configure detail layout and semantic VCF colours.
pub mod config;
/// Maintain a resumable SQLite index over decoded VCF source offsets.
pub(crate) mod data;
/// Expose the optional graph-annotation extension point.
pub mod graph;
/// Share preserved VCF records, metadata and index summaries.
pub mod model;
/// Assign consistent colours to bases, allele indices and filters.
pub mod palette;
/// Define viewport, navigation and progress messages.
pub(crate) mod protocol;
/// Validate VCF headers and records while preserving original text.
pub(crate) mod reader;
/// Manage the variant table and selected-record detail panels.
pub(crate) mod view;

pub use model::*;

/// Verify parsing, cache recovery and backend request ordering.
#[cfg(test)]
mod tests;
