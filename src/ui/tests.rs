use super::{app::*, render};
use crate::data::Dataset;
use crate::{
    config::Config,
    worker::{Snapshot, ViewRequest, Worker},
};
use crossterm::event::{KeyCode, KeyEvent, KeyModifiers, MouseButton, MouseEvent, MouseEventKind};
use ratatui::backend::TestBackend;
use ratatui::{Terminal, layout::Rect};
use std::{path::PathBuf, sync::atomic::Ordering};

fn app(path: PathBuf, cache: &std::path::Path) -> App {
    let mut data = Dataset::open(&path, cache).unwrap();
    while !data.progress().complete && data.progress().error.is_none() {
        data.step(64 * 1024).unwrap();
    }
    let snapshot = Snapshot {
        request: ViewRequest {
            token: 1,
            file: 0,
            record: 0,
            start: 0,
            count: 1024,
            list_start: 0,
            list_count: 10,
        },
        format: data.format(),
        compression: data.compression().into(),
        file_size: std::fs::metadata(&path).ok().map(|m| m.len()),
        records: data.records(0, 10).unwrap(),
        record: data.record(0).unwrap(),
        window: data.read_range(0, 0, 1024).unwrap(),
        progress: data.progress(),
        stats: Some(data.stats().unwrap()),
        paused: false,
    };
    let paths = vec![path];
    let worker = Worker::start(paths.clone(), cache.into());
    let mut app = App::new(paths, Config::default(), worker);
    app.tabs[0].snapshot = Some(Box::new(snapshot));
    app.tabs[0].message.clear();
    app.token = 1;
    app
}

fn key(code: KeyCode) -> KeyEvent {
    KeyEvent::new(code, KeyModifiers::NONE)
}

#[test]
fn renderer_selection_and_resize_keep_sequence_coordinates() {
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("sample.fa");
    std::fs::write(
        &path,
        format!(">first description\n{}\n>second\nTT", "ACGT".repeat(100)),
    )
    .unwrap();
    let mut app = app(path, &temp.path().join("cache"));
    let mut terminal = Terminal::new(TestBackend::new(80, 24)).unwrap();
    terminal
        .draw(|frame| {
            app.prepare_frame(frame.area());
            render::draw(frame, &app);
        })
        .unwrap();
    let first_width = app.hits.bases_per_line;
    let x = app.hits.sequence.x + app.hits.gutter + 2;
    let y = app.hits.sequence.y;
    app.mouse(MouseEvent {
        kind: MouseEventKind::Down(MouseButton::Left),
        column: x,
        row: y,
        modifiers: KeyModifiers::NONE,
    });
    app.mouse(MouseEvent {
        kind: MouseEventKind::Drag(MouseButton::Left),
        column: x + 5,
        row: y + 1,
        modifiers: KeyModifiers::NONE,
    });
    app.mouse(MouseEvent {
        kind: MouseEventKind::Up(MouseButton::Left),
        column: x + 5,
        row: y + 1,
        modifiers: KeyModifiers::NONE,
    });
    assert_eq!(app.tab().selection, Some((2, first_width as u64 + 7)));
    let selection = app.tab().selection;
    terminal.backend_mut().resize(120, 30);
    terminal.autoresize().unwrap();
    terminal
        .draw(|frame| {
            app.prepare_frame(frame.area());
            render::draw(frame, &app);
        })
        .unwrap();
    assert_ne!(first_width, app.hits.bases_per_line);
    assert_eq!(app.tab().selection, selection);
    assert_eq!(app.tab().start, 0);
    app.open_page(Page::Help);
    app.key(key(KeyCode::Char('q')));
    assert!(app.page.is_none());
    assert!(!app.quit);
    app.key(key(KeyCode::Char('q')));
    assert!(app.quit);
}

#[test]
fn sam_copy_is_disabled_in_sequence_and_information_views() {
    let temp = tempfile::tempdir().unwrap();
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("test/alignments.sam");
    let mut app = app(path, &temp.path().join("cache"));
    app.copy();
    assert!(app.tab().message.contains("disabled"));
    app.focus = Focus::Sequence;
    app.tab_mut().selection = Some((0, 1));
    app.copy();
    assert!(app.tab().message.contains("disabled"));
    app.open_page(Page::Record);
    app.copy_info();
    assert!(app.tab().message.contains("disabled"));
}

