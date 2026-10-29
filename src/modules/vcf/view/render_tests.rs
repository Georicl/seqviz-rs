//! Cell-level rendering checks for narrow VCF layouts, alignment and Unicode text.
use super::*;
use crate::modules::vcf::model::{
    ContigSummary, Progress, RecordSummary, VariantKind, VcfHeader, VcfRecord,
};
use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use ratatui::{Terminal, backend::TestBackend};

/// Construct a populated VCF view at the requested terminal size with named sample columns.
fn fixture(width: u16, height: u16, samples: &[&str]) -> VcfView {
    let mut view = VcfView::new(
        "variants.vcf".into(),
        0,
        VcfConfig::default(),
        Theme::default(),
        Arc::new(AtomicU64::new(0)),
    );
    view.prepare(Rect::new(0, 1, width, height - 3));
    view.request_view();
    let request = view
        .actions()
        .into_iter()
        .find_map(|action| match action {
            Action::Request(ModuleRequest::Vcf(envelope)) => {
                if let Request::View(request) = envelope.payload {
                    Some(request)
                } else {
                    None
                }
            }
            _ => None,
        })
        .unwrap();
    let values = samples
        .iter()
        .map(|_| "0/1:1,2,3,4,5,6,7,8:36:90")
        .collect::<Vec<_>>()
        .join("\t");
    let raw = format!("chr1\t123456789\tlong-id\tA\tC\t99\tPASS\tDP=36\tGT:AD:DP:GQ\t{values}");
    let record = VcfRecord {
        raw,
        chrom: "chr1".into(),
        pos: 123456789,
        ids: vec!["long-id".into()],
        reference: "A".into(),
        alternates: vec!["C".into()],
        qual: Some(99.0),
        filter: "PASS".into(),
        format: ["GT", "AD", "DP", "GQ"].map(str::to_owned).into(),
    };
    view.update(ModuleUpdate::vcf(Update::View(Box::new(Snapshot {
        request,
        header: Arc::new(VcfHeader {
            version: "VCFv4.3".into(),
            samples: samples.iter().map(|s| (*s).to_owned()).collect(),
            ..Default::default()
        }),
        rows: vec![RecordSummary {
            id: 0,
            ordinal: 0,
            chrom: "chr1".into(),
            pos: 123456789,
            ids: "long-id".into(),
            reference: "A".into(),
            alternates: "C".into(),
            qual: Some(99.0),
            filter: "PASS".into(),
            kind: VariantKind::Snv,
        }],
        selected: Some(Arc::new(VcfEntry {
            record,
            variants: Vec::new(),
            graph: None,
        })),
        total: 1,
        contigs: vec![ContigSummary {
            name: "chr1".into(),
            records: 1,
        }],
        progress: Progress {
            records: 1,
            complete: true,
            ..Default::default()
        },
        compression: "none".into(),
        paused: false,
    }))));
    view
}

/// Dispatch one unmodified key press to the test view.
fn key(view: &mut VcfView, code: KeyCode) {
    view.key(KeyEvent::new(code, KeyModifiers::NONE));
}
/// Collect all cell symbols from a rendered terminal row.
fn row(terminal: &Terminal<TestBackend>, y: u16) -> String {
    let area = terminal.backend().buffer().area;
    (0..area.width)
        .map(|x| terminal.backend().buffer()[(x, y)].symbol())
        .collect()
}

/// Check constrained tables retain key variant columns and show the full locus in the header.
#[test]
fn narrow_table_prioritizes_required_columns_and_complete_coordinate() {
    let view = fixture(30, 24, &["S1"]);
    let mut terminal = Terminal::new(TestBackend::new(30, 24)).unwrap();
    terminal.draw(|frame| view.draw(frame)).unwrap();
    let heading = row(&terminal, view.layout.rows.y - 1);
    assert!(
        heading.contains("CHROM")
            && heading.contains("POS")
            && heading.contains("REF")
            && heading.contains("ALT"),
        "{heading}"
    );
    assert!(row(&terminal, view.layout.rows.y).contains("123456789"));
}

/// Verify short terminals either allocate useful detail rows or keep focus in the table.
#[test]
fn short_layout_either_has_readable_details_or_keeps_focus_in_table() {
    for height in 8..=20 {
        let mut view = fixture(80, height, &["S1"]);
        if view.layout.details.height > 0 {
            assert!(view.layout.detail_body.height >= 1, "height {height}");
        }
        key(&mut view, KeyCode::Enter);
        if view.layout.detail_body.height == 0 {
            assert_eq!(view.focus, Focus::Table);
        }
    }
}

