//! A document can be hosted before its format has been inspected by the worker.
use super::{
    protocol::{ModuleRequest, ModuleUpdate},
    view::{Action, DocumentView, InputPrompt, Status},
};
use crate::widgets::theme::Theme;
use crossterm::event::{KeyEvent, MouseEvent};
use ratatui::{Frame, layout::Rect, style::Style, widgets::Paragraph};
use std::path::PathBuf;

pub(crate) struct LoadingView {
    path: PathBuf,
    file: usize,
    theme: Theme,
    area: Rect,
    sent: bool,
    failed: bool,
    enabled: bool,
    redraw: bool,
    message: String,
    actions: Vec<Action>,
}

impl LoadingView {
    pub(crate) fn new(path: PathBuf, file: usize, theme: Theme) -> Self {
        Self {
            path,
            file,
            theme,
            area: Rect::default(),
            sent: false,
            failed: false,
            enabled: file == 0,
            redraw: true,
            message: "Opening…".into(),
            actions: Vec::new(),
        }
    }
}

impl DocumentView for LoadingView {
    fn prepare(&mut self, area: Rect) {
        self.area = area;
    }
    fn draw(&self, frame: &mut Frame) {
        frame.render_widget(
            Paragraph::new(format!("{}\n\n{}", self.path.display(), self.message))
                .style(Style::default().fg(self.theme.muted)),
            self.area,
        );
    }
    fn key(&mut self, _: KeyEvent) {}
    fn mouse(&mut self, _: MouseEvent) {}
    fn tick(&mut self) {}
    fn captures_input(&self) -> bool {
        false
    }
    fn captures_tab_mouse(&self) -> bool {
        false
    }
    fn is_dirty(&self) -> bool {
        self.enabled && !self.sent
    }
    fn needs_redraw(&self) -> bool {
        self.redraw
    }
    fn did_draw(&mut self) {
        self.redraw = false;
    }
    fn request_view(&mut self) {
        if self.enabled && !self.sent {
            self.actions
                .push(Action::Request(ModuleRequest::Open { file: self.file }));
            self.sent = true;
        }
    }
    fn update(&mut self, update: ModuleUpdate) {
        if let ModuleUpdate::OpenFailed { file, text } = update
            && file == self.file
        {
            self.message = text;
            self.failed = true;
            self.redraw = true;
        }
    }
    fn actions(&mut self) -> Vec<Action> {
        std::mem::take(&mut self.actions)
    }
    fn activate(&mut self) {
        if self.failed {
            self.sent = false;
            self.failed = false;
            self.message = "Opening…".into();
        }
        self.enabled = true;
        self.redraw = true;
    }
    fn deactivate(&mut self) {
        self.enabled = false;
    }
    fn status(&self) -> Status {
        Status {
            message: self.message.clone(),
            hints: "Tab file · x close · q quit".into(),
        }
    }
    fn input_prompt(&self) -> Option<InputPrompt<'_>> {
        None
    }
    fn can_copy(&self) -> bool {
        false
    }
    fn accepts_copy(&self, _: u64) -> bool {
        false
    }
    fn copy_finished(&mut self, _: u64, _: String) {}
    fn resized(&mut self) {
        self.redraw = true;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn open_requests(view: &mut LoadingView) -> Vec<usize> {
        view.actions()
            .into_iter()
            .filter_map(|action| match action {
                Action::Request(ModuleRequest::Open { file }) => Some(file),
                _ => None,
            })
            .collect()
    }

    #[test]
    fn initial_activation_and_pending_reactivation_send_only_one_open() {
        let mut view = LoadingView::new("pending.vcf".into(), 1, Theme::default());
        assert!(!view.is_dirty());
        view.activate();
        assert!(view.is_dirty());
        view.request_view();
        assert_eq!(open_requests(&mut view), [1]);
        view.deactivate();
        view.activate();
        view.request_view();
        assert!(open_requests(&mut view).is_empty());
        assert!(!view.is_dirty());
        assert_eq!(view.message, "Opening…");
    }

    #[test]
    fn only_matching_failure_retries_on_reactivation() {
        let mut view = LoadingView::new("failed.vcf".into(), 0, Theme::default());
        view.request_view();
        assert_eq!(open_requests(&mut view), [0]);
        view.update(ModuleUpdate::OpenFailed {
            file: 1,
            text: "other document".into(),
        });
        assert!(!view.failed);
        assert_eq!(view.message, "Opening…");
        view.update(ModuleUpdate::OpenFailed {
            file: 0,
            text: "source missing".into(),
        });
        assert!(view.failed);
        assert_eq!(view.message, "source missing");
        view.request_view();
        assert!(open_requests(&mut view).is_empty());
        view.deactivate();
        view.activate();
        assert!(!view.failed);
        assert!(view.is_dirty());
        assert_eq!(view.message, "Opening…");
        view.request_view();
        assert_eq!(open_requests(&mut view), [0]);
        view.deactivate();
        view.activate();
        view.request_view();
        assert!(open_requests(&mut view).is_empty());
    }
}
