use seqviz::{
    cli,
    config::{self, Config},
    data::Dataset,
    display, ui,
};
use std::io::{self, IsTerminal, Write};

fn main() -> io::Result<()> {
    let paths = match cli::parse(std::env::args_os().skip(1))? {
        cli::Command::Help => {
            println!(
                "sq — terminal sequence browser\n\nUsage: sq <file> [file …]\n\nBrowse FASTA, protein FASTA, FASTQ, SAM and BAM, including gzip/BGZF.\nOpen multiple paths as file tabs. Use -- before paths beginning with '-'.\n\nF1/? help  F2 file  F3 record  F4 statistics\nTab switch file  / find name  : jump to position  q quit\n\nConfiguration: $XDG_CONFIG_HOME/seqviz-rs/config.toml\nCache: $XDG_CACHE_HOME/seqviz-rs (defaults to ~/.cache/seqviz-rs)\n\nRedirected output prints FASTA, including gzip/BGZF, using the browser reader.\nOther formats require an interactive terminal."
            );
            return Ok(());
        }
        cli::Command::Version => {
            println!("sq {}", env!("CARGO_PKG_VERSION"));
            return Ok(());
        }
        cli::Command::Browse(paths) => paths,
    };
    if io::stdout().is_terminal() && io::stdin().is_terminal() {
        return ui::run(paths, Config::load()?);
    }

    let cache = config::cache_directory();
    let stdout = io::stdout();
    let mut output = stdout.lock();

    for path in paths {
        let mut dataset = Dataset::open(&path, &cache)?;
        display::write_fasta(&mut dataset, &mut output, display::terminal_width)?;
    }

    output.flush()
}
