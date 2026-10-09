use std::env;
use std::path::PathBuf;

/// 跳过程序名，把后面的命令行参数收集为文件路径。
pub fn read_paths() -> Vec<PathBuf> {
    let mut paths = Vec::new();

    for arg in env::args_os().skip(1) {
        paths.push(PathBuf::from(arg));
    }

    paths
}
