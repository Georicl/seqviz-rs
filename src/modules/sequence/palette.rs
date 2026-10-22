//! Sequence colours, reusing the shared nucleotide palette.
use crate::widgets::nucleotides::NucleotidePalette;
use crate::widgets::theme::parse_color;
use ratatui::style::Color;
use serde::Deserialize;
use std::io;

#[derive(Debug, Clone)]
pub struct ContentPalette {
    nucleotides: NucleotidePalette,
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
            nucleotides: NucleotidePalette::default(),
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
        self.nucleotides.nucleotide_color(base)
    }

    pub fn nucleotides(&self) -> &NucleotidePalette {
        &self.nucleotides
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

    pub(crate) fn apply(&mut self, text: &str) -> io::Result<()> {
        let raw: RawContent = toml::from_str(text).map_err(|e| invalid(e.to_string()))?;
        self.nucleotides.apply(raw.nucleotides)?;
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

fn invalid(message: impl Into<String>) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, message.into())
}

#[cfg(test)]
mod tests {
    use super::*;

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
