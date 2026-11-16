//! Sample-navigation regressions through document input, rendering and clipboard actions.

use super::{Detail, Focus, Input, VcfView, tests::Fixture};
use crate::modules::view::DocumentView;
use crate::modules::{vcf::palette, view::Action};
use crossterm::event::KeyCode;
use ratatui::{Terminal, backend::TestBackend};
use std::sync::Arc;

/// Enter text into the active prompt using the same character events as a terminal.
fn type_text(fixture: &mut Fixture, text: &str) {
    for character in text.chars() {
        fixture.key(KeyCode::Char(character));
    }
}

/// Replace the editable FORMAT expression without applying it until the final key press.
fn edit_fields(fixture: &mut Fixture, text: &str, finish: KeyCode) {
    fixture.key(KeyCode::Char('F'));
    let Some(Input::Fields(current)) = &fixture.view.input else {
        panic!("FORMAT prompt was not opened");
    };
    for _ in 0..current.chars().count() {
        fixture.key(KeyCode::Backspace);
    }
    type_text(fixture, text);
    fixture.key(finish);
}

/// Find a sample by its name and commit the navigation through the public key handler.
fn find_sample(fixture: &mut Fixture, name: &str) {
    fixture.key(KeyCode::Char('/'));
    type_text(fixture, name);
    fixture.key(KeyCode::Enter);
}

/// Render one VCF page into an inspectable terminal buffer.
fn render(view: &VcfView) -> Terminal<TestBackend> {
    let mut terminal = Terminal::new(TestBackend::new(100, 30)).unwrap();
    terminal.draw(|frame| view.draw(frame)).unwrap();
    terminal
}

/// Collect terminal cell text without replacing source-column identity with displayed text.
fn row(terminal: &Terminal<TestBackend>, y: u16) -> String {
    let buffer = terminal.backend().buffer();
    (0..buffer.area.width)
        .map(|x| buffer[(x, y)].symbol())
        .collect()
}

/// Find a heading's terminal column, accounting for multibyte border characters.
fn column(line: &str, text: &str) -> u16 {
    line[..line.find(text).unwrap()].chars().count() as u16
}

/// Pinned rows and projected FORMAT columns must still refer to the original sample.
#[test]
fn sample_search_pin_and_format_selection_copy_the_source_sample() {
    let mut fixture = Fixture::new();
    find_sample(&mut fixture, "s2");
    assert_eq!(fixture.view.detail, Detail::Samples);
    assert_eq!(fixture.view.focus, Focus::Detail);
    assert_eq!(fixture.view.detail_row, 1);
    fixture.key(KeyCode::Char('p'));
    fixture.key(KeyCode::Char('P'));
    assert!(fixture.view.samples.pinned_only());
    assert_eq!(fixture.view.samples.pinned_count(), 1);
    assert_eq!(fixture.view.detail_len(), 1);
    assert_eq!(fixture.view.detail_row, 0);
    edit_fields(&mut fixture, "DP,GT", KeyCode::Enter);
    fixture.key(KeyCode::Char('y'));
    assert_eq!(fixture.copied().0, "SAMPLE\tDP\tGT\nS2\t8\t1/2");
    fixture.key(KeyCode::Char('n'));
    assert_eq!(fixture.view.detail_row, 0);
    fixture.key(KeyCode::Char('P'));
    assert_eq!(fixture.view.detail_row, 1);
    assert_eq!(fixture.view.detail_rows(1, 1), ["S2\t8\t1/2"]);
}

/// GT token colours and AD bar segments follow their identifiers after column reordering.
#[test]
fn reordered_format_columns_keep_genotype_and_depth_colours() {
    let mut fixture = Fixture::new();
    edit_fields(&mut fixture, "DP,GT,AD", KeyCode::Enter);
    let terminal = render(&fixture.view);
    let heading = row(&terminal, fixture.view.layout.detail_heading.y);
    let y = fixture.view.layout.detail_body.y;
    let sample = row(&terminal, y);
    let dp = column(&heading, "DP");
    let gt = column(&heading, "GT");
    let ad = column(&heading, "AD");
    let buffer = terminal.backend().buffer();
    for (column, value) in [(dp, "20"), (gt, "0|10"), (ad, "0,4,0,6")] {
        assert_eq!(
            sample
                .chars()
                .skip(column as usize)
                .take(value.len())
                .collect::<String>(),
            value
        );
    }
    assert_eq!(buffer[(dp, y)].fg, fixture.view.theme.foreground);
    assert_eq!(
        buffer[(gt + 2, y)].fg,
        palette::allele_color(10, &fixture.view.config, &fixture.view.theme)
    );
    let bars = (0..buffer.area.width)
        .filter(|&x| buffer[(x, y)].symbol() == "█")
        .map(|x| buffer[(x, y)].fg)
        .collect::<Vec<_>>();
    assert!(!bars.is_empty());
    for allele in [1, 3] {
        assert!(bars.contains(&palette::allele_color(
            allele,
            &fixture.view.config,
            &fixture.view.theme
        )));
    }
    assert!(!bars.contains(&palette::allele_color(
        2,
        &fixture.view.config,
        &fixture.view.theme
    )));
    edit_fields(&mut fixture, "DP,GT", KeyCode::Enter);
    let terminal = render(&fixture.view);
    assert!(!row(&terminal, y).contains('█'));
}

