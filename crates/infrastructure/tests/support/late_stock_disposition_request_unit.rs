use super::*;
use commerce_domain::order_stock::{Balance, Member, Quantity};

const ID: &str = "0123456789abcdef0123456789abcdef";
const OTHER_ID: &str = "abcdef0123456789abcdef0123456789";
const OTHER_DIGEST: &str = "abcdef0123456789abcdef0123456789abcdef0123456789abcdef0123456789";
const DIGEST: &str = "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef";

fn request() -> RetryRequest<'static> {
    RetryRequest {
        case_id: ID,
        case_digest: DIGEST,
        effect_key: ID,
        expected_generation: 1,
        reason: "Stock was counted",
        confirmed: true,
    }
}

fn invalid(request: &RetryRequest<'_>) {
    assert!(matches!(
        validate_request(request),
        Err(DispositionError::InvalidRequest)
    ));
}

#[test]
fn request_requires_exact_identifiers_and_digest() {
    assert!(validate_request(&request()).is_ok());
    let mut value = request();
    value.case_id = "ABC";
    invalid(&value);
    let mut value = request();
    value.case_digest = ID;
    invalid(&value);
    let uppercase = "A".repeat(64);
    value.case_digest = &uppercase;
    invalid(&value);
    let mut value = request();
    value.effect_key = "short";
    invalid(&value);
    let invalid_hex = "g".repeat(32);
    value.effect_key = &invalid_hex;
    invalid(&value);
}

#[test]
fn request_requires_bounded_generation_reason_and_confirmation() {
    for generation in [1, 2, 3] {
        let mut value = request();
        value.expected_generation = generation;
        assert!(validate_request(&value).is_ok());
    }
    for generation in [0, 4, -1] {
        let mut value = request();
        value.expected_generation = generation;
        invalid(&value);
    }
    let long_reason = "x".repeat(241);
    for reason in ["", " reason", "reason ", "rea\0son", long_reason.as_str()] {
        let mut value = request();
        value.reason = reason;
        invalid(&value);
    }
    let max_reason = "x".repeat(240);
    let mut value = request();
    value.reason = &max_reason;
    assert!(validate_request(&value).is_ok());
    value.confirmed = false;
    invalid(&value);
}

#[test]
fn receipt_debug_redacts_reason_and_private_identifiers() {
    let receipt = DecisionReceipt {
        id: ID.into(),
        case_id: ID.into(),
        case_digest: DIGEST.into(),
        order_id: "private-order".into(),
        attempt_id: "private-attempt".into(),
        recognition_id: "private-recognition".into(),
        actor_id: "private-actor".into(),
        effect_key: "private-key".into(),
        generation: 1,
        reason: "private-reason".into(),
        recorded_at: 42,
        outcome: DecisionOutcome::StillScarce,
        replayed: false,
    };
    let debug = format!("{receipt:?}");
    assert!(debug.contains("[redacted]"));
    assert!(debug.contains("StillScarce"));
    for secret in [
        "private-order",
        "private-attempt",
        "private-recognition",
        "private-actor",
        "private-key",
        "private-reason",
        DIGEST,
    ] {
        assert!(!debug.contains(secret));
    }
}

#[test]
fn database_errors_remain_custody_errors() {
    assert!(matches!(
        DispositionError::from(sqlx::Error::RowNotFound),
        DispositionError::Custody(LateStockError::Database)
    ));
}

fn receipt() -> DecisionReceipt {
    DecisionReceipt {
        id: ID.into(),
        case_id: ID.into(),
        case_digest: DIGEST.into(),
        order_id: ID.into(),
        attempt_id: ID.into(),
        recognition_id: ID.into(),
        actor_id: "staff-1".into(),
        effect_key: ID.into(),
        generation: 1,
        reason: "Stock was counted".into(),
        recorded_at: 42,
        outcome: DecisionOutcome::StillScarce,
        replayed: false,
    }
}
fn case(reason: ReviewReason) -> ReviewCase {
    let state = match reason {
        ReviewReason::ComponentScarcity => OriginalState::Expired,
        ReviewReason::ReleaseIntentUnresolved => OriginalState::Released,
        ReviewReason::PriorConsumptionWithoutAllocation => OriginalState::Consumed,
    };
    ReviewCase {
        id: ID.into(),
        order_id: ID.into(),
        attempt_id: ID.into(),
        recognition_id: ID.into(),
        group_id: ID.into(),
        actor: "late-stock-review".into(),
        reason,
        assessed_at: 42,
        footprint: ReviewFootprint {
            schema_version: 1,
            original: vec![OriginalMember {
                member: Member {
                    hold_id: ID.into(),
                    sku: "SET-COVERS".into(),
                    revision: 1,
                    units: 1,
                    components: vec![Quantity {
                        component_code: "SEAT-COVER".into(),
                        quantity: 1,
                    }],
                },
                state,
                pairs: vec![PairReference {
                    component_code: "SEAT-COVER".into(),
                    reserve_sequence: 1,
                    reserve_digest: DIGEST.into(),
                    terminal_sequence: Some(2),
                    terminal_digest: Some(DIGEST.into()),
                }],
            }],
            components: vec![AssessedComponent {
                component_code: "SEAT-COVER".into(),
                demand: 1,
                active: 0,
                before: Balance {
                    on_hand: 0,
                    quarantine: 0,
                    reserved: 0,
                },
            }],
        },
    }
}

