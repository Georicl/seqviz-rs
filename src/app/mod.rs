//! Application composition, command routing and document hosting.
/// Command-line parsing. Opening files remains the default command.
pub mod cli;
/// Load the existing user configuration and assemble application/module settings.
pub mod config;
/// Global shortcuts and routing; document-specific interaction stays on its page.
mod events;
/// Outer application regions; document layouts are owned by their pages.
pub(crate) mod layout;
/// Compose application chrome around the active document page.
mod render;
/// Application-global tabs, document routing and service result lifetimes.
mod state;
/// The single owner of terminal mode and serialized terminal output.
pub mod terminal;
/// Application routing, tab lifetime and modal-input regression tests.
#[cfg(test)]
mod tests;

use crate::{core::paths, modules::registry};
use std::io::{self, IsTerminal, Write};

/// Start the terminal browser when stdin and stdout are terminals; otherwise stream output.
pub fn run(paths: Vec<std::path::PathBuf>) -> io::Result<()> {
    if io::stdout().is_terminal() && io::stdin().is_terminal() {
        return terminal::run(paths, config::Config::load()?);
    }
    let cache = paths::cache_directory();
    let stdout = io::stdout();
    let mut output = stdout.lock();
    for path in paths {
        registry::write_redirected(&path, &cache, &mut output, terminal::terminal_width)?;
    }
    output.flush()
}
