//! The application hosts this interface; concrete pages own their data semantics.
use super::protocol::{ModuleRequest, ModuleUpdate};
use crossterm::event::{KeyEvent, MouseEvent};
use ratatui::{Frame, layout::Rect};
use std::sync::{Arc, atomic::AtomicBool};

/// Effects a page asks the application shell to perform after an interaction.
///
/// # Variants
///
/// - `Request`: Send a domain or lifecycle request to the background session.
/// - `BackgroundCancelled`: Set the worker's shared cooperative cancellation flag.
/// - `Clipboard`: Submit prepared text to the background clipboard service.
///   - `operation_id`: Copy operation identifier to validate on completion.
///   - `file`: Stable tab ID receiving the copy result.
///   - `text`: Text to send to the chosen clipboard backend.
///   - `allowed`: Whether the document permits copying this content.
///   - `cancel`: Cancellation flag owned by the originating page operation.
pub enum Action {
    /// Send a domain or lifecycle request to the background session.
    Request(ModuleRequest),
    /// Set the worker's shared cooperative cancellation flag.
    BackgroundCancelled(bool),
    /// Submit prepared text to the background clipboard service.
    ///
    /// # Fields
    ///
    /// - `operation_id`: Copy operation identifier to validate on completion.
    /// - `file`: Stable tab ID receiving the copy result.
    /// - `text`: Text to send to the chosen clipboard backend.
    /// - `allowed`: Whether the document permits copying this content.
    /// - `cancel`: Cancellation flag owned by the originating page operation.
    Clipboard {
        /// Copy operation identifier to validate on completion.
        operation_id: u64,
        /// Stable tab ID receiving the copy result.
        file: usize,
        /// Text to send to the chosen clipboard backend.
        text: String,
        /// Whether the document permits copying this content.
        allowed: bool,
        /// Cancellation flag owned by the originating page operation.
        cancel: Arc<AtomicBool>,
    },
}
/// Page-owned status text and shortcut hints displayed in the application footer.
///
/// # Fields
///
/// - `message`: Current progress, selection or operation status.
/// - `hints`: Contextual keyboard shortcuts for the current page state.
pub struct Status {
    /// Current progress, selection or operation status.
    pub message: String,
    /// Contextual keyboard shortcuts for the current page state.
    pub hints: String,
}
/// Borrowed label and editable text for a page's active input prompt.
///
/// # Fields
///
/// - `label`: Prompt prefix rendered before user text.
/// - `text`: Current editable contents borrowed from the page.
pub struct InputPrompt<'a> {
    /// Prompt prefix rendered before user text.
    pub label: &'a str,
    /// Current editable contents borrowed from the page.
    pub text: &'a str,
}

/// UI-thread contract for independently stateful document pages.
///
/// Pages enqueue effects through `actions`; the application owns service dispatch
/// and routes background replies back to the originating document.
pub trait DocumentView {
    /// Update page geometry and any layout-dependent viewport state.
    ///
    /// # Arguments
    ///
    /// - `area`: Terminal rectangle available for this layout or component.
    fn prepare(&mut self, area: Rect);
    /// Render the current page into its previously prepared region.
    ///
    /// # Arguments
    ///
    /// - `frame`: Terminal frame into which the current content is drawn.
    fn draw(&self, frame: &mut Frame);
    /// Handle a key routed to this page by the application shell.
    ///
    /// # Arguments
    ///
    /// - `event`: Keyboard event received from the terminal or the application shell.
    fn key(&mut self, event: KeyEvent);
    /// Handle pointer input using the page's prepared layout.
    ///
    /// # Arguments
    ///
    /// - `event`: Pointer event received from the terminal, including its position and button or
    ///   scroll action.
    fn mouse(&mut self, event: MouseEvent);
    /// Advance timed interactions such as drag autoscrolling.
    fn tick(&mut self);
    /// Report whether a modal or prompt should receive otherwise global shortcut keys.
    fn captures_input(&self) -> bool;
    /// Report whether a page modal should prevent mouse-based tab switching.
    fn captures_tab_mouse(&self) -> bool;
    /// Report whether the page needs a new backend snapshot.
    fn is_dirty(&self) -> bool;
    /// Report whether the displayed page must be repainted.
    fn needs_redraw(&self) -> bool;
    /// Acknowledge that the current page state has been rendered.
    fn did_draw(&mut self);
    /// Queue the backend work needed to refresh the viewport.
    fn request_view(&mut self);
    /// Apply a routed backend reply, checking page-specific lifetime identifiers.
    ///
    /// # Arguments
    ///
    /// - `update`: Backend reply to route or apply to the matching document and request.
    fn update(&mut self, update: ModuleUpdate);
    /// Drain pending effects for the application to dispatch.
    fn actions(&mut self) -> Vec<Action>;
    /// Resume this page when its tab becomes active.
    fn activate(&mut self);
    /// Suspend interaction and cancel page operations when leaving or closing the tab.
    fn deactivate(&mut self);
    /// Build the page's current status message and shortcut hints.
    fn status(&self) -> Status;
    /// Borrow the active input prompt, if one is open.
    fn input_prompt(&self) -> Option<InputPrompt<'_>>;
    /// Report whether this document currently permits copying.
    fn can_copy(&self) -> bool;
    /// Check whether a clipboard completion belongs to the current copy operation.
    ///
    /// # Arguments
    ///
    /// - `id`: Operation ID echoed by the clipboard result being checked.
    fn accepts_copy(&self, id: u64) -> bool;
    /// Finish the matching copy operation and expose its result to the user.
    ///
    /// # Arguments
    ///
    /// - `id`: Operation ID echoed by the clipboard result being checked.
    /// - `message`: Clipboard completion or failure message displayed for the matching operation.
    fn copy_finished(&mut self, id: u64, message: String);
    /// Invalidate state affected by a change in terminal dimensions.
    fn resized(&mut self);
}
