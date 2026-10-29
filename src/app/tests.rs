//! Application routing, tab lifetime and modal-input regression tests.

use super::{config::Config, state::App};
use crate::modules::session::Worker;
use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
/// Create an application with a temporary cache and wait for its first document.
fn app(paths: Vec<std::path::PathBuf>, cache: &std::path::Path) -> App {
    let worker = Worker::start(paths.clone(), cache.into());
    let mut app = App::new(paths, Config::default(), worker);
    wait_ready(&mut app);
    app
}
/// Drive layout and background replies until the active fixture becomes copyable.
fn wait_ready(app: &mut App) {
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
    app.prepare_frame(ratatui::layout::Rect::new(0, 0, 100, 30));
    loop {
        if app.view().is_dirty() {
            app.request_view();
        }
        app.receive();
        if app.view().can_copy() {
            break;
        }
        assert!(
            std::time::Instant::now() < deadline,
            "document did not become ready: {}",
            app.view().status().message
        );
        std::thread::sleep(std::time::Duration::from_millis(5));
    }
}
/// Verify tab navigation and closing remain global while modal input consumes page keys.
#[test]
fn global_shortcuts_and_modal_input_have_separate_owners() {
    let tmp = tempfile::tempdir().unwrap();
    let path = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("test/test.fa");
    let mut app = app(vec![path.clone(), path], &tmp.path().join("cache"));
    app.key(KeyEvent::new(KeyCode::Tab, KeyModifiers::NONE));
    assert_eq!(app.active, 1);
    wait_ready(&mut app);
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

/// Keep input on a current-tab click and cancel it when switching to another tab.
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
    let current = regions.iter().find(|(id, _)| *id == 0).unwrap().1;
    app.mouse(MouseEvent {
        kind: MouseEventKind::Down(MouseButton::Left),
        column: current.x,
        row: current.y,
        modifiers: KeyModifiers::NONE,
    });
    assert!(
        app.view().captures_input(),
        "clicking the current tab discarded input"
    );
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

/// Probe mixed formats by content and keep a duplicate VCF tab usable after alias closure.
#[test]
fn mixed_modules_route_by_content_and_keep_alias_tabs_usable() {
    use std::fs;
    let tmp = tempfile::tempdir().unwrap();
    let fasta = tmp.path().join("reference.txt");
    let vcf = tmp.path().join("variants.fa");
    fs::write(&fasta, ">reference\nACGT\n").unwrap();
    fs::write(&vcf, "##fileformat=VCFv4.3\n##contig=<ID=chr1>\n#CHROM\tPOS\tID\tREF\tALT\tQUAL\tFILTER\tINFO\nchr1\t10\t.\tA\tC\t.\tPASS\t.\n").unwrap();
    let mut app = app(vec![fasta, vcf.clone(), vcf], &tmp.path().join("cache"));
    assert!(app.view().status().message.contains("FASTA"));
    app.switch_tab(1);
    wait_ready(&mut app);
    app.key(KeyEvent::new(KeyCode::Char('c'), KeyModifiers::NONE));
    assert!(app.view().captures_input());
    assert_eq!(app.view().input_prompt().unwrap().label, "Contig: ");
    app.key(KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE));
    app.close_tab();
    assert_eq!(app.active, 2);
    wait_ready(&mut app);
    assert!(app.view().can_copy());
    app.switch_tab(0);
    wait_ready(&mut app);
    assert!(app.view().status().message.contains("FASTA"));
}

/// Keep a failed loading page responsive to the global close shortcut.
#[test]
fn failed_background_probe_remains_a_closable_document() {
    let tmp = tempfile::tempdir().unwrap();
    let missing = tmp.path().join("missing.vcf");
    let worker = Worker::start(vec![missing.clone()], tmp.path().join("cache"));
    let mut app = App::new(vec![missing], Config::default(), worker);
    app.request_view();
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
    while app.view().status().message == "Opening…" {
        app.receive();
        assert!(std::time::Instant::now() < deadline);
        std::thread::sleep(std::time::Duration::from_millis(5));
    }
    assert!(!app.view().can_copy());
    app.key(KeyEvent::new(KeyCode::Char('x'), KeyModifiers::NONE));
    assert!(app.quit);
}

/// Allow the tiny-terminal quit shortcut even while a page input prompt is open.
#[test]
fn tiny_terminal_quit_hint_works_with_modal_input_open() {
    let tmp = tempfile::tempdir().unwrap();
    let path = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("test/test.fa");
    let mut app = app(vec![path], &tmp.path().join("cache"));
    app.key(KeyEvent::new(KeyCode::Char('/'), KeyModifiers::NONE));
    assert!(app.view().captures_input());
    app.prepare_frame(ratatui::layout::Rect::new(0, 0, 20, 6));
    assert!(app.hits.too_small);
    app.key(KeyEvent::new(KeyCode::Char('q'), KeyModifiers::NONE));
    assert!(app.quit);
}
