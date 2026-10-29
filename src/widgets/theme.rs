//! Colours shared by application chrome and reusable widgets.
use ratatui::style::Color;
use std::io;

/// Application chrome colours, separate from biological content palettes.
#[derive(Debug, Clone)]
pub struct Theme {
    /// Default page background.
    pub background: Color,
    /// Normal text colour.
    pub foreground: Color,
    /// Secondary text and shortcut hint colour.
    pub muted: Color,
    /// Inactive panel border colour.
    pub border: Color,
    /// Focused border, label and cursor-related highlight colour.
    pub focus: Color,
    /// Background colour for selected content and the active tab.
    pub selection: Color,
}

impl Default for Theme {
    /// Use the built-in dark background, light foreground and blue focus colours.
    fn default() -> Self {
        Self {
            background: Color::Rgb(24, 27, 35),
            foreground: Color::Rgb(224, 229, 239),
            muted: Color::Rgb(143, 155, 179),
            border: Color::Rgb(71, 84, 110),
            focus: Color::Rgb(122, 183, 255),
            selection: Color::Rgb(62, 77, 108),
        }
    }
}

/// Replace a colour only when an override is present, preserving defaults otherwise.
pub(crate) fn set_color(target: &mut Color, value: Option<String>, key: &str) -> io::Result<()> {
    if let Some(value) = value {
        *target = parse_color(&value, key)?;
    }
    Ok(())
}

/// Parse a terminal colour and include the configuration key in validation errors.
pub(crate) fn parse_color(text: &str, key: &str) -> io::Result<Color> {
    text.parse::<Color>().map_err(|_| {
        io::Error::new(
            io::ErrorKind::InvalidData,
            format!("Invalid colour for {key}: {text:?}; use a colour name or #RRGGBB"),
        )
    })
}
