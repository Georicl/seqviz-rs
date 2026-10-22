//! Incremental VCF data access and its document backend.
pub(crate) mod backend;
pub mod config;
pub(crate) mod data;
pub mod graph;
pub mod model;
pub mod palette;
pub(crate) mod protocol;
pub(crate) mod reader;
pub(crate) mod view;

pub use model::*;

#[cfg(test)]
mod tests;