#[test]
fn jump_is_one_based_and_rejects_positions_past_record() {
    let temp = tempfile::tempdir().unwrap();
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("test/test.fa");
    let mut app = app(path, &temp.path().join("cache"));
    app.input = Some(Input::Position("5".into()));
    app.key(key(KeyCode::Enter));
    assert_eq!(app.tab().start, 4);
    app.input = Some(Input::Position("9".into()));
    app.key(key(KeyCode::Enter));
    assert_eq!(app.tab().start, 4);
    assert!(app.tab().message.contains("outside"));
}

#[test]
fn held_edge_drag_extends_selection_while_new_view_data_is_pending() {
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("long.fa");
    std::fs::write(&path, format!(">long\n{}\n", "ACGT".repeat(5_000))).unwrap();
    let mut app = app(path, &temp.path().join("cache"));
    app.prepare_frame(Rect::new(0, 0, 80, 24));
    let x = app.hits.sequence.x + app.hits.gutter;
    let top = app.hits.sequence.y;
    let bottom = app.hits.sequence.bottom() - 1;
    app.mouse(MouseEvent {
        kind: MouseEventKind::Down(MouseButton::Left),
        column: x,
        row: top,
        modifiers: KeyModifiers::NONE,
    });
    app.mouse(MouseEvent {
        kind: MouseEventKind::Drag(MouseButton::Left),
        column: x,
        row: bottom,
        modifiers: KeyModifiers::NONE,
    });
    let before = app.tab().selection.unwrap();
    let width = app.hits.bases_per_line as u64;
    // No new mouse event or worker snapshot arrives while the pointer is held.
    for _ in 0..4 {
        app.drag_scroll();
    }
    assert_eq!(app.tab().snapshot.as_ref().unwrap().window.start, 0);
    assert_eq!(app.tab().start, 4 * width);
    assert_eq!(app.tab().selection, Some((before.0, before.1 + 4 * width)));
    let selection = app.tab().selection;
    app.prepare_frame(Rect::new(0, 0, 120, 30));
    assert_eq!(app.tab().selection, selection);
    app.mouse(MouseEvent {
        kind: MouseEventKind::Up(MouseButton::Left),
        column: x,
        row: bottom,
        modifiers: KeyModifiers::NONE,
    });
    assert!(app.drag.is_none());
}

#[test]
fn resizing_and_quality_visibility_preserve_active_operation_identity() {
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("reads.fq");
    std::fs::write(
        &path,
        format!("@read\n{}\n+\n{}\n", "ACGT".repeat(100), "I".repeat(400)),
    )
    .unwrap();
    let mut app = app(path, &temp.path().join("cache"));
    app.prepare_frame(Rect::new(0, 0, 80, 24));
    let rows = app.hits.sequence_rows;
    let (operation_id, cancel) = app.begin_operation(OperationKind::Copy);
    app.request_view();
    let token = app.token;
    app.key(key(KeyCode::Char('Q')));
    app.prepare_frame(Rect::new(0, 0, 120, 30));
    app.request_view();
    assert_ne!(rows, app.hits.sequence_rows);
    assert!(app.token > token);
    assert!(!cancel.load(Ordering::Relaxed));
    assert!(app.matches_operation(0, operation_id, OperationKind::Copy));
    app.choose_record(0);
    assert!(cancel.load(Ordering::Relaxed));
    assert!(app.pending_operation.is_none());
}

#[test]
fn a_new_operation_never_resets_the_old_cancellation_flag() {
    let temp = tempfile::tempdir().unwrap();
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("test/test.fa");
    let mut app = app(path, &temp.path().join("cache"));
    let (old_id, old_cancel) = app.begin_operation(OperationKind::Search);
    let (new_id, new_cancel) = app.begin_operation(OperationKind::Copy);
    assert!(old_cancel.load(Ordering::Relaxed));
    assert!(!new_cancel.load(Ordering::Relaxed));
    assert!(!app.matches_operation(0, old_id, OperationKind::Search));
    assert!(app.matches_operation(0, new_id, OperationKind::Copy));
    app.key(key(KeyCode::Esc));
    assert!(new_cancel.load(Ordering::Relaxed));
    assert!(app.pending_operation.is_none());
}
