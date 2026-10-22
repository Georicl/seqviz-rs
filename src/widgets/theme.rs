//! Colours shared by application chrome and reusable widgets.
use ratatui::style::Color;
use std::io;

#[derive(Debug, Clone)]
pub struct Theme {
    pub background: Color,
    pub foreground: Color,
    pub muted: Color,
    pub border: Color,
    pub focus: Color,
    pub selection: Color,
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
        }
    }
}

pub(crate) fn set_color(target: &mut Color, value: Option<String>, key: &str) -> io::Result<()> {
    if let Some(value) = value {
        *target = parse_color(&value, key)?;
    }
    Ok(())
}

pub(crate) fn parse_color(text: &str, key: &str) -> io::Result<Color> {
    text.parse::<Color>().map_err(|_| {
        io::Error::new(
            io::ErrorKind::InvalidData,
            format!("Invalid colour for {key}: {text:?}; use a colour name or #RRGGBB"),
        )
    })
}
