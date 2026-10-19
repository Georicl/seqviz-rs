//! Clipboard output shared by record, sequence and information views.

use std::{
    env,
    io::{self, IsTerminal, Write},
    process::{Command, Stdio},
};

use base64::{Engine, engine::general_purpose::STANDARD};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CopyMethod {
    System,
    /// The escape sequence was sent; terminal clipboard acceptance is not acknowledged.
    Osc52,
}

/// Every call must supply the active file's copy permission, including info-panel copies.
/// SAM/BAM callers must pass `false` before attempting to read/format copied content.
pub fn copy_text(text: &str, copy_allowed: bool) -> io::Result<CopyMethod> {
    copy_with(
        text,
        copy_allowed,
        env::var_os("SSH_CONNECTION").is_some() || env::var_os("SSH_TTY").is_some(),
        copy_system,
        |text| {
            let mut output = io::stdout().lock();
            if !output.is_terminal() {
                return Err(io::Error::new(
                    io::ErrorKind::NotConnected,
                    "OSC 52 clipboard output requires a terminal",
                ));
            }
            output.write_all(osc52_sequence(text).as_bytes())?;
            output.flush()
        },
    )
}

fn copy_with(
    text: &str,
    allowed: bool,
    ssh: bool,
    mut system: impl FnMut(&str) -> io::Result<()>,
    mut osc52: impl FnMut(&str) -> io::Result<()>,
) -> io::Result<CopyMethod> {
    if !allowed {
        return Err(io::Error::new(
            io::ErrorKind::PermissionDenied,
            "Copying is disabled for SAM/BAM files",
        ));
    }
    if !ssh && system(text).is_ok() {
        return Ok(CopyMethod::System);
    }
    osc52(text)
        .map_err(|error| io::Error::new(error.kind(), format!("Clipboard unavailable: {error}")))?;
    Ok(CopyMethod::Osc52)
}

fn osc52_sequence(text: &str) -> String {
    format!("\x1b]52;c;{}\x07", STANDARD.encode(text.as_bytes()))
}

fn copy_system(text: &str) -> io::Result<()> {
    let commands: &[(&str, &[&str])] = match env::consts::OS {
        "macos" => &[("pbcopy", &[])],
        "linux" => &[
            ("wl-copy", &[]),
            ("xclip", &["-selection", "clipboard"]),
            ("xsel", &["--clipboard", "--input"]),
        ],
        _ => &[],
    };
    let mut last_error = io::Error::new(
        io::ErrorKind::NotFound,
        "No system clipboard tool is available",
    );
    for (command, args) in commands {
        match run_clipboard_command(command, args, text) {
            Ok(()) => return Ok(()),
            Err(error) => last_error = error,
        }
    }
    Err(last_error)
}

fn run_clipboard_command(command: &str, args: &[&str], text: &str) -> io::Result<()> {
    let mut child = Command::new(command)
        .args(args)
        .stdin(Stdio::piped())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()?;
    let result = child
        .stdin
        .take()
        .expect("piped clipboard stdin")
        .write_all(text.as_bytes());
    if let Err(error) = result {
        let _ = child.kill();
        let _ = child.wait();
        return Err(error);
    }
    let status = child.wait()?;
    if status.success() {
        Ok(())
    } else {
        Err(io::Error::other(format!("{command} exited with {status}")))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn disallowed_copy_never_calls_any_backend() {
        for text in [">record\nACGT\n", "selected bases", "Help and statistics"] {
            let error = copy_with(
                text,
                false,
                false,
                |_| panic!("system clipboard was called"),
                |_| panic!("OSC 52 was called"),
            )
            .unwrap_err();
            assert_eq!(error.kind(), io::ErrorKind::PermissionDenied);
        }
    }

    #[test]
    fn ssh_only_uses_the_remote_terminal_clipboard() {
        let method = copy_with(
            "ACGT",
            true,
            true,
            |_| panic!("SSH must not use the server clipboard"),
            |text| {
                assert_eq!(text, "ACGT");
                Ok(())
            },
        )
        .unwrap();
        assert_eq!(method, CopyMethod::Osc52);
    }

    #[test]
    fn local_clipboard_success_and_fallback_are_distinct() {
        assert_eq!(
            copy_with(
                "AC",
                true,
                false,
                |_| Ok(()),
                |_| panic!("no fallback needed")
            )
            .unwrap(),
            CopyMethod::System
        );
        assert_eq!(
            copy_with(
                "AC",
                true,
                false,
                |_| Err(io::ErrorKind::NotFound.into()),
                |_| Ok(())
            )
            .unwrap(),
            CopyMethod::Osc52
        );
        assert!(
            copy_with(
                "AC",
                true,
                false,
                |_| Err(io::ErrorKind::NotFound.into()),
                |_| Err(io::ErrorKind::BrokenPipe.into())
            )
            .is_err()
        );
    }

    #[test]
    fn osc52_encodes_exact_utf8_and_control_characters_as_data() {
        let text = ">记录\nACGT\x1b]0;not a title\x07";
        let output = osc52_sequence(text);
        assert!(output.starts_with("\x1b]52;c;"));
        assert!(output.ends_with('\x07'));
        let payload = output
            .strip_prefix("\x1b]52;c;")
            .unwrap()
            .strip_suffix('\x07')
            .unwrap();
        assert_eq!(STANDARD.decode(payload).unwrap(), text.as_bytes());
        assert!(!payload.contains('\x1b'));
    }
}
