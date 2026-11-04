//! Reconcile indexed counts and canonical allocation with validated decisions.
use super::*;
use disposition::{DecisionOutcome, DecisionReceipt};

fn decision(outcome: DecisionOutcome) -> DecisionReceipt {
    DecisionReceipt {
        id: String::new(),
        case_id: String::new(),
        case_digest: String::new(),
        order_id: String::new(),
        attempt_id: String::new(),
        recognition_id: String::new(),
        actor_id: String::new(),
        effect_key: String::new(),
        generation: 1,
        reason: String::new(),
        recorded_at: 0,
        outcome,
        replayed: false,
    }
}

#[test]
fn open_case_requires_matching_count_and_no_allocated_outcome() {
    assert_eq!(validate_decisions(&[], 0, None), Ok(()));
    let scarce = [decision(DecisionOutcome::StillScarce)];
    assert_eq!(validate_decisions(&scarce, 1, None), Ok(()));
    assert_eq!(
        validate_decisions(&scarce, 0, None),
        Err(LateStockError::InvalidStoredData)
    );
    let allocated = [decision(DecisionOutcome::Allocated {
        allocation_id: "allocation".into(),
    })];
    assert_eq!(
        validate_decisions(&allocated, 1, None),
        Err(LateStockError::InvalidStoredData)
    );
}

#[test]
fn resolved_case_requires_exact_last_allocation_and_matching_count() {
    let history = [
        decision(DecisionOutcome::StillScarce),
        decision(DecisionOutcome::Allocated {
            allocation_id: "allocation".into(),
        }),
    ];
    assert_eq!(validate_decisions(&history, 2, Some("allocation")), Ok(()));
    assert_eq!(
        validate_decisions(&history, 1, Some("allocation")),
        Err(LateStockError::InvalidStoredData)
    );
    assert_eq!(
        validate_decisions(&history, 2, Some("different")),
        Err(LateStockError::InvalidStoredData)
    );
    assert_eq!(
        validate_decisions(&[], 0, Some("allocation")),
        Err(LateStockError::InvalidStoredData)
    );
    let scarce = [decision(DecisionOutcome::StillScarce)];
    assert_eq!(
        validate_decisions(&scarce, 1, Some("allocation")),
        Err(LateStockError::InvalidStoredData)
    );
}

#[test]
fn canonical_receipt_must_match_both_claimed_identity_and_attempt() {
    assert_eq!(
        validate_receipt_binding("receipt", "attempt", "receipt", "attempt"),
        Ok(())
    );
    assert_eq!(
        validate_receipt_binding("other", "attempt", "receipt", "attempt"),
        Err(LateStockError::InvalidStoredData)
    );
    assert_eq!(
        validate_receipt_binding("receipt", "other", "receipt", "attempt"),
        Err(LateStockError::InvalidStoredData)
    );
}
