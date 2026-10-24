//! Command-line parsing. Opening files remains the default command.

use clap::Parser;
use std::path::PathBuf;

#[derive(Parser)]
#[command(
    name = "sq",
    display_name = "seqviz",
    version,
    about = "A terminal browser for bioformatics file"
)]
pub struct Cli {
    /// Files to open
    #[arg(value_name = "FILE", required = true, num_args = 1..)]
    pub files: Vec<PathBuf>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn double_dash_preserves_option_like_file_names() {
        let args = Cli::try_parse_from(["sq", "--", "--help", "a b.fa"]).unwrap();

        assert_eq!(
            args.files,
            vec![PathBuf::from("--help"), PathBuf::from("a b.fa"),]
        );
    }
}
