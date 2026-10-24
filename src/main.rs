use clap::Parser;
use seqviz::app::cli;
use std::io;

fn main() -> io::Result<()> {
    let args = cli::Cli::parse();
    seqviz::app::run(args.files)
}
