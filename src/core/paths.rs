//! Configuration and cache locations, without creating files or directories.
use std::{env, path::PathBuf};

/// Resolve the cache location without creating it or opening any files.
pub fn cache_directory() -> PathBuf {
    if let Some(path) = env::var_os("XDG_CACHE_HOME") {
        PathBuf::from(path).join("seqviz-rs")
    } else if let Some(user_directory) = env::var_os("HOME") {
        PathBuf::from(user_directory).join(".cache/seqviz-rs")
    } else {
        env::temp_dir().join("seqviz-rs")
    }
}

/// Locate the user config under an absolute XDG path or the home-directory fallback.
pub fn config_path() -> Option<PathBuf> {
    config_path_from(
        env::var_os("XDG_CONFIG_HOME").map(PathBuf::from),
        env::var_os("HOME").map(PathBuf::from),
    )
}

/// Resolve configuration precedence without reading or changing process environment.
fn config_path_from(xdg: Option<PathBuf>, home: Option<PathBuf>) -> Option<PathBuf> {
    xdg.filter(|p| p.is_absolute())
        .or_else(|| home.map(|p| p.join(".config")))
        .map(|p| p.join("seqviz-rs/config.toml"))
}

/// Configuration-path precedence and absolute-XDG-path tests.
#[cfg(test)]
mod tests {
    use super::*;
    use std::path::Path;

    /// Verify a relative XDG path is ignored in favour of the home configuration directory.
    #[test]
    fn xdg_config_location_obeys_absolute_path_rule() {
        let home = Some(PathBuf::from("/home/test"));
        assert_eq!(
            config_path_from(Some("/config".into()), home.clone()).unwrap(),
            Path::new("/config/seqviz-rs/config.toml")
        );
        assert_eq!(
            config_path_from(Some("relative".into()), home).unwrap(),
            Path::new("/home/test/.config/seqviz-rs/config.toml")
        );
    }
}
