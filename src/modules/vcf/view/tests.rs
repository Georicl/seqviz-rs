//! VCF document interaction tests with a real indexed source and queued application actions.
use super::*;
use crate::modules::vcf::data::Dataset;
use crossterm::event::{KeyCode, KeyEvent, KeyModifiers, MouseButton, MouseEvent, MouseEventKind};
use ratatui::{Terminal, backend::TestBackend};
use tempfile::TempDir;

/// A fully indexed temporary VCF connected to a document view without a worker thread.
struct Fixture {
    /// Temporary source/cache lifetime guard.
    _directory: TempDir,
    /// Fully indexed source used to answer view requests synchronously.
    dataset: Dataset,
    /// Document under test.
    view: VcfView,
    /// Original first record used to assert lossless copying.
    first: String,
}

impl Fixture {
    /// Build a multi-ALT, two-sample source and load its first viewport.
    fn new() -> Self {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("variants.vcf");
        let first = format!(
            "chr1\t10\t{}\tA\tC,G,T,AA,AC,AG,AT,CA,CG,CT\t99\tPASS\tDP=20;LONG={}\tGT:AD:DP:GQ\t0|10:0,4,0,6:20:80\t1/2:0,3,5:8:50",
            "long-id-".repeat(20),
            "abcdef".repeat(40)
        );
        let header = concat!(
            "##fileformat=VCFv4.3\n",
            "##contig=<ID=chr1>\n##contig=<ID=chr10>\n",
            "##INFO=<ID=DP,Number=1,Type=Integer,Description=\"Depth\">\n",
            "##INFO=<ID=LONG,Number=1,Type=String,Description=\"Long text\">\n",
            "##FORMAT=<ID=GT,Number=1,Type=String,Description=\"Genotype\">\n",
            "##FORMAT=<ID=AD,Number=R,Type=Integer,Description=\"Allele depth\">\n",
            "##FORMAT=<ID=DP,Number=1,Type=Integer,Description=\"Depth\">\n",
            "##FORMAT=<ID=GQ,Number=1,Type=Integer,Description=\"Quality\">\n",
            "#CHROM\tPOS\tID\tREF\tALT\tQUAL\tFILTER\tINFO\tFORMAT\tS1\tS2\n",
        );
        std::fs::write(&path, format!("{header}{first}\nchr1\t30\t.\tT\tC\t.\t.\tDP=4\tGT:AD:DP:GQ\t0/1:2,2:4:10\t0/0:4,0:4:20\nchr10\t20\t.\tG\tA\t42\tLowQual\tDP=6\tGT:AD:DP:GQ\t0/1:3,3:6:15\t1/1:0,6:6:30\n")).unwrap();
        let mut dataset = Dataset::open(&path, &directory.path().join("cache")).unwrap();
        while !dataset.progress().complete {
            dataset.step(64 * 1024).unwrap();
        }
        let mut view = VcfView::new(
            path,
            0,
            VcfConfig::default(),
            Theme::default(),
            Arc::new(AtomicU64::new(0)),
        );
        view.prepare(Rect::new(0, 0, 100, 30));
        let mut fixture = Self {
            _directory: directory,
            dataset,
            view,
            first,
        };
        fixture.refresh();
        fixture
    }

    /// Answer the queued viewport request synchronously from the fixture dataset.
    fn refresh(&mut self) {
        self.view.request_view();
        let request = self
            .view
            .actions()
            .into_iter()
            .find_map(|action| {
                if let Action::Request(ModuleRequest::Vcf(envelope)) = action
                    && let Request::View(request) = envelope.payload
                {
                    return Some(request);
                }
                None
            })
            .unwrap();
        let selected = self
            .dataset
            .entry(request.contig.as_deref(), request.selected)
            .unwrap();
        let snapshot = Snapshot {
            rows: self
                .dataset
                .rows(request.contig.as_deref(), request.start, request.count)
                .unwrap(),
            selected,
            total: self.dataset.total(request.contig.as_deref()).unwrap(),
            contigs: self.dataset.contigs().unwrap(),
            header: self.dataset.header(),
            progress: self.dataset.progress(),
            compression: self.dataset.compression().to_owned(),
            paused: false,
            request,
        };
        self.view
            .update(ModuleUpdate::vcf(Update::View(Box::new(snapshot))));
    }

