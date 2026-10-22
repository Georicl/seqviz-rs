//! Shared nucleotide colours, independent of sequence or variant document types.
use super::theme::parse_color;
use ratatui::style::Color;
use std::io;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NucleotidePalette {
    colors: [Color; 8],
}

impl Default for NucleotidePalette {
    fn default() -> Self {
        Self {
            colors: [
                Color::Rgb(126, 211, 151),
                Color::Rgb(122, 183, 255),
                Color::Rgb(240, 198, 116),
                Color::Rgb(241, 133, 148),
                Color::Rgb(241, 133, 148),
                Color::Rgb(190, 155, 243),
                Color::Rgb(143, 155, 179),
                Color::Rgb(143, 155, 179),
            ],
        }
    }
}

impl NucleotidePalette {
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
        self.colors[index]
    }

    pub(crate) fn apply(
        &mut self,
        colors: impl IntoIterator<Item = (String, String)>,
    ) -> io::Result<()> {
        for (name, value) in colors {
            let index = match name.as_str() {
                "a" => 0,
                "c" => 1,
                "g" => 2,
                "t" => 3,
                "u" => 4,
                "ambiguous" => 5,
                "gap" => 6,
                "unknown" => 7,
                _ => {
                    return Err(io::Error::new(
                        io::ErrorKind::InvalidData,
                        format!("Unknown nucleotide colour: {name}"),
                    ));
                }
            };
            self.colors[index] = parse_color(&value, &format!("nucleotides.{name}"))?;
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn preserves_nucleotide_defaults_and_semantic_classes() {
        let palette = NucleotidePalette::default();
        for (base, expected) in [
            (b'a', Color::Rgb(126, 211, 151)),
            (b'c', Color::Rgb(122, 183, 255)),
            (b'g', Color::Rgb(240, 198, 116)),
            (b't', Color::Rgb(241, 133, 148)),
            (b'u', Color::Rgb(241, 133, 148)),
            (b'n', Color::Rgb(190, 155, 243)),
            (b'-', Color::Rgb(143, 155, 179)),
            (b'?', Color::Rgb(143, 155, 179)),
        ] {
            assert_eq!(palette.nucleotide_color(base), expected);
            assert_eq!(
                palette.nucleotide_color(base.to_ascii_uppercase()),
                expected
            );
        }
        for base in b"RYSWKMBDHVNryswkmbdhvn" {
            assert_eq!(
                palette.nucleotide_color(*base),
                palette.nucleotide_color(b'N')
            );
        }
        assert_eq!(
            palette.nucleotide_color(b'.'),
            palette.nucleotide_color(b'-')
        );
    }

    #[test]
    fn rejects_unknown_nucleotide_settings_and_invalid_colors() {
        for (name, value) in [("a", "not-a-colour"), ("adenine", "green")] {
            assert!(
                NucleotidePalette::default()
                    .apply([(name.to_owned(), value.to_owned())])
                    .is_err()
            );
        }
    }
}
