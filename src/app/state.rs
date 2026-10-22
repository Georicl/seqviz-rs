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

pub(crate) struct Tab {
    pub path: PathBuf,
    pub closed: bool,
    pub view: Box<dyn DocumentView>,
}
pub(crate) struct App {
    pub tabs: Vec<Tab>,
    pub active: usize,
    pub config: Config,
    pub hits: layout::Layout,
    pub redraw: bool,
    pub quit: bool,
    pub worker: Worker,
    clipboard: ClipboardService,
    factory: registry::ViewFactory,
}
impl App {
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
    pub fn view(&self) -> &dyn DocumentView {
        self.tabs[self.active].view.as_ref()
    }
    pub fn view_mut(&mut self) -> &mut dyn DocumentView {
        self.tabs[self.active].view.as_mut()
    }
    pub fn open_tabs(&self) -> Vec<usize> {
        self.tabs
            .iter()
            .enumerate()
            .filter_map(|(i, t)| (!t.closed).then_some(i))
            .collect()
    }
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
    pub fn tick(&mut self) {
        self.view_mut().tick();
        self.flush_actions();
    }
    pub fn request_view(&mut self) {
        self.view_mut().request_view();
        self.flush_actions();
    }
    pub fn needs_redraw(&self) -> bool {
        self.redraw || self.view().needs_redraw()
    }
    pub fn did_draw(&mut self) {
        self.redraw = false;
        self.view_mut().did_draw();
    }
    pub fn resized(&mut self) {
        self.redraw = true;
        self.view_mut().resized();
    }
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
    pub fn tab_hit(&self, x: u16, y: u16) -> Option<usize> {
        widgets::tabs::hit(
            &widgets::tabs::layout(self.hits.tabs, &self.open_tabs(), self.active),
            x,
            y,
        )
    }
}
impl Drop for App {
    fn drop(&mut self) {
        for tab in &mut self.tabs {
            tab.view.deactivate();
        }
    }
}
