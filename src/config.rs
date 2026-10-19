//! User configuration and semantic sequence colours.

use std::{
    env, fs, io,
    path::{Path, PathBuf},
};

use ratatui::style::Color;
use serde::Deserialize;

#[derive(Debug, Clone)]
pub struct Config {
    pub show_quality: bool,
    pub theme: Theme,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            show_quality: true,
            theme: Theme::default(),
        }
    }
}

impl Config {
    /// Missing user configuration uses the built-in theme.
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
        let mut config = Self::default();
        if let Some(show) = raw.display.show_quality {
            config.show_quality = show;
        }
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
            theme
                .content
                .apply(&content_text)
                .map_err(|e| invalid(format!("{}: {e}", content_path.display())))?;
        }
        Ok(config)
    }
}

pub fn config_path() -> Option<PathBuf> {
    config_path_from(
        env::var_os("XDG_CONFIG_HOME").map(PathBuf::from),
        env::var_os("HOME").map(PathBuf::from),
    )
}

fn config_path_from(xdg: Option<PathBuf>, home: Option<PathBuf>) -> Option<PathBuf> {
    xdg.filter(|p| p.is_absolute())
        .or_else(|| home.map(|p| p.join(".config")))
        .map(|p| p.join("seqviz-rs/config.toml"))
}

#[derive(Debug, Clone)]
pub struct Theme {
    pub background: Color,
    pub foreground: Color,
    pub muted: Color,
    pub border: Color,
    pub focus: Color,
    pub selection: Color,
    pub content: ContentPalette,
}

impl Default for Theme {
    fn default() -> Self {
        Self {
            background: Color::Rgb(24, 27, 35),
            foreground: Color::Rgb(224, 229, 239),
            muted: Color::Rgb(143, 155, 179),
            border: Color::Rgb(71, 84, 110),
            focus: Color::Rgb(122, 183, 255),
            selection: Color::Rgb(62, 77, 108),
            content: ContentPalette::default(),
        }
    }
}

impl Theme {
    pub fn nucleotide_color(&self, base: u8) -> Color {
        self.content.nucleotide_color(base)
    }
    pub fn amino_acid_color(&self, residue: u8) -> Color {
        self.content.amino_acid_color(residue)
    }
    /// `score` is a decoded Phred score, not an ASCII quality character.
    pub fn quality_color(&self, score: u8) -> Color {
        self.content.quality_color(score)
    }
}

#[derive(Debug, Clone)]
pub struct ContentPalette {
    nucleotides: [Color; 8],
    amino_acids: [Color; 10],
    quality: Vec<(u8, Color)>,
}

impl Default for ContentPalette {
    fn default() -> Self {
        let green = Color::Rgb(126, 211, 151);
        let blue = Color::Rgb(122, 183, 255);
        let yellow = Color::Rgb(240, 198, 116);
        let red = Color::Rgb(241, 133, 148);
        let purple = Color::Rgb(190, 155, 243);
        let grey = Color::Rgb(143, 155, 179);
        Self {
            nucleotides: [green, blue, yellow, red, red, purple, grey, grey],
            amino_acids: [
                yellow,
                green,
                red,
                blue,
                purple,
                Color::Rgb(120, 213, 212),
                grey,
                red,
                grey,
                grey,
            ],
            quality: vec![(0, red), (10, yellow), (20, blue), (30, green)],
        }
    }
}

impl ContentPalette {
    pub fn nucleotide_color(&self, base: u8) -> Color {
        let index = match base.to_ascii_uppercase() {
            b'A' => 0,
            b'C' => 1,
            b'G' => 2,
            b'T' => 3,
            b'U' => 4,
            b'R' | b'Y' | b'S' | b'W' | b'K' | b'M' | b'B' | b'D' | b'H' | b'V' | b'N' => 5,
            b'-' | b'.' => 6,
            _ => 7,
        };
        self.nucleotides[index]
    }

    pub fn amino_acid_color(&self, residue: u8) -> Color {
        let index = match residue.to_ascii_uppercase() {
            b'A' | b'I' | b'L' | b'M' | b'V' => 0,
            b'S' | b'T' | b'N' | b'Q' | b'C' | b'U' => 1,
            b'D' | b'E' => 2,
            b'K' | b'R' | b'H' | b'O' => 3,
            b'F' | b'W' | b'Y' => 4,
            b'G' | b'P' => 5,
            b'B' | b'Z' | b'J' | b'X' => 6,
            b'*' => 7,
            b'-' | b'.' => 8,
            _ => 9,
        };
        self.amino_acids[index]
    }

    pub fn quality_color(&self, score: u8) -> Color {
        self.quality
            .iter()
            .rev()
            .find(|(min, _)| score >= *min)
            .map(|(_, color)| *color)
            .unwrap_or(self.quality[0].1)
    }