    /// Send an unmodified key press through the document interface.
    fn key(&mut self, code: KeyCode) {
        self.view.key(KeyEvent::new(code, KeyModifiers::NONE));
    }

    /// Extract the queued clipboard payload, operation ID and cancellation flag.
    fn copied(&mut self) -> (String, u64, Arc<AtomicBool>) {
        self.view
            .actions()
            .into_iter()
            .find_map(|action| {
                if let Action::Clipboard {
                    text,
                    operation_id,
                    cancel,
                    ..
                } = action
                {
                    Some((text, operation_id, cancel))
                } else {
                    None
                }
            })
            .unwrap()
    }
}

/// Verify full raw-record/header copying and explicitly selected header text.
#[test]
fn raw_copy_keeps_tabs_and_full_record_and_header_copy_is_a_small_vcf() {
    let mut fixture = Fixture::new();
    fixture.key(KeyCode::Char('y'));
    let (raw, _, _) = fixture.copied();
    assert_eq!(raw, fixture.first);
    assert!(raw.contains(&"long-id-".repeat(20)));
    assert!(raw.contains('\t'));
    assert!(!raw.contains('\u{1b}'));
    fixture.key(KeyCode::Char('Y'));
    let (vcf, _, _) = fixture.copied();
    assert!(vcf.starts_with("##fileformat=VCFv4.3\n"));
    assert!(vcf.ends_with(&format!("{}\n", fixture.first)));
    fixture.key(KeyCode::Enter);
    fixture.key(KeyCode::Char('3'));
    fixture.key(KeyCode::Char('y'));
    assert_eq!(fixture.copied().0, fixture.first);
    fixture.key(KeyCode::Char('m'));
    assert!(
        fixture
            .view
            .info
            .lines
            .iter()
            .any(|line| line.starts_with("#CHROM "))
    );
    assert!(
        fixture
            .view
            .info
            .lines
            .iter()
            .all(|line| !line.contains('\t'))
    );
    fixture.key(KeyCode::Char('y'));
    assert_eq!(fixture.copied().0, fixture.dataset.header().raw);
    let inner = layout::inset(fixture.view.layout.area);
    for kind in [
        MouseEventKind::Down(MouseButton::Left),
        MouseEventKind::Up(MouseButton::Left),
    ] {
        fixture.view.mouse(MouseEvent {
            kind,
            column: inner.x,
            row: inner.y,
            modifiers: KeyModifiers::NONE,
        });
    }
    assert!(fixture.view.info.selection.is_none());
    fixture.key(KeyCode::Char('y'));
    assert_eq!(fixture.copied().0, fixture.dataset.header().raw);
    for (kind, x) in [
        (MouseEventKind::Down(MouseButton::Left), inner.x),
        (MouseEventKind::Drag(MouseButton::Left), inner.x + 3),
        (MouseEventKind::Up(MouseButton::Left), inner.x + 3),
    ] {
        fixture.view.mouse(MouseEvent {
            kind,
            column: x,
            row: inner.y,
            modifiers: KeyModifiers::NONE,
        });
    }
    fixture.key(KeyCode::Char('y'));
    assert_eq!(fixture.copied().0, "##fi");
}

/// Check INFO, sample-row and dragged-text copy payloads preserve their logical content.
#[test]
fn detail_copy_uses_fields_sample_headers_and_original_selected_text() {
    let mut fixture = Fixture::new();
    fixture.key(KeyCode::Enter);
    fixture.key(KeyCode::Char('y'));
    assert_eq!(fixture.copied().0, "DP=20");
    let body = fixture.view.layout.detail_body;
    for kind in [
        MouseEventKind::Down(MouseButton::Left),
        MouseEventKind::Up(MouseButton::Left),
    ] {
        fixture.view.mouse(MouseEvent {
            kind,
            column: body.x,
            row: body.y,
            modifiers: KeyModifiers::NONE,
        });
    }
    assert!(fixture.view.selection.is_none());
    fixture.key(KeyCode::Char('y'));
    assert_eq!(fixture.copied().0, "DP=20");
    fixture.key(KeyCode::Char('2'));
    fixture.key(KeyCode::Char('y'));
    assert_eq!(
        fixture.copied().0,
        "SAMPLE\tGT\tAD\tDP\tGQ\nS1\t0|10\t0,4,0,6\t20\t80"
    );
    let body = fixture.view.layout.detail_body;
    for (kind, x) in [
        (MouseEventKind::Down(MouseButton::Left), body.x),
        (MouseEventKind::Drag(MouseButton::Left), body.x + 15),
        (MouseEventKind::Up(MouseButton::Left), body.x + 15),
    ] {
        fixture.view.mouse(MouseEvent {
            kind,
            column: x,
            row: body.y,
            modifiers: KeyModifiers::NONE,
        });
    }
    fixture.key(KeyCode::Char('y'));
    assert_eq!(fixture.copied().0, "S1\t0|10");
}

