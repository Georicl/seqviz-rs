//! Settings owned by the sequence page, decoded from the legacy [display] section.
use super::palette::ContentPalette;
use serde::Deserialize;

#[derive(Debug, Clone, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct SequenceConfig {
    pub show_quality: bool,
    #[serde(skip)]
    pub palette: ContentPalette,
}

impl Default for SequenceConfig {
    fn default() -> Self {
        Self {
            show_quality: true,
            palette: ContentPalette::default(),
        }
    }
}
