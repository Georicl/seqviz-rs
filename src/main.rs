use seqviz::{config::Config, display, format, ui};
use std::fs::File;
use std::io::{self, BufReader, IsTerminal, Write};
use std::path::PathBuf;

fn main() -> io::Result<()> {
    let mut paths = Vec::<PathBuf>::new();
    let mut positional = false;
    for arg in std::env::args_os().skip(1) {
        if !positional && arg == "--" {
            positional = true;
            continue;
        }
        if !positional && (arg == "--help" || arg == "-h") {
            println!(
                "sq — terminal sequence browser\n\nUsage: sq <file> [file …]\n\nBrowse FASTA, protein FASTA, FASTQ, SAM and BAM, including gzip/BGZF.\nOpen multiple paths as file tabs. Use -- before paths beginning with '-'.\n\nF1/? help  F2 file  F3 record  F4 statistics\nTab switch file  / find name  : jump to position  q quit\n\nConfiguration: $XDG_CONFIG_HOME/seqviz-rs/config.toml\nCache: $XDG_CACHE_HOME/seqviz-rs (defaults to ~/.cache/seqviz-rs)\n\nRedirected output retains plain FASTA printing; browsing requires a terminal."
            );
            return Ok(());
        }
        if !positional && (arg == "--version" || arg == "-V") {
            println!("sq {}", env!("CARGO_PKG_VERSION"));
            return Ok(());
        }
        paths.push(arg.into());
    }
    if paths.is_empty() {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "Usage: sq <file> [file …]",
        ));
    }
    if io::stdout().is_terminal() && io::stdin().is_terminal() {
        return ui::run(paths, Config::load()?);
    }

    let files = format::receive_files(paths);
    let stdout = io::stdout();
    let mut output = stdout.lock();

    for file in files {
        let mut reader = BufReader::new(File::open(&file.path)?);

        display::write_fasta(&mut reader, &mut output, display::terminal_width)?;
    }

    output.flush()
}
