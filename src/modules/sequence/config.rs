//! Settings owned by the sequence page, decoded from the legacy `[display]` section.
use super::palette::ContentPalette;
use serde::Deserialize;

/// Display preferences for sequence documents, independent of the shared application theme.
#[derive(Debug, Clone, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct SequenceConfig {
    /// Show an aligned quality-character row when a record has quality data.
    pub show_quality: bool,
    /// Sequence colours loaded separately from the display settings.
    #[serde(skip)]
    pub palette: ContentPalette,
}

impl Default for SequenceConfig {
    /// Show quality rows and use the built-in sequence colour palette.
    fn default() -> Self {
        Self {
            show_quality: true,
            palette: ContentPalette::default(),
        }
    }
}
