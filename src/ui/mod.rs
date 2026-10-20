mod app;
mod events;
mod layout;
mod panels;
mod render;

use crate::{
    config::{self, Config},
    worker::Worker,
};
use app::App;
use crossterm::{
    event::{self, DisableMouseCapture, EnableMouseCapture, Event, KeyEventKind},
    execute,
    terminal::{EnterAlternateScreen, LeaveAlternateScreen, disable_raw_mode, enable_raw_mode},
};
use ratatui::{Terminal, backend::CrosstermBackend, layout::Rect};
use std::{
    io,
    path::PathBuf,
    time::{Duration, Instant},
};

struct TerminalGuard;
impl Drop for TerminalGuard {
    fn drop(&mut self) {
        let _ = disable_raw_mode();
        let _ = execute!(io::stdout(), DisableMouseCapture, LeaveAlternateScreen);
    }
}

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
            app.drag_scroll();
            last_drag = Instant::now();
        }
        let size = terminal.size()?;
        app.prepare_frame(Rect::new(0, 0, size.width, size.height));
        if app.dirty {
            app.request_view();
        }
        if app.redraw {
            terminal.draw(|frame| render::draw(frame, &app))?;
            app.redraw = false;
        }
        if event::poll(Duration::from_millis(30))? {
            match event::read()? {
                Event::Key(key) if key.kind != KeyEventKind::Release => app.key(key),
                Event::Mouse(mouse) => app.mouse(mouse),
                Event::Resize(_, _) => {
                    app.dirty = true;
                    app.redraw = true;
                    app.info_selection = None;
                }
                _ => {}
            }
        }
    }
    app.cancel_operation();
    Ok(())
}

#[cfg(test)]
mod tests;
