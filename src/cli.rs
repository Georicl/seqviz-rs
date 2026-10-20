//! Command-line parsing. Opening files remains the default command.

use std::{ffi::OsString, io, path::PathBuf};

pub enum Command {
    Browse(Vec<PathBuf>),
    Help,
    Version,
}

pub fn parse(args: impl IntoIterator<Item = OsString>) -> io::Result<Command> {
    let mut paths = Vec::new();
    let mut positional = false;
    for arg in args {
        if !positional {
            if arg == "--" {
                positional = true;
                continue;
            }
            if arg == "--help" || arg == "-h" {
                return Ok(Command::Help);
            }
            if arg == "--version" || arg == "-V" {
                return Ok(Command::Version);
            }
        }
        paths.push(PathBuf::from(arg));
    }
    if paths.is_empty() {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "Usage: sq <file> [file …]",
        ));
    }
    Ok(Command::Browse(paths))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn double_dash_preserves_option_like_file_names() {
        let Command::Browse(paths) = parse(["--", "--help", "a b.fa"].map(OsString::from)).unwrap()
        else {
            panic!("expected file paths");
        };
        assert_eq!(paths, [PathBuf::from("--help"), PathBuf::from("a b.fa")]);
    }
}
