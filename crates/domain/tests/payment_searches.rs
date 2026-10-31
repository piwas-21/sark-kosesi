use commerce_domain::payment_attempts::{PaymentError, searches::*};
#[path = "support/payment_search_fixture.rs"]
mod fixture;
#[test]
fn empty_end_and_multiple_candidates_preserve_uncertainty_and_exact_continuation() {
    let a = fixture::attempt();
    let first = fixture::first();
    let second = fixture::second();
    assert!(first.validate(&a, 1200).is_ok());
    assert!(second.validate(&a, 1200).is_ok());
    assert!(second.follows(&first));
    assert_eq!(first.intent.candidates, vec!["tr_ONE", "tr_TWO"]);
    assert!(second.intent.next.is_none() && second.intent.candidates.is_empty());
    let mut last = second.clone();
    last.page_number = 100;
    last.revision = 100;
    last.intent.from = Some("tr_Z".into());
    last.intent.next = Some("tr_CONTINUE_LATER".into());
    assert!(
        last.validate(&a, 1200).is_ok(),
        "cap remains visibly unresolved"
    );
}
#[test]
fn every_header_boundary_and_original_dispatch_are_validated() {
    let changes: [fn(&mut SearchSnapshot); 12] = [
        |s| s.schema_version = 2,
        |s| s.id = "invalid".into(),
        |s| s.attempt_id = "2".repeat(32),
        |s| s.effect_key = "!".into(),
        |s| s.revision = 0,
        |s| s.revision = 9_007_199_254_740_992,
        |s| s.page_number = 0,
        |s| s.page_number = 101,
        |s| s.page_number = 2,
        |s| s.recorded_at = 1199,
        |s| s.recorded_at = 9_007_199_254_740_992,
        |s| s.intent.credential_fingerprint = "!".into(),
    ];
    for change in changes {
        let mut s = fixture::first();
        change(&mut s);
        assert!(s.validate(&fixture::attempt(), 1200).is_err());
    }
    assert!(
        fixture::first()
            .validate(&fixture::attempt(), 1099)
            .is_err()
    );
    let mut bad = fixture::attempt();
    bad.binding.amount_minor = 0;
    assert!(fixture::first().validate(&bad, 1200).is_err());
    let mut s = fixture::second();
    s.page_number = 1;
    assert!(s.validate(&fixture::attempt(), 1200).is_err());
}
#[test]
fn bounded_cursors_candidate_sets_and_effect_intent_reject_bad_shapes() {
    let changes: [fn(&mut SearchIntent); 9] = [
        |s| s.previous_page_id = Some("!".into()),
        |s| s.previous_page_id = None,
        |s| s.from = None,
        |s| s.from = Some("bad".into()),
        |s| s.next = Some("bad".into()),
        |s| s.next = s.from.clone(),
        |s| s.candidates = vec!["bad".into()],
        |s| s.candidates = vec!["tr_A".into(); 2],
        |s| s.candidates = (0..11).map(|n| format!("tr_{n}")).collect(),
    ];
    for change in changes {
        let mut s = fixture::second().intent;
        change(&mut s);
        assert_eq!(s.validate(), Err(PaymentError::InvalidSnapshot));
    }
    for id in [
        "tr_".into(),
        "tr_bad\n".into(),
        format!("tr_{}", "x".repeat(254)),
    ] {
        let mut s = fixture::first().intent;
        s.candidates = vec![id];
        assert!(s.validate().is_err());
    }
    let mut ten = fixture::first().intent;
    ten.candidates = (0..10).map(|n| format!("tr_{n}")).collect();
    assert!(ten.validate().is_ok());
}
#[test]
fn continuation_requires_every_exact_predecessor_fact() {
    let first = fixture::first();
    let changes: [fn(&mut SearchSnapshot); 7] = [
        |s| s.attempt_id = "2".repeat(32),
        |s| s.intent.previous_page_id = None,
        |s| s.intent.previous_page_id = Some("7".repeat(32)),
        |s| s.intent.from = Some("tr_B".into()),
        |s| s.page_number = 3,
        |s| s.revision = 1,
        |s| s.recorded_at = 4999,
    ];
    for change in changes {
        let mut s = fixture::second();
        change(&mut s);
        assert!(!s.follows(&first));
    }
    let mut terminal = first.clone();
    terminal.intent.next = None;
    assert!(!fixture::second().follows(&terminal));
    let mut overflow = first;
    overflow.page_number = i64::MAX;
    assert!(!fixture::second().follows(&overflow));
}
