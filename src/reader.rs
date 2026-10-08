use std::fs::{self, File};
use crate::format::FileInfo;

fn file_reader(FileInfo: FileInfo) {
    let files = File::open(FileInfo.path)
}