//! Reusable terminal components, receiving explicit content, state and style.
/// One-line input rendering and editing, independent of the input's meaning.
pub mod input;
/// Shared nucleotide colours, independent of sequence or variant document types.
pub mod nucleotides;
/// Render caller-provided status and shortcuts without interpreting their contents.
pub mod status_bar;
/// A file-agnostic tab bar with the browser's existing paging geometry.
pub mod tabs;
/// Selectable, scrollable text with display-column-aware wrapping.
pub mod text_panel;
/// Colours shared by application chrome and reusable widgets.
pub mod theme;
