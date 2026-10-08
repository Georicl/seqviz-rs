use std::env;
use std::path::PathBuf;

/// Collect command-line arguments as file paths, skipping the program argument.
pub fn read_paths() -> Vec<PathBuf> {
    let mut paths = Vec::new();

    for arg in env::args_os().skip(1) {
        paths.push(PathBuf::from(arg));
    }

    paths
}
