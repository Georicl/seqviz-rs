//! Command-line entry point for the terminal browser.

use clap::Parser;
use seqviz::app::cli;
use std::io;

/// Parse input paths and return application failures as the command exit status.
fn main() -> io::Result<()> {
    let args = cli::Cli::parse();
    seqviz::app::run(args.files)
}
