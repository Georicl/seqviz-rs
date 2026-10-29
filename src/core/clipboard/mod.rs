//! Clipboard preparation runs off the UI thread; only the UI writes terminal escapes.
/// Bounded background clipboard queue and operation-aware result delivery.
mod service;
/// System-command I/O belongs to the clipboard service, never the UI thread.
mod system;

use base64::{Engine, engine::general_purpose::STANDARD};
pub use service::{ClipboardService, ClipboardUpdate};

/// Result of clipboard preparation: a completed system copy or a terminal payload.
#[derive(Debug, PartialEq, Eq)]
pub enum ClipboardOutput {
    /// Text has already been delivered to a system clipboard command.
    System,
    /// Already encoded off-thread. The UI must validate operation identity before writing it.
    Osc52(String),
}

/// Encode UTF-8 bytes as base64 in an OSC 52 clipboard request.
fn osc52_sequence(text: &str) -> String {
    format!("\x1b]52;c;{}\x07", STANDARD.encode(text.as_bytes()))
}

/// OSC 52 encoding tests for Unicode and embedded control characters.
#[cfg(test)]
mod tests {
    use super::*;
    /// Check that Unicode and control characters remain encoded data inside OSC 52.
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
