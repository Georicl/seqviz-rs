//! Load the existing user configuration and assemble application/module settings.
pub use crate::core::paths::{cache_directory, config_path};
use crate::modules::sequence::config::SequenceConfig;
pub use crate::modules::sequence::palette::ContentPalette;
pub use crate::widgets::theme::Theme;
use crate::widgets::theme::set_color;
use serde::Deserialize;
use std::{
    fs, io,
    ops::{Deref, DerefMut},
    path::{Path, PathBuf},
};

#[derive(Debug, Clone, Default)]
pub struct Config {
    pub theme: Theme,
    pub sequence: SequenceConfig,
}

// Existing callers can continue accessing display settings while ownership lives
// in SequenceConfig; shared Theme no longer contains any sequence palette.
impl Deref for Config {
    type Target = SequenceConfig;
    fn deref(&self) -> &Self::Target {
        &self.sequence
    }
}
impl DerefMut for Config {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.sequence
    }
}

impl Config {
    /// Missing user configuration uses the built-in theme and module defaults.
    pub fn load() -> io::Result<Self> {
        let Some(path) = config_path() else {
            return Ok(Self::default());
        };
        match fs::read_to_string(&path) {
            Ok(text) => Self::parse(&text, &path),
            Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(Self::default()),
            Err(error) => Err(error),
        }
    }

    pub fn load_from(path: &Path) -> io::Result<Self> {
        Self::parse(&fs::read_to_string(path)?, path)
    }

    fn parse(text: &str, path: &Path) -> io::Result<Self> {
        let raw: RawConfig =
            toml::from_str(text).map_err(|e| invalid(format!("{}: {e}", path.display())))?;
        let mut config = Self {
            theme: Theme::default(),
            sequence: raw.display,
        };
        let theme = &mut config.theme;
        set_color(
            &mut theme.background,
            raw.theme.background,
            "theme.background",
        )?;
        set_color(
            &mut theme.foreground,
            raw.theme.foreground,
            "theme.foreground",
        )?;
        set_color(&mut theme.muted, raw.theme.muted, "theme.muted")?;
        set_color(&mut theme.border, raw.theme.border, "theme.border")?;
        set_color(&mut theme.focus, raw.theme.focus, "theme.focus")?;
        set_color(&mut theme.selection, raw.theme.selection, "theme.selection")?;
        if let Some(content) = raw.theme.content {
            let content_path = path
                .parent()
                .unwrap_or_else(|| Path::new("."))
                .join(content);
            let content_text = fs::read_to_string(&content_path).map_err(|e| {
                io::Error::new(e.kind(), format!("{}: {e}", content_path.display()))
            })?;
            config
                .sequence
                .palette
                .apply(&content_text)
                .map_err(|e| invalid(format!("{}: {e}", content_path.display())))?;
        }
        Ok(config)
    }
}

#[derive(Default, Deserialize)]
#[serde(default, deny_unknown_fields)]
struct RawConfig {
    display: SequenceConfig,
    theme: RawTheme,
}

#[derive(Default, Deserialize)]
#[serde(default, deny_unknown_fields)]
struct RawTheme {
    content: Option<PathBuf>,
    background: Option<String>,
    foreground: Option<String>,
    muted: Option<String>,
    border: Option<String>,
    focus: Option<String>,
    selection: Option<String>,
}

fn invalid(message: impl Into<String>) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, message.into())
}

#[cfg(test)]
mod tests {
    use super::*;
    use ratatui::style::Color;

    #[test]
    fn sample_config_resolves_its_content_palette() {
        let config =
            Config::load_from(&Path::new(env!("CARGO_MANIFEST_DIR")).join("config/config.toml"))
                .unwrap();
        assert!(config.show_quality);
        assert_eq!(
            config.sequence.palette.nucleotide_color(b'a'),
            Color::Rgb(126, 211, 151)
        );
        assert_eq!(
            config.sequence.palette.amino_acid_color(b'D'),
            config.sequence.palette.amino_acid_color(b'e')
        );
        assert_ne!(
            config.sequence.palette.quality_color(29),
            config.sequence.palette.quality_color(30)
        );
    }

    #[test]
    fn partial_config_preserves_defaults() {
        let config = Config::parse(
            "[display]\nshow_quality = false\n[theme]\nfocus = '#112233'",
            Path::new("config.toml"),
        )
        .unwrap();
        assert!(!config.show_quality);
        assert_eq!(config.theme.focus, Color::Rgb(17, 34, 51));
        assert_eq!(config.theme.background, Theme::default().background);
    }

    #[test]
    fn invalid_config_and_missing_referenced_palette_are_reported() {
        assert!(
            Config::parse(
                "[theme]\nforeground='not-a-colour'",
                Path::new("config.toml")
            )
            .is_err()
        );
        assert!(Config::parse("[display]\nshow_qualty=false", Path::new("config.toml")).is_err());
        assert!(
            Config::parse(
                "[theme]\ncontent='missing.toml'",
                Path::new("/no-such-seqviz-dir/config.toml")
            )
            .is_err()
        );
    }
}
