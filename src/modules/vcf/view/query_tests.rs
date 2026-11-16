//! End-to-end query and statistics controls over the shared real-source fixture.
use super::{tests::Fixture, *};
use crossterm::event::KeyCode;

/// Submit a filter through its modal input, including ordinary text editing.
fn filter(fixture: &mut Fixture, expression: &str) {
    fixture.key(KeyCode::Char('f'));
    fixture.view.input = Some(Input::Filter(String::new()));
    for character in expression.chars() {
        fixture.key(KeyCode::Char(character));
    }
    fixture.key(KeyCode::Enter);
}

/// Filtered result ordinals still select and copy the correct original source record.
#[test]
fn filtering_and_ordering_preserve_record_identity_and_clear_cleanly() {
    let mut fixture = Fixture::new();
    filter(&mut fixture, "qual>=40 sort=qual");
    fixture.refresh();
    let snapshot = fixture.view.snapshot.as_ref().unwrap();
    assert_eq!(snapshot.total, 2);
    assert_eq!(
        snapshot.rows.iter().map(|row| row.id).collect::<Vec<_>>(),
        [2, 0]
    );
    assert_eq!(fixture.view.entry().unwrap().record.chrom, "chr10");
    fixture.key(KeyCode::Char('y'));
    assert!(fixture.copied().0.starts_with("chr10\t20\t"));

    fixture.key(KeyCode::Down);
    fixture.refresh();
    assert_eq!(fixture.view.entry().unwrap().record.pos, 10);
    fixture.key(KeyCode::Char('y'));
    assert!(fixture.copied().0.starts_with("chr1\t10\t"));

    filter(&mut fixture, "");
    fixture.refresh();
    assert_eq!(fixture.view.query, Query::default());
    assert_eq!(fixture.view.snapshot.as_ref().unwrap().total, 3);
    assert_eq!(fixture.view.entry().unwrap().record.pos, 10);
}

/// Invalid filters and Escape never replace the active query or accept an old snapshot.
#[test]
fn query_validation_and_cancellation_leave_the_current_results_intact() {
    let mut fixture = Fixture::new();
    let old = fixture.view.snapshot.as_ref().unwrap().clone();
    filter(&mut fixture, "pass");
    let query = fixture.view.query.clone();
    fixture.view.update(ModuleUpdate::vcf(Update::View(old)));
    assert!(fixture.view.entry().is_none());
    fixture.refresh();
    assert_eq!(fixture.view.snapshot.as_ref().unwrap().total, 1);

    filter(&mut fixture, "qual>=NaN");
    assert_eq!(fixture.view.query, query);
    assert!(matches!(fixture.view.input, Some(Input::Filter(_))));
    fixture.key(KeyCode::Esc);
    assert_eq!(fixture.view.query, query);
    assert!(fixture.view.entry().is_some());

    filter(&mut fixture, "qual>=1000");
    fixture.refresh();
    assert_eq!(fixture.view.snapshot.as_ref().unwrap().total, 0);
    assert!(fixture.view.entry().is_none());
    fixture.key(KeyCode::Char('y'));
    assert!(
        !fixture
            .view
            .actions()
            .iter()
            .any(|action| matches!(action, Action::Clipboard { .. }))
    );
}

/// A statistics response survives resizing, but closing its panel rejects stale results.
#[test]
fn statistics_have_an_independent_cancellable_operation_lifetime() {
    let mut fixture = Fixture::new();
    filter(&mut fixture, "pass");
    fixture.refresh();
    fixture.key(KeyCode::F(4));
    let (token, contig, query, cancel) = fixture
        .view
        .actions()
        .into_iter()
        .find_map(|action| {
            if let Action::Request(ModuleRequest::Vcf(envelope)) = action
                && let Request::Statistics {
                    token,
                    contig,
                    query,
                    cancel,
                    ..
                } = envelope.payload
            {
                return Some((token, contig, query, cancel));
            }
            None
        })
        .unwrap();
    assert!(query.pass_only);
    let statistics = fixture
        .dataset
        .statistics(contig.as_deref(), &query, Arc::clone(&cancel))
        .unwrap();
    assert_eq!(statistics.records, 1);
    fixture.view.prepare(Rect::new(0, 0, 60, 20));
    fixture.view.request_view();
    fixture.view.update(ModuleUpdate::vcf(Update::Statistics {
        file: 0,
        token,
        statistics: statistics.clone(),
    }));
    assert_eq!(fixture.view.statistics.as_ref().unwrap().records, 1);
    assert!(
        fixture
            .view
            .info
            .lines
            .join("\n")
            .contains("Matching records: 1")
    );
    fixture.key(KeyCode::Esc);
    assert!(cancel.load(Ordering::Relaxed));
    fixture.view.statistics = None;
    fixture.view.update(ModuleUpdate::vcf(Update::Statistics {
        file: 0,
        token,
        statistics,
    }));
    assert!(fixture.view.statistics.is_none());
}