/// Check Home/End navigation discards dragged selections before copying the newly selected sample.
#[test]
fn home_and_end_replace_dragged_text_with_the_new_sample_context() {
    let mut fixture = Fixture::new();
    fixture.key(KeyCode::Enter);
    fixture.key(KeyCode::Char('2'));
    let body = fixture.view.layout.detail_body;
    for (navigation, from_row, expected) in [
        (
            KeyCode::End,
            0,
            "SAMPLE\tGT\tAD\tDP\tGQ\nS2\t1/2\t0,3,5\t8\t50",
        ),
        (
            KeyCode::Home,
            1,
            "SAMPLE\tGT\tAD\tDP\tGQ\nS1\t0|10\t0,4,0,6\t20\t80",
        ),
    ] {
        for (kind, column) in [
            (MouseEventKind::Down(MouseButton::Left), body.x),
            (MouseEventKind::Drag(MouseButton::Left), body.x + 1),
        ] {
            fixture.view.mouse(MouseEvent {
                kind,
                column,
                row: body.y + from_row,
                modifiers: KeyModifiers::NONE,
            });
        }
        fixture.key(KeyCode::Char('y'));
        assert!(matches!(fixture.copied().0.as_str(), "S1" | "S2"));
        fixture.key(navigation);
        assert!(fixture.view.selection.is_none());
        assert!(!fixture.view.dragging);
        fixture.key(KeyCode::Char('y'));
        assert_eq!(fixture.copied().0, expected);
    }
}

/// Verify contig ranking and acceptance of only the current position-lookup generation.
#[test]
fn picker_prioritizes_exact_contig_and_position_uses_filtered_ordinals() {
    let mut fixture = Fixture::new();
    fixture.view.horizontal = 36;
    fixture.key(KeyCode::Char('c'));
    assert!(fixture.view.picker_options()[0].0.is_none());
    for ch in "chr1".chars() {
        fixture.key(KeyCode::Char(ch));
    }
    let options = fixture.view.picker_options();
    assert_eq!(options[0].0.as_deref(), Some("chr1"));
    assert!(options.iter().any(|(name, _)| name.is_none()));
    fixture.key(KeyCode::Enter);
    assert_eq!(fixture.view.contig.as_deref(), Some("chr1"));
    assert_eq!(fixture.view.horizontal, 0);
    fixture.refresh();
    fixture.key(KeyCode::Down);
    fixture.refresh();
    assert_eq!(fixture.view.entry().unwrap().record.pos, 30);
    fixture.key(KeyCode::Char(':'));
    fixture.key(KeyCode::Char('3'));
    fixture.key(KeyCode::Char('0'));
    fixture.key(KeyCode::Enter);
    let token = fixture.view.token;
    assert!(fixture.view.actions().into_iter().any(|action| matches!(action, Action::Request(ModuleRequest::Vcf(envelope)) if matches!(envelope.payload, Request::Locate { ref contig, position:30, .. } if contig == "chr1"))));
    fixture.view.update(ModuleUpdate::vcf(Update::Located {
        file: 0,
        token: token - 1,
        selected: 0,
    }));
    assert_eq!(fixture.view.selected, 1);
    fixture.view.update(ModuleUpdate::vcf(Update::Located {
        file: 0,
        token,
        selected: 0,
    }));
    assert_eq!(fixture.view.selected, 0);
    fixture.refresh();
    fixture.key(KeyCode::Down);
    fixture.refresh();
    fixture.view.start = 1;
    fixture.key(KeyCode::Char(':'));
    fixture.key(KeyCode::Char('9'));
    fixture.key(KeyCode::Enter);
    let token = fixture.view.token;
    fixture.view.update(ModuleUpdate::vcf(Update::LocateFailed {
        file: 0,
        token: token - 1,
        text: "old failure".into(),
    }));
    assert_eq!(fixture.view.locating, Some(token));
    assert_eq!((fixture.view.selected, fixture.view.start), (1, 1));
    fixture.view.update(ModuleUpdate::vcf(Update::LocateFailed {
        file: 0,
        token,
        text: "No indexed variants for chr1".into(),
    }));
    assert_eq!(fixture.view.locating, None);
    assert_eq!((fixture.view.selected, fixture.view.start), (0, 0));
    assert!(fixture.view.dirty);
    fixture.refresh();
    assert_eq!(fixture.view.entry().unwrap().record.pos, 10);
    assert_eq!(fixture.view.message, "No indexed variants for chr1");
}

