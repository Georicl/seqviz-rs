use seqviz::cli;
use seqviz::fa_reader;
use seqviz::format;
use std::fs::File;
use std::io::{self, BufReader};

fn main() -> io::Result<()> {
    let paths = crate::cli::read_paths();

    let files = format::receive_files(paths);

    for file in files {
        let mut reader = BufReader::new(File::open(&file.path)?);

        if let Some(header) = fa_reader::read_first_header(&mut reader)? {
            println!("{header:#?}");
        }
    }

    Ok(())
}