/// Failed searches, cancelled input and field validation preserve the chosen sample and columns.
#[test]
fn sample_editing_and_failed_search_preserve_columns_and_selection() {
    let mut fixture = Fixture::new();
    edit_fields(&mut fixture, "DP,GT", KeyCode::Enter);
    edit_fields(&mut fixture, "GT", KeyCode::Esc);
    assert_eq!(fixture.view.sample_heading(), "SAMPLE\tDP\tGT");
    edit_fields(&mut fixture, "GT,UNKNOWN", KeyCode::Enter);
    assert!(matches!(fixture.view.input, Some(Input::Fields(_))));
    assert!(fixture.view.message.contains("Unknown FORMAT field"));
    assert_eq!(fixture.view.sample_heading(), "SAMPLE\tDP\tGT");
    fixture.key(KeyCode::Esc);
    find_sample(&mut fixture, "S2");
    edit_fields(&mut fixture, "GT,DP", KeyCode::Enter);
    assert_eq!(fixture.view.detail_row, 1);
    find_sample(&mut fixture, "absent-sample");
    assert!(fixture.view.message.contains("No sample matches"));
    assert_eq!(fixture.view.detail_row, 1);
    fixture.key(KeyCode::Char('/'));
    type_text(&mut fixture, "S1");
    fixture.key(KeyCode::Esc);
    assert_eq!(fixture.view.detail_row, 1);
    fixture.key(KeyCode::Char('y'));
    assert_eq!(fixture.copied().0, "SAMPLE\tGT\tDP\nS2\t1/2\t8");
}

/// A selected FORMAT field survives navigation to a record that omits that identifier.
#[test]
fn format_selection_persists_across_records_and_pads_absent_fields() {
    let mut fixture = Fixture::new();
    edit_fields(&mut fixture, "DP,GT", KeyCode::Enter);
    fixture.view.change_record(1);
    fixture.refresh();
    let entry = Arc::make_mut(
        fixture
            .view
            .snapshot
            .as_mut()
            .unwrap()
            .selected
            .as_mut()
            .unwrap(),
    );
    // Replace the selected snapshot with a valid next-record FORMAT layout.
    entry.record.format = vec!["GT".into()];
    entry.record.raw = "chr1\t30\t.\tT\tC\t.\t.\tDP=4\tGT\t0/1\t0/0".into();
    fixture.key(KeyCode::Enter);
    fixture.key(KeyCode::Char('2'));
    fixture.key(KeyCode::Char('y'));
    assert_eq!(fixture.copied().0, "SAMPLE\tDP\tGT\nS1\t.\t0/1");
    assert_eq!(fixture.view.detail_rows(0, 2), ["S1\t.\t0/1", "S2\t.\t0/0"]);
}

/// An empty pinned subset must not copy a hidden sample or a heading without a row.
#[test]
fn empty_pinned_sample_list_does_not_queue_clipboard_text() {
    let mut fixture = Fixture::new();
    fixture.key(KeyCode::Char('P'));
    assert_eq!(fixture.view.detail_len(), 0);
    fixture.key(KeyCode::Char('y'));
    assert!(
        !fixture
            .view
            .actions()
            .iter()
            .any(|action| matches!(action, Action::Clipboard { .. }))
    );
    assert_eq!(fixture.view.message, "Nothing to copy");
    fixture.key(KeyCode::Char('p'));
    fixture.key(KeyCode::Char('P'));
    assert_eq!(fixture.view.detail_len(), 2);
    fixture.key(KeyCode::Char('y'));
    assert!(
        fixture
            .copied()
            .0
            .starts_with("SAMPLE\tGT\tAD\tDP\tGQ\nS1\t")
    );
}
