//! The single owner of terminal mode and serialized terminal output.
use super::{
    config::{self, Config},
    render,
    state::App,
};
use crate::modules::session::Worker;
use crossterm::{
    event::{self, DisableMouseCapture, EnableMouseCapture, Event, KeyEventKind},
    execute,
    terminal::{EnterAlternateScreen, LeaveAlternateScreen, disable_raw_mode, enable_raw_mode},
};
use ratatui::{Terminal, backend::CrosstermBackend, layout::Rect};
use std::{
    io::{self, IsTerminal, Write},
    path::PathBuf,
    time::{Duration, Instant},
};

/// Restores cooked input, mouse reporting and the main screen when the session exits.
struct TerminalGuard;
impl Drop for TerminalGuard {
    /// Attempt terminal restoration even when the event loop returns an error.
    fn drop(&mut self) {
        let _ = disable_raw_mode();
        let _ = execute!(io::stdout(), DisableMouseCapture, LeaveAlternateScreen);
    }
}

/// Own the terminal session and interleave input, worker updates, timed ticks and redraws.
pub fn run(paths: Vec<PathBuf>, config: Config) -> io::Result<()> {
    let worker = Worker::start(paths.clone(), config::cache_directory());
    let mut app = App::new(paths, config, worker);
    enable_raw_mode()?;
    let _guard = TerminalGuard;
    execute!(io::stdout(), EnterAlternateScreen, EnableMouseCapture)?;
    let mut terminal = Terminal::new(CrosstermBackend::new(io::stdout()))?;
    let mut last_drag = Instant::now();
    while !app.quit {
        app.receive();
        if last_drag.elapsed() >= Duration::from_millis(60) {
            app.tick();
            last_drag = Instant::now();
        }
        let size = terminal.size()?;
        app.prepare_frame(Rect::new(0, 0, size.width, size.height));
        if app.view().is_dirty() {
            app.request_view();
        }
        if app.needs_redraw() {
            terminal.draw(|frame| render::draw(frame, &app))?;
            app.did_draw();
        }
        if event::poll(Duration::from_millis(30))? {
            match event::read()? {
                Event::Key(key) if key.kind != KeyEventKind::Release => app.key(key),
                Event::Mouse(mouse) => app.mouse(mouse),
                Event::Resize(_, _) => {
                    app.resized();
                }
                _ => {}
            }
        }
    }
    app.view_mut().deactivate();
    Ok(())
}

/// Write a prepared clipboard escape on the UI thread and flush terminal output.
pub fn write_osc52(payload: &str) -> io::Result<()> {
    let mut output = io::stdout().lock();
    if !output.is_terminal() {
        return Err(io::Error::new(
            io::ErrorKind::NotConnected,
            "OSC 52 clipboard output requires a terminal",
        ));
    }
    output.write_all(payload.as_bytes())?;
    output.flush()
}
/// Read stdout's terminal width, falling back to 80 columns when unavailable.
pub fn terminal_width() -> usize {
    terminal_size::terminal_size_of(io::stdout())
        .map(|(terminal_size::Width(width), _)| usize::from(width))
        .filter(|&width| width > 0)
        .unwrap_or(80)
}
