//! Load the existing user configuration and assemble application/module settings.
pub use crate::core::paths::{cache_directory, config_path};
use crate::modules::sequence::config::SequenceConfig;
pub use crate::modules::sequence::palette::ContentPalette;
use crate::modules::vcf::config::VcfConfig;
pub use crate::widgets::theme::Theme;
use crate::widgets::theme::set_color;
use serde::Deserialize;
use std::{
    fs, io,
    ops::{Deref, DerefMut},
    path::{Path, PathBuf},
};

/// Resolved application theme and per-module display settings.
#[derive(Debug, Clone, Default)]
pub struct Config {
    /// Colours for application chrome and shared widgets.
    pub theme: Theme,
    /// Display preferences and content colours for sequence documents.
    pub sequence: SequenceConfig,
    /// Variant-browser layout and colour settings.
    pub vcf: VcfConfig,
}

// Existing callers can continue accessing display settings while ownership lives
// in SequenceConfig; shared Theme no longer contains any sequence palette.
impl Deref for Config {
    /// Sequence settings exposed for backward-compatible field access.
    type Target = SequenceConfig;
    /// Expose sequence settings through the legacy top-level configuration interface.
    fn deref(&self) -> &Self::Target {
        &self.sequence
    }
}
impl DerefMut for Config {
    /// Allow legacy callers to update the embedded sequence settings.
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

    /// Load a specific TOML file and resolve referenced palettes relative to that file.
    pub fn load_from(path: &Path) -> io::Result<Self> {
        Self::parse(&fs::read_to_string(path)?, path)
    }

    /// Merge validated settings with defaults and share the nucleotide palette with VCF.
    fn parse(text: &str, path: &Path) -> io::Result<Self> {
        let raw: RawConfig =
            toml::from_str(text).map_err(|e| invalid(format!("{}: {e}", path.display())))?;
        let mut config = Self {
            theme: Theme::default(),
            sequence: raw.display,
            vcf: raw.vcf,
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
        config.vcf.nucleotides = config.sequence.palette.nucleotides().clone();
        Ok(config)
    }
}

/// TOML sections accepted before theme values and relative palette paths are resolved.
#[derive(Default, Deserialize)]
#[serde(default, deny_unknown_fields)]
struct RawConfig {
    /// Legacy TOML section name for sequence display preferences.
    display: SequenceConfig,
    /// Optional chrome colours and external content palette path.
    theme: RawTheme,
    /// VCF-specific display and colour overrides.
    vcf: VcfConfig,
}

/// Optional theme overrides and the path to a separate sequence content palette.
#[derive(Default, Deserialize)]
#[serde(default, deny_unknown_fields)]
struct RawTheme {
    /// Palette path resolved relative to the configuration file.
    content: Option<PathBuf>,
    /// Override for the page background.
    background: Option<String>,
    /// Override for normal text.
    foreground: Option<String>,
    /// Override for secondary text and hints.
    muted: Option<String>,
    /// Override for inactive panel borders.
    border: Option<String>,
    /// Override for focused elements.
    focus: Option<String>,
    /// Override for selection backgrounds.
    selection: Option<String>,
}

/// Create a configuration validation error with `InvalidData` kind.
fn invalid(message: impl Into<String>) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, message.into())
}

/// Configuration defaults, relative palette loading and validation regressions.
#[cfg(test)]
mod tests {
    use super::*;
    use ratatui::style::Color;

    /// Verify the bundled configuration loads its palette and shares nucleotide colours with VCF.
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
            config.vcf.nucleotides.nucleotide_color(b'a'),
            config.sequence.palette.nucleotide_color(b'a')
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

    /// Check that omitted options retain defaults while explicit overrides take effect.
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
        assert_eq!(config.vcf.show_ad_bars, VcfConfig::default().show_ad_bars);
    }

    /// Check legacy palette paths remain relative to the config and supply both modules.
    #[test]
    fn legacy_content_palette_is_shared_with_vcf_and_keeps_relative_paths() {
        let directory = tempfile::tempdir().unwrap();
        let palettes = directory.path().join("palettes");
        fs::create_dir(&palettes).unwrap();
        fs::write(
            palettes.join("shared.toml"),
            "[nucleotides]\na='#123456'\nc='cyan'\nu='magenta'\nambiguous='yellow'\ngap='red'\nunknown='blue'\n[amino_acids]\nbasic='green'",
        )
        .unwrap();
        let path = directory.path().join("config.toml");
        // The old configuration format also applies the nucleotide settings to VCF.
        for extra in ["", "\n[vcf]\npass_color='#112233'\nshow_ad_bars=false\n"] {
            let config = Config::parse(
                &format!(
                    "[display]\nshow_quality=false\n[theme]\ncontent='palettes/shared.toml'\n{extra}"
                ),
                &path,
            )
            .unwrap();
            assert!(!config.sequence.show_quality);
            assert_eq!(
                config.vcf.nucleotides.nucleotide_color(b'A'),
                Color::Rgb(18, 52, 86)
            );
            assert_eq!(config.vcf.nucleotides.nucleotide_color(b'C'), Color::Cyan);
            assert_eq!(
                config.vcf.nucleotides.nucleotide_color(b'U'),
                Color::Magenta
            );
            for base in u8::MIN..=u8::MAX {
                assert_eq!(
                    config.vcf.nucleotides.nucleotide_color(base),
                    config.sequence.palette.nucleotide_color(base)
                );
            }
            assert_eq!(config.sequence.palette.amino_acid_color(b'K'), Color::Green);
            if !extra.is_empty() {
                assert_eq!(config.vcf.pass_color, Color::Rgb(17, 34, 51));
                assert!(!config.vcf.show_ad_bars);
            }
        }
    }

    /// Reject misspelled keys, cross-module settings and invalid colour values.
    #[test]
    fn vcf_settings_do_not_accept_unknown_or_sequence_fields() {
        for text in [
            "unknown=true",
            "[vcf]\npass_colr='green'",
            "[vcf]\nshow_quality=false",
            "[display]\nshow_ad_bars=false",
            "[theme]\npass_color='green'",
            "[vcf]\npass_color='not-a-colour'",
            "[vcf]\nallele_colors=[]",
        ] {
            assert!(
                Config::parse(text, Path::new("config.toml")).is_err(),
                "{text}"
            );
        }
    }

    /// Report malformed settings and missing palette files instead of silently using defaults.
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