/// Check Escape and tab deactivation cancel clipboard operations and reject stale completions.
#[test]
fn cancelled_and_inactive_copy_completions_are_rejected() {
    let mut fixture = Fixture::new();
    fixture.key(KeyCode::Char('y'));
    let (_, first, first_cancel) = fixture.copied();
    fixture.key(KeyCode::Esc);
    assert!(first_cancel.load(Ordering::Relaxed));
    assert!(!fixture.view.accepts_copy(first));
    fixture.key(KeyCode::Char('y'));
    let (_, second, second_cancel) = fixture.copied();
    fixture.view.deactivate();
    assert!(second_cancel.load(Ordering::Relaxed));
    assert!(!fixture.view.accepts_copy(second));
    let actions = fixture.view.actions();
    assert!(
        actions
            .iter()
            .any(|a| matches!(a, Action::BackgroundCancelled(true)))
    );
    assert!(!actions.iter().any(|a| matches!(a, Action::Request(ModuleRequest::Vcf(e)) if matches!(e.payload, Request::Cancel(_)))));
    fixture.view.activate();
    assert!(
        fixture
            .view
            .actions()
            .iter()
            .any(|a| matches!(a, Action::BackgroundCancelled(false)))
    );
    fixture
        .view
        .copy_finished(second, "stale completion".into());
    assert_ne!(fixture.view.message, "stale completion");
}

/// Render the document and contig picker down to a one-cell terminal without panics.
#[test]
fn terminal_renders_details_and_picker_at_narrow_sizes() {
    let mut fixture = Fixture::new();
    let mut terminal = Terminal::new(TestBackend::new(100, 30)).unwrap();
    terminal.draw(|frame| fixture.view.draw(frame)).unwrap();
    let rendered: String = terminal
        .backend()
        .buffer()
        .content()
        .iter()
        .map(|cell| cell.symbol())
        .collect();
    assert!(rendered.contains("Variants"));
    assert!(rendered.contains("SAMPLES"));
    assert!(rendered.contains("VCFv4.3"));
    fixture.key(KeyCode::Enter);
    fixture.key(KeyCode::Char('2'));
    assert_eq!(fixture.view.focus, Focus::Detail);
    fixture.key(KeyCode::Esc);
    assert_eq!(fixture.view.focus, Focus::Table);
    for (width, height) in [(28, 8), (12, 5), (1, 1)] {
        terminal.backend_mut().resize(width, height);
        terminal.autoresize().unwrap();
        fixture.view.prepare(Rect::new(0, 0, width, height));
        terminal.draw(|frame| fixture.view.draw(frame)).unwrap();
        fixture.key(KeyCode::Char('c'));
        terminal.draw(|frame| fixture.view.draw(frame)).unwrap();
        fixture.key(KeyCode::Esc);
    }
}

