//! Clipboard preparation runs off the UI thread; only the UI writes terminal escapes.
mod service;
mod system;

use base64::{Engine, engine::general_purpose::STANDARD};
pub use service::{ClipboardService, ClipboardUpdate};

#[derive(Debug, PartialEq, Eq)]
pub enum ClipboardOutput {
    System,
    /// Already encoded off-thread. The UI must validate operation identity before writing it.
    Osc52(String),
}

fn osc52_sequence(text: &str) -> String {
    format!("\x1b]52;c;{}\x07", STANDARD.encode(text.as_bytes()))
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn osc52_encodes_exact_utf8_and_control_characters_as_data() {
        let text = ">记录\nACGT\x1b]0;not a title\x07";
        let output = osc52_sequence(text);
        let payload = output
            .strip_prefix("\x1b]52;c;")
            .unwrap()
            .strip_suffix('\x07')
            .unwrap();
        assert_eq!(STANDARD.decode(payload).unwrap(), text.as_bytes());
        assert!(!payload.contains('\x1b'));
    }
}
