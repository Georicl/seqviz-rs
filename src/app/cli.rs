//! Command-line parsing. Opening files remains the default command.

use clap::{Parser, Subcommand};
use std::path::PathBuf;

#[derive(Subcommand)]
pub enum Commands {
    /// Open files in browser
    Browse {
        /// Files to open
        #[arg(short, long, required = true, num_args = 1..)]
        files: Vec<PathBuf>,
    },
}

#[derive(Parser)]
#[command(name = "sq")]
#[command(version)]
pub struct Cli {
    #[command(subcommand)]
    pub command: Commands,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn double_dash_preserves_option_like_file_names() {
        let args = Cli::try_parse_from(["sq", "browse", "--files", "a.fa", "a b.fa"]).unwrap();
        let Commands::Browse { files } = args.command;
        assert_eq!(files, vec![PathBuf::from("a.fa"), PathBuf::from("a b.fa")]);
    }
}