/// Check heading and sample TAB stops stay aligned with long values and horizontal scrolling.
#[test]
fn sample_heading_long_name_and_long_ad_share_columns_after_horizontal_scroll() {
    let mut view = fixture(100, 30, &["abcdefghijklmnopqrstuvwxy", "S2"]);
    key(&mut view, KeyCode::Char('2'));
    let mut terminal = Terminal::new(TestBackend::new(100, 30)).unwrap();
    for horizontal in [0, 12, 28] {
        view.horizontal = horizontal;
        terminal.draw(|frame| view.draw(frame)).unwrap();
        let heading = row(&terminal, view.layout.detail_heading.y);
        let first = row(&terminal, view.layout.detail_body.y);
        let second = row(&terminal, view.layout.detail_body.y + 1);
        if let Some(at) = heading.find("GT") {
            assert_eq!(first.find("0/1"), Some(at));
            assert_eq!(second.find("0/1"), Some(at));
        }
        if let Some(at) = heading.find("DP") {
            assert_eq!(first.find("36"), Some(at));
            assert_eq!(second.find("36"), Some(at));
        }
        if let Some(at) = heading.find("GQ") {
            assert_eq!(first.find("90"), Some(at));
            assert_eq!(second.find("90"), Some(at));
        }
    }
}

/// Verify rendering preserves combining marks and wide Unicode glyphs as complete graphemes.
#[test]
fn details_keep_combining_graphemes_and_wide_characters() {
    let mut view = fixture(80, 24, &["e\u{301}中S"]);
    key(&mut view, KeyCode::Char('2'));
    let mut terminal = Terminal::new(TestBackend::new(80, 24)).unwrap();
    terminal.draw(|frame| view.draw(frame)).unwrap();
    let body = view.layout.detail_body;
    assert_eq!(
        terminal.backend().buffer()[(body.x, body.y)].symbol(),
        "e\u{301}"
    );
    assert_eq!(
        terminal.backend().buffer()[(body.x + 1, body.y)].symbol(),
        "中"
    );
    view.horizontal = 2;
    terminal.draw(|frame| view.draw(frame)).unwrap();
    assert_eq!(terminal.backend().buffer()[(body.x, body.y)].symbol(), " ");
    assert_eq!(
        terminal.backend().buffer()[(body.x + 1, body.y)].symbol(),
        "S"
    );
}

/// Ensure the raw panel fits its viewport while copy actions retain the complete source record.
#[test]
fn raw_view_clips_at_viewport_without_mutating_the_copied_record() {
    let mut view = fixture(80, 24, &["S1"]);
    let entry = Arc::make_mut(view.snapshot.as_mut().unwrap().selected.as_mut().unwrap());
    entry.record.raw.push_str(&"\t0/1".repeat(100_000));
    let original = entry.record.raw.clone();
    key(&mut view, KeyCode::Enter);
    key(&mut view, KeyCode::Char('3'));
    let mut terminal = Terminal::new(TestBackend::new(80, 24)).unwrap();
    terminal.draw(|frame| view.draw(frame)).unwrap();
    key(&mut view, KeyCode::Char('y'));
    assert!(
        view.actions()
            .into_iter()
            .any(|action| matches!(action,Action::Clipboard{text,..} if text==original))
    );
}

/// Check initial failures replace header, table and detail loading placeholders.
#[test]
fn initial_load_error_replaces_reading_placeholders() {
    let mut view = fixture(80, 24, &["S1"]);
    view.snapshot = None;
    view.update(ModuleUpdate::vcf(Update::Message {
        file: 0,
        text: "Invalid VCF column header".into(),
    }));
    let mut terminal = Terminal::new(TestBackend::new(80, 24)).unwrap();
    terminal.draw(|frame| view.draw(frame)).unwrap();
    let text = terminal
        .backend()
        .buffer()
        .content()
        .iter()
        .map(|cell| cell.symbol())
        .collect::<String>();
    assert!(text.contains("Error: Invalid VCF column header"));
    assert!(!text.contains("Reading selected record") && !text.contains("Reading records"));

    let mut view = fixture(80, 24, &["S1"]);
    let snapshot = view.snapshot.as_mut().unwrap();
    snapshot.rows.clear();
    snapshot.selected = None;
    snapshot.total = 0;
    snapshot.progress.complete = false;
    snapshot.progress.error = Some("At VCF line 4: Invalid POS field".into());
    terminal.draw(|frame| view.draw(frame)).unwrap();
    let text = terminal
        .backend()
        .buffer()
        .content()
        .iter()
        .map(|cell| cell.symbol())
        .collect::<String>();
    assert!(text.contains("Error: At VCF line 4: Invalid POS field"));
    assert!(!text.contains("Reading selected record") && !text.contains("Reading records"));
}
