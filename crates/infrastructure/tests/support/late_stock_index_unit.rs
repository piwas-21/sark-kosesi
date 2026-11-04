//! Stored index snapshots must describe one coherent ownership path.
use super::*;

const ATTEMPT: &str = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";
const CASE: &str = "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb";
const RECEIPT: &str = "cccccccccccccccccccccccccccccccc";

fn empty() -> Index {
    Index {
        attempt_id: None,
        kind: None,
        receipt_id: None,
        review: false,
        review_id: None,
        review_attempt_id: None,
        original_id: None,
        late_id: None,
        original_event: false,
        late_event: false,
        review_event: false,
        decisions: 0,
        decision_events: 0,
        orphan: false,
    }
}
fn review() -> Index {
    Index {
        review: true,
        review_id: Some(CASE.into()),
        review_attempt_id: Some(ATTEMPT.into()),
        review_event: true,
        orphan: true,
        ..empty()
    }
}
fn allocation(kind: &str) -> Index {
    let mut index = empty();
    index.attempt_id = Some(ATTEMPT.into());
    index.kind = Some(kind.into());
    index.receipt_id = Some(RECEIPT.into());
    index.orphan = true;
    if kind == "original" {
        index.original_id = Some(RECEIPT.into());
        index.original_event = true;
    } else {
        index.late_id = Some(RECEIPT.into());
        index.late_event = true;
    }
    index
}
fn allocated_review() -> Index {
    let mut index = allocation("late");
    index.review = true;
    index.review_id = Some(CASE.into());
    index.review_attempt_id = Some(ATTEMPT.into());
    index.review_event = true;
    index.decisions = 1;
    index.decision_events = 1;
    index
}

#[test]
fn intact_empty_review_and_both_allocation_paths_are_admitted() {
    let mut scarce = review();
    scarce.decisions = 3;
    scarce.decision_events = 3;
    for (name, index) in [
        ("empty", empty()),
        ("initial review", review()),
        ("three still-scarce decisions", scarce),
        ("original allocation", allocation("original")),
        ("automatic late allocation", allocation("late")),
        ("allocated review", allocated_review()),
    ] {
        assert_eq!(index.validate(), Ok(()), "{name}");
    }
}

#[test]
fn damaged_index_combinations_fail_closed() {
    type Damage = (&'static str, Index, fn(&mut Index));
    let cases: Vec<Damage> = vec![
        ("missing case ID", review(), |i| i.review_id = None),
        ("bad case ID", review(), |i| {
            i.review_id = Some("bad".into())
        }),
        ("missing case attempt", review(), |i| {
            i.review_attempt_id = None
        }),
        ("bad case attempt", review(), |i| {
            i.review_attempt_id = Some("bad".into())
        }),
        ("case ID without case", empty(), |i| {
            i.review_id = Some(CASE.into())
        }),
        ("case attempt without case", empty(), |i| {
            i.review_attempt_id = Some(ATTEMPT.into())
        }),
        ("negative count", review(), |i| {
            i.decisions = -1;
            i.decision_events = -1;
        }),
        ("too many decisions", review(), |i| {
            i.decisions = 4;
            i.decision_events = 4;
        }),
        ("missing decision event", review(), |i| i.decisions = 1),
        ("detached decisions", empty(), |i| {
            i.decisions = 1;
            i.decision_events = 1;
        }),
        ("missing attempt", allocation("late"), |i| {
            i.attempt_id = None
        }),
        ("invalid attempt", allocation("late"), |i| {
            i.attempt_id = Some("bad".into())
        }),
        ("missing kind", allocation("late"), |i| i.kind = None),
        ("unknown kind", allocation("late"), |i| {
            i.kind = Some("other".into())
        }),
        ("missing receipt", allocation("late"), |i| {
            i.receipt_id = None
        }),
        ("invalid receipt", allocation("late"), |i| {
            i.receipt_id = Some("bad".into())
        }),
        ("wrong original receipt", allocation("original"), |i| {
            i.original_id = Some(CASE.into())
        }),
        ("original with late body", allocation("original"), |i| {
            i.late_id = Some(CASE.into())
        }),
        ("original with late event", allocation("original"), |i| {
            i.late_event = true
        }),
        ("wrong late receipt", allocation("late"), |i| {
            i.late_id = Some(CASE.into())
        }),
        ("late with original body", allocation("late"), |i| {
            i.original_id = Some(CASE.into())
        }),
        ("late with original event", allocation("late"), |i| {
            i.original_event = true
        }),
        ("original with review", allocation("original"), |i| {
            i.review = true;
            i.review_id = Some(CASE.into());
            i.review_attempt_id = Some(ATTEMPT.into());
            i.review_event = true;
        }),
        ("late review missing event", allocated_review(), |i| {
            i.review_event = false
        }),
        ("late event without review", allocation("late"), |i| {
            i.review_event = true
        }),
        ("open case with original body", review(), |i| {
            i.original_id = Some(RECEIPT.into())
        }),
        ("open case with late body", review(), |i| {
            i.late_id = Some(RECEIPT.into())
        }),
        ("open case with original event", review(), |i| {
            i.original_event = true
        }),
        ("open case with late event", review(), |i| {
            i.late_event = true
        }),
        ("orphan source", empty(), |i| i.orphan = true),
    ];
    for (name, mut index, damage) in cases {
        assert_eq!(index.validate(), Ok(()), "positive control: {name}");
        damage(&mut index);
        assert_eq!(
            index.validate(),
            Err(LateStockError::InvalidStoredData),
            "{name}"
        );
    }
}

#[test]
fn review_binding_rejects_changed_or_missing_case_after_lock() {
    let open = review();
    assert_eq!(open.require_review_binding(CASE, ATTEMPT), Ok(()));
    let resolved = allocated_review();
    assert_eq!(resolved.require_review_binding(CASE, ATTEMPT), Ok(()));
    assert_eq!(
        empty().require_review_binding(CASE, ATTEMPT),
        Err(LateStockError::InvalidStoredData)
    );
    assert_eq!(
        open.require_review_binding(RECEIPT, ATTEMPT),
        Err(LateStockError::InvalidStoredData)
    );
    assert_eq!(
        open.require_review_binding(CASE, RECEIPT),
        Err(LateStockError::InvalidStoredData)
    );
}
