use seqviz::format;
use seqviz::cli;

fn main() {
    let paths = crate::cli::read_paths();
    
    let files = format::receive_files(paths);
}