#[test]
fn stored_ids_must_be_canonical_before_attempt_claim() {
    assert!(require_stored_ids(&[ID, ID]).is_ok());
    assert_eq!(
        require_stored_ids(&["bad"]),
        Err(LateStockError::InvalidStoredData)
    );
    assert_eq!(
        require_stored_ids(&[ID, "bad"]),
        Err(LateStockError::InvalidStoredData)
    );
}

#[test]
fn fresh_retry_requires_the_same_retained_open_review() {
    let retained = case(ReviewReason::ComponentScarcity);
    assert!(retained.validate().is_ok());
    assert!(require_open_review(Some(Outcome::NeedsReview(retained.clone())), ID).is_ok());
    assert!(matches!(
        require_open_review(Some(Outcome::NeedsReview(retained)), OTHER_ID),
        Err(DispositionError::Custody(LateStockError::InvalidStoredData))
    ));
    assert!(matches!(
        require_open_review(None, ID),
        Err(DispositionError::Custody(LateStockError::InvalidStoredData))
    ));
}

#[test]
fn replay_must_exist_exactly_in_validated_history() {
    let prior = receipt();
    assert!(replay_in_history(&prior, &[receipt()]).is_ok());
    assert_eq!(
        replay_in_history(&prior, &[]),
        Err(LateStockError::InvalidStoredData)
    );
    let mut changed = receipt();
    changed.outcome = DecisionOutcome::Allocated {
        allocation_id: ID.into(),
    };
    assert_eq!(
        replay_in_history(&prior, &[changed]),
        Err(LateStockError::InvalidStoredData)
    );
}

#[test]
fn replay_requires_original_intent_and_current_actor() {
    assert!(validate_replay_intent(&receipt(), &request(), "staff-1").is_ok());
    let mut value = receipt();
    value.case_id = OTHER_ID.into();
    assert!(matches!(
        validate_replay_intent(&value, &request(), "staff-1"),
        Err(DispositionError::Conflict)
    ));
    let mut value = receipt();
    value.case_digest = OTHER_DIGEST.into();
    assert!(matches!(
        validate_replay_intent(&value, &request(), "staff-1"),
        Err(DispositionError::Conflict)
    ));
    let mut value = receipt();
    value.generation = 2;
    assert!(matches!(
        validate_replay_intent(&value, &request(), "staff-1"),
        Err(DispositionError::Conflict)
    ));
    let mut value = receipt();
    value.reason = "different".into();
    assert!(matches!(
        validate_replay_intent(&value, &request(), "staff-1"),
        Err(DispositionError::Conflict)
    ));
    assert!(matches!(
        validate_replay_intent(&receipt(), &request(), "staff-2"),
        Err(DispositionError::Conflict)
    ));
}

#[test]
fn new_decision_binds_exact_scarcity_case_and_attempt() {
    let scarcity = case(ReviewReason::ComponentScarcity);
    assert!(scarcity.validate().is_ok());
    assert!(validate_case_binding(&scarcity, ID, ID).is_ok());
    assert!(matches!(
        validate_case_binding(&scarcity, OTHER_ID, ID),
        Err(DispositionError::Conflict)
    ));
    assert!(matches!(
        validate_case_binding(&scarcity, ID, OTHER_ID),
        Err(DispositionError::Conflict)
    ));
    let released = case(ReviewReason::ReleaseIntentUnresolved);
    assert!(released.validate().is_ok());
    assert!(matches!(
        validate_case_binding(&released, ID, ID),
        Err(DispositionError::Conflict)
    ));
    let consumed = case(ReviewReason::PriorConsumptionWithoutAllocation);
    assert!(consumed.validate().is_ok());
    assert!(matches!(
        validate_case_binding(&consumed, ID, ID),
        Err(DispositionError::Conflict)
    ));
}

#[test]
fn next_generation_requires_contiguous_history_without_allocation() {
    assert!(validate_next_generation(&[], 1).is_ok());
    assert!(validate_next_generation(&[receipt()], 2).is_ok());
    assert!(matches!(
        validate_next_generation(&[receipt()], 1),
        Err(DispositionError::Conflict)
    ));
    let first = receipt();
    let mut second = receipt();
    second.generation = 2;
    let mut third = receipt();
    third.generation = 3;
    assert!(matches!(
        validate_next_generation(&[first, second, third], 3),
        Err(DispositionError::Conflict)
    ));
    let mut allocated = receipt();
    allocated.outcome = DecisionOutcome::Allocated {
        allocation_id: ID.into(),
    };
    assert!(matches!(
        validate_next_generation(&[allocated], 2),
        Err(DispositionError::Conflict)
    ));
}
