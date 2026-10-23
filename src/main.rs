use clap::Parser;
use seqviz::app::cli;
use std::io;

fn main() -> io::Result<()> {
    let paths = cli::Cli::parse();
    match paths.command {
        cli::Commands::Browse { files } => seqviz::app::run(files),
    }
}
