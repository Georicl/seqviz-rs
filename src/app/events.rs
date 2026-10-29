//! Global shortcuts and routing; document-specific interaction stays on its page.
use super::state::App;
use crossterm::event::{KeyCode, KeyEvent, KeyModifiers, MouseButton, MouseEvent, MouseEventKind};
impl App {
    /// Handle global shortcuts unless the active page captures input; always allow Ctrl-C.
    pub fn key(&mut self, key: KeyEvent) {
        self.redraw = true;
        if key.code == KeyCode::Char('c') && key.modifiers.contains(KeyModifiers::CONTROL) {
            self.quit = true;
            return;
        }
        if self.hits.too_small {
            if key.code == KeyCode::Char('q') {
                self.quit = true;
            }
            return;
        }
        if self.view().captures_input() {
            self.view_mut().key(key);
        } else {
            match key.code {
                KeyCode::Char('q') => self.quit = true,
                KeyCode::Tab => self.cycle_tab(false),
                KeyCode::BackTab => self.cycle_tab(true),
                KeyCode::Char('x') => self.close_tab(),
                _ => self.view_mut().key(key),
            }
        }
        self.flush_actions();
    }
    /// Switch tabs on eligible clicks, otherwise forward the event to the active page.
    pub fn mouse(&mut self, event: MouseEvent) {
        self.redraw = true;
        if self.hits.too_small {
            return;
        }
        if !self.view().captures_tab_mouse()
            && event.kind == MouseEventKind::Down(MouseButton::Left)
            && let Some(id) = self.tab_hit(event.column, event.row)
        {
            self.switch_tab(id);
            return;
        }
        self.view_mut().mouse(event);
        self.flush_actions();
    }
}
