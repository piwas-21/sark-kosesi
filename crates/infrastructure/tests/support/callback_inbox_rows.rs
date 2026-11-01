//! Closed private row/list readers reject independently malformed imported headers.
use super::*;
fn row() -> Row {
    Row {
        intake_id: "1".repeat(32),
        source_event_id: "2".repeat(32),
        created_at: 1500,
        updated_at: 1500,
        due_at: 1500,
        generation: 0,
        lease_until: None,
        state: "pending".into(),
        review_reason: None,
        settled: false,
        source_present: true,
        generation_present: true,
        active_exact: true,
        settlement_exact: true,
    }
}
#[test]
fn independent_job_row_header_and_custody_controls_have_a_healthy_positive() {
    let changes: [fn(&mut Row); 12] = [
        |r| r.source_present = false,
        |r| r.generation_present = false,
        |r| r.active_exact = false,
        |r| r.settlement_exact = false,
        |r| r.state = "unknown".into(),
        |r| r.review_reason = Some("unknown".into()),
        |r| r.intake_id = "bad".into(),
        |r| r.source_event_id = "bad".into(),
        |r| r.created_at = -1,
        |r| r.updated_at = 1499,
        |r| r.generation = 6,
        |r| r.settled = true,
    ];
    for change in changes {
        let mut r = row();
        change(&mut r);
        assert_eq!(r.decode().err(), Some(PaymentStoreError::InvalidStoredData));
    }
    let healthy = row().decode().expect("exact pending header");
    assert!(healthy.state == JobState::Pending);
    assert_eq!(healthy.generation, 0);
    assert_eq!(
        state(JobState::NeedsReview).expect("closed state"),
        "needs_review"
    );
    assert_eq!(
        reason(Some(ReviewReason::GenerationLimit)).expect("closed reason"),
        Some("generation_limit".into())
    );
}
#[test]
fn imported_identifier_walks_reject_oversize_or_bad_ids_before_processing() {
    assert!(bounded_operations(vec!["1".repeat(32); 5]).is_ok());
    assert!(bounded_operations(Vec::new()).is_ok());
    assert_eq!(
        bounded_operations(vec!["1".repeat(32); 6]).err(),
        Some(PaymentStoreError::InvalidStoredData)
    );
    assert_eq!(
        bounded_operations(vec!["bad".into()]).err(),
        Some(PaymentStoreError::InvalidStoredData)
    );
    assert!(bounded_intakes(vec!["1".repeat(32); 200]).is_ok());
    assert!(bounded_intakes(Vec::new()).is_ok());
    assert_eq!(
        bounded_intakes(vec!["1".repeat(32); 201]).err(),
        Some(PaymentStoreError::InvalidStoredData)
    );
    assert_eq!(
        bounded_intakes(vec!["bad".into()]).err(),
        Some(PaymentStoreError::InvalidStoredData)
    );
}