    fn apply(&mut self, text: &str) -> io::Result<()> {
        let raw: RawContent = toml::from_str(text).map_err(|e| invalid(e.to_string()))?;
        for (name, value) in raw.nucleotides {
            let index = match name.as_str() {
                "a" => 0,
                "c" => 1,
                "g" => 2,
                "t" => 3,
                "u" => 4,
                "ambiguous" => 5,
                "gap" => 6,
                "unknown" => 7,
                _ => return Err(invalid(format!("Unknown nucleotide colour: {name}"))),
            };
            self.nucleotides[index] = parse_color(&value, &format!("nucleotides.{name}"))?;
        }
        for (name, value) in raw.amino_acids {
            let index = match name.as_str() {
                "nonpolar" => 0,
                "polar" => 1,
                "acidic" => 2,
                "basic" => 3,
                "aromatic" => 4,
                "special" => 5,
                "ambiguous" => 6,
                "stop" => 7,
                "gap" => 8,
                "unknown" => 9,
                _ => return Err(invalid(format!("Unknown amino-acid colour: {name}"))),
            };
            self.amino_acids[index] = parse_color(&value, &format!("amino_acids.{name}"))?;
        }
        if let Some(thresholds) = raw.quality {
            if thresholds.is_empty() {
                return Err(invalid(
                    "quality must contain at least the min = 0 threshold",
                ));
            }
            let mut quality = thresholds
                .into_iter()
                .map(|t| Ok((t.min, parse_color(&t.color, "quality.color")?)))
                .collect::<io::Result<Vec<_>>>()?;
            quality.sort_by_key(|(min, _)| *min);
            if quality[0].0 != 0
                || quality.iter().any(|(min, _)| *min > 93)
                || quality.windows(2).any(|w| w[0].0 == w[1].0)
            {
                return Err(invalid(
                    "Quality thresholds must be unique Phred scores from 0 to 93 and include 0",
                ));
            }
            self.quality = quality;
        }
        Ok(())
    }
}

#[derive(Default, Deserialize)]
#[serde(default, deny_unknown_fields)]
struct RawConfig {
    display: RawDisplay,
    theme: RawTheme,
}
#[derive(Default, Deserialize)]
#[serde(default, deny_unknown_fields)]
struct RawDisplay {
    show_quality: Option<bool>,
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
#[derive(Default, Deserialize)]
#[serde(default, deny_unknown_fields)]
struct RawContent {
    nucleotides: std::collections::BTreeMap<String, String>,
    amino_acids: std::collections::BTreeMap<String, String>,
    quality: Option<Vec<RawThreshold>>,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RawThreshold {
    min: u8,
    color: String,
}

fn set_color(target: &mut Color, value: Option<String>, key: &str) -> io::Result<()> {
    if let Some(value) = value {
        *target = parse_color(&value, key)?;
    }
    Ok(())
}

fn parse_color(text: &str, key: &str) -> io::Result<Color> {
    text.parse::<Color>().map_err(|_| {
        invalid(format!(
            "Invalid colour for {key}: {text:?}; use a colour name or #RRGGBB"
        ))
    })
}

fn invalid(message: impl Into<String>) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, message.into())
}

#[cfg(test)]
mod tests {
    use super::*;

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

    #[test]
    fn sample_config_resolves_its_content_palette() {
        let config =
            Config::load_from(&Path::new(env!("CARGO_MANIFEST_DIR")).join("config/config.toml"))
                .unwrap();
        assert!(config.show_quality);
        assert_eq!(
            config.theme.nucleotide_color(b'a'),
            Color::Rgb(126, 211, 151)
        );
        assert_eq!(
            config.theme.amino_acid_color(b'D'),
            config.theme.amino_acid_color(b'e')
        );
        assert_ne!(
            config.theme.quality_color(29),
            config.theme.quality_color(30)
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

    #[test]
    fn custom_quality_thresholds_are_inclusive_and_sorted() {
        let mut content = ContentPalette::default();
        content
            .apply("[[quality]]\nmin=30\ncolor='green'\n[[quality]]\nmin=0\ncolor='red'")
            .unwrap();
        assert_eq!(content.quality_color(29), Color::Red);
        assert_eq!(content.quality_color(30), Color::Green);
        for bad in [
            "quality=[]",
            "[[quality]]\nmin=10\ncolor='red'",
            "[[quality]]\nmin=0\ncolor='red'\n[[quality]]\nmin=0\ncolor='blue'",
            "[[quality]]\nmin=0\ncolor='red'\n[[quality]]\nmin=94\ncolor='blue'",
        ] {
            assert!(ContentPalette::default().apply(bad).is_err());
        }
    }

    #[test]
    fn palette_retains_semantic_classes_and_lowercase() {
        let mut content = ContentPalette::default();
        content.apply("[nucleotides]\nambiguous='cyan'\nu='magenta'\n[amino_acids]\nbasic='blue'\npolar='green'\nstop='red'").unwrap();
        for base in b"ryswkmbdhvn" {
            assert_eq!(content.nucleotide_color(*base), Color::Cyan);
        }
        assert_eq!(content.nucleotide_color(b'u'), Color::Magenta);
        assert_eq!(content.amino_acid_color(b'o'), Color::Blue);
        assert_eq!(content.amino_acid_color(b'u'), Color::Green);
        assert_eq!(content.amino_acid_color(b'*'), Color::Red);
    }
}
