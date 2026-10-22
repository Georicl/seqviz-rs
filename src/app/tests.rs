use super::{config::Config, state::App};
use crate::modules::session::Worker;
use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
fn app(paths: Vec<std::path::PathBuf>, cache: &std::path::Path) -> App {
    let worker = Worker::start(paths.clone(), cache.into());
    App::new(paths, Config::default(), worker)
}
#[test]
fn global_shortcuts_and_modal_input_have_separate_owners() {
    let tmp = tempfile::tempdir().unwrap();
    let path = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("test/test.fa");
    let mut app = app(vec![path.clone(), path], &tmp.path().join("cache"));
    app.key(KeyEvent::new(KeyCode::Tab, KeyModifiers::NONE));
    assert_eq!(app.active, 1);
    app.key(KeyEvent::new(KeyCode::Char('?'), KeyModifiers::NONE));
    assert!(app.view().captures_input());
    app.key(KeyEvent::new(KeyCode::Char('q'), KeyModifiers::NONE));
    assert!(!app.quit);
    assert!(!app.view().captures_input());
    app.key(KeyEvent::new(KeyCode::Char('x'), KeyModifiers::NONE));
    assert!(app.tabs[1].closed);
    assert_eq!(app.active, 0);
    app.key(KeyEvent::new(KeyCode::Char('q'), KeyModifiers::NONE));
    assert!(app.quit);
}

#[test]
fn input_box_still_allows_mouse_tab_switching() {
    use crossterm::event::{MouseButton, MouseEvent, MouseEventKind};
    let tmp = tempfile::tempdir().unwrap();
    let path = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("test/test.fa");
    let mut app = app(vec![path.clone(), path], &tmp.path().join("cache"));
    app.prepare_frame(ratatui::layout::Rect::new(0, 0, 80, 24));
    app.key(KeyEvent::new(KeyCode::Char('/'), KeyModifiers::NONE));
    assert!(app.view().captures_input());
    let regions = crate::widgets::tabs::layout(app.hits.tabs, &app.open_tabs(), app.active);
    let rect = regions.iter().find(|(id, _)| *id == 1).unwrap().1;
    app.mouse(MouseEvent {
        kind: MouseEventKind::Down(MouseButton::Left),
        column: rect.x,
        row: rect.y,
        modifiers: KeyModifiers::NONE,
    });
    assert_eq!(app.active, 1);
    assert!(!app.view().captures_input());
}
