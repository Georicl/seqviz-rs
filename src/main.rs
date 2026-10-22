use seqviz::app::cli;
use std::io;

fn main() -> io::Result<()> {
    let paths = match cli::parse(std::env::args_os().skip(1))? {
        cli::Command::Help => {
            println!(
                "sq — terminal sequence and variant browser

Usage: sq <file> [file …]

Browse FASTA, protein FASTA, FASTQ, SAM, BAM and ordinary VCF, including gzip/BGZF.
Open multiple paths as file tabs. Use -- before paths beginning with '-'.

F1/? help  F2 file  Tab switch file  : jump to position  q quit
Sequence: F3 record  F4 statistics  / find name
VCF: c contig  Enter details  1/2/3 INFO/Samples/Raw  m/F3 header  y/Y copy

Configuration: $XDG_CONFIG_HOME/seqviz-rs/config.toml
Cache: $XDG_CACHE_HOME/seqviz-rs (defaults to ~/.cache/seqviz-rs)

Redirected output prints FASTA, including gzip/BGZF, using the browser reader.
Other formats require an interactive terminal."
            );
            return Ok(());
        }
        cli::Command::Version => {
            println!("sq {}", env!("CARGO_PKG_VERSION"));
            return Ok(());
        }
        cli::Command::Browse(paths) => paths,
    };
    seqviz::app::run(paths)
}
