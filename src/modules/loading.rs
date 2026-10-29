//! A document can be hosted before its format has been inspected by the worker.
use super::{
    protocol::{ModuleRequest, ModuleUpdate},
    view::{Action, DocumentView, InputPrompt, Status},
};
use crate::widgets::theme::Theme;
use crossterm::event::{KeyEvent, MouseEvent};
use ratatui::{Frame, layout::Rect, style::Style, widgets::Paragraph};
use std::path::PathBuf;

/// Temporary page that requests format probing and displays opening errors.
pub(crate) struct LoadingView {
    /// Input path shown while probing or reporting errors.
    path: PathBuf,
    /// Stable tab ID used in open requests and matching replies.
    file: usize,
    /// Colours used by the placeholder.
    theme: Theme,
    /// Latest document rectangle supplied by the application.
    area: Rect,
    /// Whether an open request is already pending or has completed.
    sent: bool,
    /// Whether the last matching probe failed and can be retried on reactivation.
    failed: bool,
    /// Whether this tab is active and allowed to request probing.
    enabled: bool,
    /// Whether the placeholder needs repainting.
    redraw: bool,
    /// Opening status or the last probe error.
    message: String,
    /// Open requests waiting for application dispatch.
    actions: Vec<Action>,
}

impl LoadingView {
    /// Create a loading page, initially enabling only the first input tab.
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
    /// Store the document region for the opening message.
    fn prepare(&mut self, area: Rect) {
        self.area = area;
    }
    /// Render the input path and current opening or error message.
    fn draw(&self, frame: &mut Frame) {
        frame.render_widget(
            Paragraph::new(format!("{}\n\n{}", self.path.display(), self.message))
                .style(Style::default().fg(self.theme.muted)),
            self.area,
        );
    }
    /// Ignore page-level keys while global tab and quit shortcuts remain available.
    fn key(&mut self, _: KeyEvent) {}
    /// Ignore page-level pointer events during format detection.
    fn mouse(&mut self, _: MouseEvent) {}
    /// Leave opening to the worker; this placeholder has no timed interactions.
    fn tick(&mut self) {}
    /// Allow application keyboard shortcuts while the page is loading.
    fn captures_input(&self) -> bool {
        false
    }
    /// Allow mouse-based tab switching while the page is loading.
    fn captures_tab_mouse(&self) -> bool {
        false
    }
    /// Request probing only when this page is enabled and has not sent an open request.
    fn is_dirty(&self) -> bool {
        self.enabled && !self.sent
    }
    /// Expose whether the opening message or activation state needs painting.
    fn needs_redraw(&self) -> bool {
        self.redraw
    }
    /// Clear the placeholder's redraw flag after rendering.
    fn did_draw(&mut self) {
        self.redraw = false;
    }
    /// Queue one format-probe request and suppress duplicates while it is pending.
    fn request_view(&mut self) {
        if self.enabled && !self.sent {
            self.actions
                .push(Action::Request(ModuleRequest::Open { file: self.file }));
            self.sent = true;
        }
    }
    /// Display a matching open failure and make it eligible for retry on reactivation.
    fn update(&mut self, update: ModuleUpdate) {
        if let ModuleUpdate::OpenFailed { file, text } = update
            && file == self.file
        {
            self.message = text;
            self.failed = true;
            self.redraw = true;
        }
    }
    /// Drain queued open requests for dispatch by the application.
    fn actions(&mut self) -> Vec<Action> {
        std::mem::take(&mut self.actions)
    }
    /// Enable the page and reset failed attempts so revisiting the tab retries opening.
    fn activate(&mut self) {
        if self.failed {
            self.sent = false;
            self.failed = false;
            self.message = "Opening…".into();
        }
        self.enabled = true;
        self.redraw = true;
    }
    /// Suspend new open requests while this tab is inactive.
    fn deactivate(&mut self) {
        self.enabled = false;
    }
    /// Show the opening/error message with global tab and quit shortcuts.
    fn status(&self) -> Status {
        Status {
            message: self.message.clone(),
            hints: "Tab file · x close · q quit".into(),
        }
    }
    /// Return no prompt because the loading page has no editable input.
    fn input_prompt(&self) -> Option<InputPrompt<'_>> {
        None
    }
    /// Disable copying until a concrete document view has been created.
    fn can_copy(&self) -> bool {
        false
    }
    /// Reject clipboard completions because this page never starts a copy.
    fn accepts_copy(&self, _: u64) -> bool {
        false
    }
    /// Ignore copy completions; the loading placeholder owns no copy operation.
    fn copy_finished(&mut self, _: u64, _: String) {}
    /// Request repainting after the terminal dimensions change.
    fn resized(&mut self) {
        self.redraw = true;
    }
}

/// Opening request deduplication and retry-on-reactivation tests.
#[cfg(test)]
mod tests {
    use super::*;

    /// Drain a loading page's effects and collect only format-probe file IDs.
    fn open_requests(view: &mut LoadingView) -> Vec<usize> {
        view.actions()
            .into_iter()
            .filter_map(|action| match action {
                Action::Request(ModuleRequest::Open { file }) => Some(file),
                _ => None,
            })
            .collect()
    }

    /// Keep repeated activation from duplicating a still-pending open request.
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

    /// Ignore unrelated failures and retry a matching failure once on reactivation.
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
