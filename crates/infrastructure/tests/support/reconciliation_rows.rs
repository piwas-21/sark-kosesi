//! Independent literal operational header bounds, paired with healthy states.
use super::*;
fn row() -> Row {
    Row {
        attempt_id: "1".repeat(32),
        source_event_id: "2".repeat(32),
        created_at: 1500,
        updated_at: 1500,
        due_at: 1500,
        generation: 0,
        resume_revision: 0,
        lease_until: None,
        phase: "search".into(),
        page_id: None,
        candidate_index: 0,
        operation: 0,
        polls: 0,
        failures: 0,
        review_reason: None,
        source_present: true,
    }
}
#[test]
fn operational_headers_require_source_identity_bounded_progress_and_consistent_review_state() {
    assert!(row().decode().is_ok());
    let changes: [fn(&mut Row); 27] = [
        |r| r.resume_revision = -1,
        |r| r.resume_revision = 4,
        |r| r.source_present = false,
        |r| r.attempt_id = "bad".into(),
        |r| r.source_event_id = "bad".into(),
        |r| r.created_at = -1,
        |r| r.updated_at = 1499,
        |r| r.due_at = -1,
        |r| r.generation = 1501,
        |r| {
            r.generation = 1;
            r.lease_until = Some(1499);
        },
        |r| r.page_id = Some("bad".into()),
        |r| r.candidate_index = 11,
        |r| r.operation = 1501,
        |r| r.operation = 1,
        |r| r.failures = 5,
        |r| {
            r.generation = 1;
            r.lease_until = Some(1561);
        },
        |r| r.polls = 11,
        |r| r.polls = 10,
        |r| r.failures = 6,
        |r| r.phase = "unknown".into(),
        |r| r.review_reason = Some("unknown".into()),
        |r| r.phase = "candidates".into(),
        |r| r.phase = "needs_review".into(),
        |r| r.review_reason = Some("unresolved".into()),
        |r| {
            r.phase = "needs_review".into();
            r.review_reason = Some("unresolved".into());
            r.generation = 1;
            r.lease_until = Some(1560);
        },
        |r| r.lease_until = Some(9_007_199_254_740_992),
        |r| {
            r.generation = 1;
            r.updated_at = 9_007_199_254_740_991;
            r.due_at = 9_007_199_254_740_991;
            r.lease_until = Some(9_007_199_254_740_992);
        },
    ];
    for change in changes {
        let mut r = row();
        change(&mut r);
        assert_eq!(r.decode().err(), Some(RecoveryError::InvalidStoredData));
    }
    let mut candidate = row();
    candidate.phase = "candidates".into();
    candidate.page_id = Some("3".repeat(32));
    candidate.lease_until = Some(1560);
    candidate.generation = 1;
    assert!(candidate.decode().is_ok());
    let mut review = row();
    review.phase = "needs_review".into();
    review.review_reason = Some("paid_observed".into());
    assert!(review.decode().is_ok());
}
