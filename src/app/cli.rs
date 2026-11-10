//! Command-line parsing. Opening files remains the default command.

use clap::Parser;
use std::path::PathBuf;

/// Positional input paths parsed by the `sq` command.
///
/// # Fields
///
/// - `files`: One or more input file paths, kept in command-line order.
#[derive(Parser)]
#[command(
    name = "sq",
    display_name = "seqviz",
    version,
    about = "A terminal browser for bioformatics file",
    long_about = None
)]
pub struct Cli {
    /// Files to open
    #[arg(value_name = "FILE", required = true, num_args = 1..)]
    pub files: Vec<PathBuf>,
}

/// Argument parsing regressions for literal input paths.
#[cfg(test)]
mod tests {
    use super::*;

    /// Verify `--` preserves literal file names that resemble options or contain spaces.
    #[test]
    fn double_dash_preserves_option_like_file_names() {
        let args = Cli::try_parse_from(["sq", "--", "--help", "a b.fa"]).unwrap();
        assert_eq!(
            args.files,
            vec![PathBuf::from("--help"), PathBuf::from("a b.fa"),]
        );
    }
}
