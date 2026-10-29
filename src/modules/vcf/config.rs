//! Display settings and semantic colours for the VCF page.
use crate::widgets::{nucleotides::NucleotidePalette, theme::parse_color};
use ratatui::style::Color;
use serde::{Deserialize, Deserializer, de::Error};

/// VCF display options loaded from the application configuration.
#[derive(Clone, Debug, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct VcfConfig {
    /// Shared nucleotide colours supplied by application configuration.
    #[serde(skip)]
    pub nucleotides: NucleotidePalette,
    /// Colour for records whose FILTER field is PASS.
    #[serde(deserialize_with = "deserialize_color")]
    pub pass_color: Color,
    /// Colour for named non-PASS filters.
    #[serde(deserialize_with = "deserialize_color")]
    pub filter_color: Color,
    /// Colour for missing values represented by a dot.
    #[serde(deserialize_with = "deserialize_color")]
    pub missing_color: Color,
    /// Cyclic colours for one-based ALT indices; REF uses the theme foreground.
    #[serde(deserialize_with = "deserialize_colors")]
    pub allele_colors: Vec<Color>,
    /// Whether sample details may append proportional allele-depth bars.
    pub show_ad_bars: bool,
    /// Preferred total height of the detail panel, including its border and headings.
    pub detail_rows: u16,
}

impl Default for VcfConfig {
    /// Use the default nucleotide/allele palette and a ten-row detail panel.
    fn default() -> Self {
        Self {
            nucleotides: NucleotidePalette::default(),
            pass_color: Color::Rgb(126, 211, 151),
            filter_color: Color::Rgb(240, 198, 116),
            missing_color: Color::Rgb(143, 155, 179),
            allele_colors: vec![
                Color::Rgb(122, 183, 255),
                Color::Rgb(190, 155, 243),
                Color::Rgb(120, 213, 212),
                Color::Rgb(240, 198, 116),
                Color::Rgb(241, 133, 148),
            ],
            show_ad_bars: true,
            detail_rows: 10,
        }
    }
}

/// Parse one configured terminal colour and report a serde error on invalid input.
fn deserialize_color<'de, D: Deserializer<'de>>(deserializer: D) -> Result<Color, D::Error> {
    let text = String::deserialize(deserializer)?;
    parse_color(&text, "vcf colour").map_err(D::Error::custom)
}

/// Parse a nonempty list of colours used cyclically for ALT allele indices.
fn deserialize_colors<'de, D: Deserializer<'de>>(deserializer: D) -> Result<Vec<Color>, D::Error> {
    let values = Vec::<String>::deserialize(deserializer)?;
    if values.is_empty() {
        return Err(D::Error::custom("vcf.allele_colors must not be empty"));
    }
    values
        .into_iter()
        .map(|value| parse_color(&value, "vcf.allele_colors").map_err(D::Error::custom))
        .collect()
}

/// Validate VCF colour configuration and rejection of unsupported settings.
#[cfg(test)]
mod tests {
    use super::*;

    /// Check colour overrides, empty palettes, unknown fields and invalid colour names.
    #[test]
    fn colours_can_be_configured_and_invalid_values_are_rejected() {
        let config: VcfConfig = toml::from_str(
            "pass_color='#112233'\nallele_colors=['blue','magenta']\nshow_ad_bars=false",
        )
        .unwrap();
        assert_eq!(config.pass_color, Color::Rgb(17, 34, 51));
        assert_eq!(config.allele_colors, [Color::Blue, Color::Magenta]);
        assert!(!config.show_ad_bars);
        for text in [
            "allele_colors=[]",
            "pass_color='no-such-colour'",
            "unknown=true",
        ] {
            assert!(toml::from_str::<VcfConfig>(text).is_err());
        }
    }
}
