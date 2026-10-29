//! Application-global tabs, document routing and service result lifetimes.
use super::{config::Config, layout};
use crate::{
    core::clipboard::{ClipboardOutput, ClipboardService},
    modules::{
        protocol::ModuleUpdate,
        registry,
        session::Worker,
        view::{Action, DocumentView},
    },
    widgets,
};
use ratatui::layout::Rect;
use std::{
    path::PathBuf,
    sync::{
        Arc,
        atomic::{AtomicU64, Ordering},
    },
};

/// A stable file slot with an independent document view, even for alias paths.
pub(crate) struct Tab {
    /// Original input path used for display and format probing.
    pub path: PathBuf,
    /// Whether this stable tab slot has been closed.
    pub closed: bool,
    /// Independent page state, including for tabs sharing a dataset.
    pub view: Box<dyn DocumentView>,
}
/// Owns tab navigation, document views, worker routing and clipboard delivery.
pub(crate) struct App {
    /// Stable input slots; closed tabs remain in the vector.
    pub tabs: Vec<Tab>,
    /// Index of the currently displayed open tab.
    pub active: usize,
    /// Resolved application and module settings.
    pub config: Config,
    /// Current terminal regions for drawing and mouse hit testing.
    pub hits: layout::Layout,
    /// Whether application chrome needs repainting.
    pub redraw: bool,
    /// Whether the terminal loop should exit.
    pub quit: bool,
    /// Background session shared by all tabs.
    pub worker: Worker,
    /// Off-thread clipboard preparation and system-copy service.
    clipboard: ClipboardService,
    /// Creates loading and domain pages with shared preferences and operation IDs.
    factory: registry::ViewFactory,
}
impl App {
    /// Create loading views in input order and share theme settings and operation IDs.
    pub fn new(paths: Vec<PathBuf>, config: Config, worker: Worker) -> Self {
        let clock = Arc::new(AtomicU64::new(0));
        let factory = registry::ViewFactory::new(
            config.theme.clone(),
            config.sequence.clone(),
            config.vcf.clone(),
            clock,
        );
        let tabs = paths
            .into_iter()
            .enumerate()
            .map(|(file, path)| Tab {
                view: factory.loading(path.clone(), file),
                path,
                closed: false,
            })
            .collect();
        Self {
            tabs,
            active: 0,
            config,
            hits: layout::Layout::default(),
            redraw: true,
            quit: false,
            worker,
            clipboard: ClipboardService::start(),
            factory,
        }
    }
    /// Borrow the active tab's document interface.
    pub fn view(&self) -> &dyn DocumentView {
        self.tabs[self.active].view.as_ref()
    }
    /// Mutably borrow the active tab's document interface.
    pub fn view_mut(&mut self) -> &mut dyn DocumentView {
        self.tabs[self.active].view.as_mut()
    }
    /// Return open tab IDs in input order without renumbering closed slots.
    pub fn open_tabs(&self) -> Vec<usize> {
        self.tabs
            .iter()
            .enumerate()
            .filter_map(|(i, t)| (!t.closed).then_some(i))
            .collect()
    }
    /// Recalculate application regions and prepare the active page when it fits.
    pub fn prepare_frame(&mut self, area: Rect) {
        let next = layout::calculate(area);
        if self.hits.area != area {
            self.redraw = true;
        }
        self.hits = next;
        if !self.hits.too_small {
            self.view_mut().prepare(next.body);
        }
    }
    /// Drain active-page actions into the worker, cancellation flag and clipboard queue.
    pub fn flush_actions(&mut self) {
        let actions = self.view_mut().actions();
        for action in actions {
            match action {
                Action::Request(request) => self.worker.dispatch(request),
                Action::BackgroundCancelled(value) => self
                    .worker
                    .cancel_background
                    .store(value, Ordering::Relaxed),
                Action::Clipboard {
                    operation_id,
                    file,
                    text,
                    allowed,
                    cancel,
                } => {
                    if let Err(error) =
                        self.clipboard
                            .submit(operation_id, file, text, allowed, cancel)
                    {
                        self.tabs[file]
                            .view
                            .copy_finished(operation_id, error.to_string());
                    }
                }
            }
        }
    }
    /// Route pending worker replies and accept clipboard results only for the current operation.
    pub fn receive(&mut self) {
        while let Some(update) = self.worker.try_update() {
            let Some(file) = update.file() else {
                continue;
            };
            if let Some(tab) = self.tabs.get_mut(file).filter(|t| !t.closed) {
                if let ModuleUpdate::Opened { module, .. } = update {
                    let mut view = self.factory.create(tab.path.clone(), file, module);
                    view.activate();
                    if file != self.active {
                        view.deactivate();
                    }
                    view.prepare(self.hits.body);
                    tab.view = view;
                    self.redraw = true;
                } else {
                    tab.view.update(update);
                }
            }
        }
        while let Ok(update) = self.clipboard.updates.try_recv() {
            let file = update.file;
            if file != self.active
                || self.tabs[file].closed
                || !self.tabs[file].view.accepts_copy(update.operation_id)
            {
                continue;
            }
            let message = match update.result {
                Ok(ClipboardOutput::System) => "Copied to clipboard".into(),
                Ok(ClipboardOutput::Osc52(payload)) if self.tabs[file].view.can_copy() => {
                    match super::terminal::write_osc52(&payload) {
                        Ok(()) => "Clipboard request sent to terminal (OSC 52)".into(),
                        Err(error) => error.to_string(),
                    }
                }
                Ok(ClipboardOutput::Osc52(_)) => "Copy is disabled for this document".into(),
                Err(error) => error.to_string(),
            };
            self.tabs[file]
                .view
                .copy_finished(update.operation_id, message);
        }
        self.flush_actions();
    }
    /// Deactivate the old page, flush its actions and activate a valid open destination.
    pub fn switch_tab(&mut self, next: usize) {
        if next == self.active || next >= self.tabs.len() || self.tabs[next].closed {
            return;
        }
        self.view_mut().deactivate();
        self.flush_actions();
        self.active = next;
        self.view_mut().activate();
        self.redraw = true;
    }
    /// Move to the next open tab in the chosen direction, wrapping at either end.
    pub fn cycle_tab(&mut self, backwards: bool) {
        let n = self.tabs.len();
        for step in 1..=n {
            let id = if backwards {
                (self.active + n - step % n) % n
            } else {
                (self.active + step) % n
            };
            if !self.tabs[id].closed {
                self.switch_tab(id);
                break;
            }
        }
    }
    /// Cancel and close the active tab, then select another tab or end the session.
    pub fn close_tab(&mut self) {
        self.view_mut().deactivate();
        self.flush_actions();
        self.worker.close(self.active);
        self.tabs[self.active].closed = true;
        if self.tabs.iter().all(|t| t.closed) {
            self.quit = true;
        } else {
            self.cycle_tab(false);
        }
    }
    /// Advance active-page interaction timers and dispatch resulting actions.
    pub fn tick(&mut self) {
        self.view_mut().tick();
        self.flush_actions();
    }
    /// Ask the active page for a fresh snapshot and dispatch its request.
    pub fn request_view(&mut self) {
        self.view_mut().request_view();
        self.flush_actions();
    }
    /// Report whether either application chrome or the active page changed.
    pub fn needs_redraw(&self) -> bool {
        self.redraw || self.view().needs_redraw()
    }
    /// Clear application and active-page redraw flags after a completed frame.
    pub fn did_draw(&mut self) {
        self.redraw = false;
        self.view_mut().did_draw();
    }
    /// Invalidate both application chrome and the active page after terminal resizing.
    pub fn resized(&mut self) {
        self.redraw = true;
        self.view_mut().resized();
    }
    /// Pair open tab IDs with lossy UTF-8 file names for the tab bar.
    pub fn labels(&self) -> Vec<(usize, String)> {
        self.open_tabs()
            .into_iter()
            .map(|i| {
                let name = self.tabs[i]
                    .path
                    .file_name()
                    .unwrap_or_default()
                    .to_string_lossy();
                (i, name.into_owned())
            })
            .collect()
    }
    /// Map terminal coordinates to the visible tab ID using the rendering geometry.
    pub fn tab_hit(&self, x: u16, y: u16) -> Option<usize> {
        widgets::tabs::hit(
            &widgets::tabs::layout(self.hits.tabs, &self.open_tabs(), self.active),
            x,
            y,
        )
    }
}
impl Drop for App {
    /// Deactivate every page so pending operations are cancelled before services are dropped.
    fn drop(&mut self) {
        for tab in &mut self.tabs {
            tab.view.deactivate();
        }
    }
}
