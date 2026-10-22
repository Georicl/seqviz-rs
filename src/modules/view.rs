//! The application hosts this interface; concrete pages own their data semantics.
use super::protocol::{ModuleRequest, ModuleUpdate};
use crossterm::event::{KeyEvent, MouseEvent};
use ratatui::{Frame, layout::Rect};
use std::sync::{Arc, atomic::AtomicBool};

pub enum Action {
    Request(ModuleRequest),
    BackgroundCancelled(bool),
    Clipboard {
        operation_id: u64,
        file: usize,
        text: String,
        allowed: bool,
        cancel: Arc<AtomicBool>,
    },
}
pub struct Status {
    pub message: String,
    pub hints: String,
}
pub struct InputPrompt<'a> {
    pub label: &'a str,
    pub text: &'a str,
}

pub trait DocumentView {
    fn prepare(&mut self, area: Rect);
    fn draw(&self, frame: &mut Frame);
    fn key(&mut self, event: KeyEvent);
    fn mouse(&mut self, event: MouseEvent);
    fn tick(&mut self);
    fn captures_input(&self) -> bool;
    fn captures_tab_mouse(&self) -> bool;
    fn is_dirty(&self) -> bool;
    fn needs_redraw(&self) -> bool;
    fn did_draw(&mut self);
    fn request_view(&mut self);
    fn update(&mut self, update: ModuleUpdate);
    fn actions(&mut self) -> Vec<Action>;
    fn activate(&mut self);
    fn deactivate(&mut self);
    fn status(&self) -> Status;
    fn input_prompt(&self) -> Option<InputPrompt<'_>>;
    fn can_copy(&self) -> bool;
    fn accepts_copy(&self, id: u64) -> bool;
    fn copy_finished(&mut self, id: u64, message: String);
    fn resized(&mut self);
}
