//! Terminal document browser: application, shared services, widgets and domain modules.
pub mod app;
pub mod core;
pub mod modules;
pub mod widgets;

// Preserve the existing library entry points while implementation lives in its owner layer.
pub use app::{cli, config, terminal as ui};
pub use core::{clipboard, source};
pub use modules::{
    sequence::{data, headers, output as display},
    session as worker,
};
