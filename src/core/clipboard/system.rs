//! System-command I/O belongs to the clipboard service, never the UI thread.
use std::{
    env,
    io::{self, Write},
    process::{Command, Stdio},
    sync::atomic::{AtomicBool, Ordering},
    thread,
    time::Duration,
};

/// Create the interrupted error used to stop clipboard work without a fallback.
pub(super) fn cancelled() -> io::Error {
    io::Error::new(io::ErrorKind::Interrupted, "Copy cancelled")
}

/// Try platform clipboard tools in order until one succeeds or cancellation is requested.
pub(super) fn copy(text: &str, cancel: &AtomicBool) -> io::Result<()> {
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
        if cancel.load(Ordering::Relaxed) {
            return Err(cancelled());
        }
        match run_command(command, args, text, cancel) {
            Ok(()) => return Ok(()),
            Err(error) if error.kind() == io::ErrorKind::Interrupted => return Err(error),
            Err(error) => last_error = error,
        }
    }
    Err(last_error)
}

/// Write text through a scoped pipe writer while polling and cancelling the child process.
fn run_command(command: &str, args: &[&str], text: &str, cancel: &AtomicBool) -> io::Result<()> {
    let mut child = Command::new(command)
        .args(args)
        .stdin(Stdio::piped())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()?;
    let mut input = child.stdin.take().expect("piped clipboard stdin");
    // A separate writer lets this service kill a cancelled command even if its
    // pipe is full. Killing the child releases write_all before the scope joins.
    thread::scope(|scope| {
        let writer = scope.spawn(move || input.write_all(text.as_bytes()));
        let status = loop {
            if cancel.load(Ordering::Relaxed) {
                let _ = child.kill();
                let _ = child.wait();
                let _ = writer.join();
                return Err(cancelled());
            }
            match child.try_wait() {
                Ok(Some(status)) => break status,
                Ok(None) => thread::sleep(Duration::from_millis(10)),
                Err(error) => {
                    let _ = child.kill();
                    let _ = child.wait();
                    let _ = writer.join();
                    return Err(error);
                }
            }
        };
        writer
            .join()
            .map_err(|_| io::Error::other("Clipboard writer stopped"))??;
        if status.success() {
            Ok(())
        } else {
            Err(io::Error::other(format!("{command} exited with {status}")))
        }
    })
}
