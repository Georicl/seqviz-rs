//! Shared services independent of application pages and biological formats.
/// Cache identity and writer ownership for persistent record indexes.
pub(crate) mod cache;
/// Clipboard preparation runs off the UI thread; only the UI writes terminal escapes.
pub mod clipboard;
/// Configuration and cache locations, without creating files or directories.
pub mod paths;
/// Seekable input with on-disk gzip/BGZF restart points.
pub mod source;
/// Domain-independent background execution, channels and cancellation lifetime.
pub mod tasks;