/// Check Unicode/TAB clipping and pointer selection map to the same original text.
#[test]
fn horizontal_clipping_and_mouse_selection_share_original_display_columns() {
    let mut fixture = Fixture::new();
    Arc::make_mut(&mut fixture.view.snapshot.as_mut().unwrap().header).samples[0] = "中S".into();
    fixture.key(KeyCode::Enter);
    fixture.key(KeyCode::Char('2'));
    let mut terminal = Terminal::new(TestBackend::new(100, 30)).unwrap();
    let body = fixture.view.layout.detail_body;
    fixture.view.horizontal = 1;
    terminal.draw(|frame| fixture.view.draw(frame)).unwrap();
    let buffer = terminal.backend().buffer();
    assert_eq!(buffer[(body.x, body.y)].symbol(), " ");
    assert_eq!(buffer[(body.x + 1, body.y)].symbol(), "S");
    assert_eq!(buffer[(body.x + 11, body.y)].symbol(), "0");
    assert_eq!(fixture.view.detail_position(body.x, body.y), (0, 0));
    assert_eq!(fixture.view.detail_position(body.x + 11, body.y), (0, 3));
    for (kind, x) in [
        (MouseEventKind::Down(MouseButton::Left), body.x),
        (MouseEventKind::Drag(MouseButton::Left), body.x + 11),
    ] {
        fixture.view.mouse(MouseEvent {
            kind,
            column: x,
            row: body.y,
            modifiers: KeyModifiers::NONE,
        });
    }
    fixture.key(KeyCode::Char('y'));
    assert_eq!(fixture.copied().0, "中S\t0");
    fixture.view.layout.detail_body.width = 5;
    for (horizontal, expected) in [(4, "     "), (11, " 0|10")] {
        fixture.view.horizontal = horizontal;
        terminal.draw(|frame| fixture.view.draw(frame)).unwrap();
        let actual: String = (0..5)
            .map(|x| terminal.backend().buffer()[(body.x + x, body.y)].symbol())
            .collect();
        assert_eq!(actual, expected);
        let expected_index = if horizontal == 4 { 2 } else { 3 };
        assert_eq!(
            fixture.view.detail_position(body.x + 1, body.y),
            (0, expected_index)
        );
    }
}

/// Verify resize preserves the selected sample and returns focus when details are hidden.
#[test]
fn resizing_keeps_current_sample_visible_and_hidden_details_cannot_take_focus() {
    let mut fixture = Fixture::new();
    let snapshot = fixture.view.snapshot.as_mut().unwrap();
    Arc::make_mut(&mut snapshot.header).samples = (0..20).map(|i| format!("S{i:02}")).collect();
    let entry = Arc::make_mut(snapshot.selected.as_mut().unwrap());
    entry.record.raw = entry
        .record
        .raw
        .split('\t')
        .take(9)
        .map(str::to_owned)
        .chain((0..20).map(|_| "0/1:1,1:2:99".into()))
        .collect::<Vec<_>>()
        .join("\t");
    fixture.key(KeyCode::Enter);
    fixture.key(KeyCode::Char('2'));
    fixture.key(KeyCode::End);
    assert_eq!(fixture.view.detail_row, 19);
    fixture.view.prepare(Rect::new(0, 0, 100, 11));
    let height = fixture.view.layout.detail_body.height as usize;
    assert!(height > 0);
    assert!(fixture.view.detail_start <= fixture.view.detail_row);
    assert!(fixture.view.detail_row < fixture.view.detail_start + height);
    let mut terminal = Terminal::new(TestBackend::new(100, 11)).unwrap();
    terminal.draw(|frame| fixture.view.draw(frame)).unwrap();
    let rendered: String = terminal
        .backend()
        .buffer()
        .content()
        .iter()
        .map(|cell| cell.symbol())
        .collect();
    assert!(rendered.contains("S19"));
    fixture.view.prepare(Rect::new(0, 0, 100, 8));
    assert_eq!(fixture.view.focus, Focus::Table);
    fixture.key(KeyCode::Enter);
    assert_eq!(fixture.view.focus, Focus::Table);
}

