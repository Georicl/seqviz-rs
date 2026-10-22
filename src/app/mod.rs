//! Application composition, command routing and document hosting.
pub mod cli;
pub mod config;
mod events;
pub(crate) mod layout;
mod render;
mod state;
pub mod terminal;
#[cfg(test)]
mod tests;

use crate::{core::paths, modules::registry};
use std::io::{self, IsTerminal, Write};

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
