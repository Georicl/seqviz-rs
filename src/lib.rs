//! Terminal document browser: application, shared services, widgets and domain modules.
/// Application composition, command routing and document hosting.
pub mod app;
/// Shared services independent of application pages and biological formats.
pub mod core;
/// Built-in document modules and their application-facing contracts.
pub mod modules;
/// Reusable terminal components, receiving explicit content, state and style.
pub mod widgets;

// Preserve the existing library entry points while implementation lives in its owner layer.
pub use app::{cli, config, terminal as ui};
pub use core::{clipboard, source};
pub use modules::{
    sequence::{data, headers, output as display},
    session as worker,
};