/// Check missing INFO, samples and records produce status text without empty clipboard jobs.
#[test]
fn absent_detail_context_never_submits_an_empty_clipboard_request() {
    let mut fixture = Fixture::new();
    let snapshot = fixture.view.snapshot.as_mut().unwrap();
    Arc::make_mut(&mut snapshot.header).samples.clear();
    let entry = Arc::make_mut(snapshot.selected.as_mut().unwrap());
    let mut columns = entry
        .record
        .raw
        .split('\t')
        .take(8)
        .map(str::to_owned)
        .collect::<Vec<_>>();
    columns[7] = ".".into();
    entry.record.raw = columns.join("\t");
    entry.record.format.clear();
    fixture.key(KeyCode::Enter);
    for detail in ['1', '2'] {
        fixture.key(KeyCode::Char(detail));
        fixture.key(KeyCode::Char('y'));
        assert_eq!(fixture.view.message, "Nothing to copy");
        assert!(
            !fixture
                .view
                .actions()
                .iter()
                .any(|action| matches!(action, Action::Clipboard { .. }))
        );
    }
    let snapshot = fixture.view.snapshot.as_mut().unwrap();
    snapshot.rows.clear();
    snapshot.selected = None;
    snapshot.total = 0;
    fixture.key(KeyCode::Char('y'));
    assert_eq!(fixture.view.message, "No record selected");
    assert!(
        !fixture
            .view
            .actions()
            .iter()
            .any(|action| matches!(action, Action::Clipboard { .. }))
    );
}

/// Ensure load errors persist independently of clipboard and pause status messages.
#[test]
fn backend_load_failure_is_separate_from_clipboard_and_pause_messages() {
    let mut fixture = Fixture::new();
    fixture.view.snapshot = None;
    fixture.view.update(ModuleUpdate::vcf(Update::Message {
        file: 0,
        text: "Background work paused".into(),
    }));
    assert!(fixture.view.load_error.is_none());
    fixture.view.update(ModuleUpdate::vcf(Update::Message {
        file: 0,
        text: "VCF header: invalid header".into(),
    }));
    assert_eq!(
        fixture.view.load_error.as_deref(),
        Some("VCF header: invalid header")
    );
    fixture.refresh();
    assert!(fixture.view.load_error.is_none());
    assert_ne!(fixture.view.message, "VCF header: invalid header");
    fixture.key(KeyCode::Char('y'));
    assert!(fixture.view.load_error.is_none());
}

/// Verify resizing retains a lookup token while user navigation and cancellation invalidate it.
#[test]
fn resize_preserves_pending_locate_but_navigation_cancel_and_deactivation_end_it() {
    let mut fixture = Fixture::new();
    fixture.key(KeyCode::Char(':'));
    fixture.key(KeyCode::Char('9'));
    fixture.key(KeyCode::Enter);
    let token = fixture.view.token;
    fixture.view.prepare(Rect::new(0, 0, 100, 22));
    fixture.view.request_view();
    assert_eq!(fixture.view.token, token);
    assert_eq!(fixture.view.locating, Some(token));
    let actions = fixture.view.actions();
    assert!(actions.iter().any(|action| matches!(action, Action::Request(ModuleRequest::Vcf(envelope)) if matches!(envelope.payload, Request::Locate { .. }))));
    assert!(!actions.iter().any(|action| matches!(action, Action::Request(ModuleRequest::Vcf(envelope)) if matches!(envelope.payload, Request::View(_)))));
    fixture.view.update(ModuleUpdate::vcf(Update::Located {
        file: 0,
        token,
        selected: 1,
    }));
    assert!(fixture.view.locating.is_none());
    assert!(fixture.view.dirty);
    fixture.refresh();
    assert_eq!(fixture.view.entry().unwrap().record.pos, 30);

    for operation in ["navigate", "cancel", "contig", "deactivate"] {
        fixture.key(KeyCode::Char(':'));
        fixture.key(KeyCode::Char('9'));
        fixture.key(KeyCode::Enter);
        let token = fixture.view.token;
        match operation {
            "navigate" => fixture.key(KeyCode::Up),
            "cancel" => fixture.key(KeyCode::Esc),
            "contig" => {
                fixture.key(KeyCode::Char('c'));
                fixture.key(KeyCode::Enter);
            }
            "deactivate" => fixture.view.deactivate(),
            _ => unreachable!(),
        }
        assert!(fixture.view.locating.is_none(), "{operation}");
        let selected = fixture.view.selected;
        fixture.view.update(ModuleUpdate::vcf(Update::Located {
            file: 0,
            token,
            selected: 1,
        }));
        assert_eq!(fixture.view.selected, selected, "{operation}");
        if operation == "deactivate" {
            fixture.view.activate();
        }
        fixture.refresh();
    }
}
